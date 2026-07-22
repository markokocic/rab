//! Agent event handler — processes yoagent `AgentEvent`s for UI updates and persistence.
//!
//! Extracted from `app.rs` to reduce file size. The single `handle_agent_event` function
//! is the primary dispatcher for all agent lifecycle events.

use std::cell::RefCell;
use std::rc::Rc;

use crate::tui::Component;
use crate::tui::components::Spacer;

use super::App;

/// Process a single agent event: persist to session and update the UI.
pub fn handle_agent_event(app: &mut App, event: yoagent::types::AgentEvent) {
    // ── Persistence: delegate to the shared handler (single source of truth) ──
    // Handle with &event before the display match consumes it.
    {
        let ev = &event;
        if let E::MessageEnd { message } = ev {
            if crate::agent::types::message_is_user(message)
                && let Some(ref mut s) = app.session
            {
                s.reset_overflow_recovery();
            }
            if crate::agent::types::message_error(message).is_none()
                && !crate::agent::types::message_is_system_stop(message)
                && let Some(ref mut s) = app.session
            {
                s.on_agent_event(ev);
            }
        }
        if let E::ToolExecutionEnd { tool_call_id, .. } = ev
            && tool_call_id != "__bang__"
            && let Some(ref mut s) = app.session
        {
            s.on_agent_event(ev);
        }
        if let E::AgentEnd { .. } = ev
            && let Some(ref mut s) = app.session
        {
            s.on_agent_event(ev);
        }
    }

    // ── Display logic (consumes owned event) ──
    use yoagent::types::AgentEvent as E;
    match event {
        E::AgentStart => {
            app.is_streaming = true;
            app.working.start();
            app.refresh_git_branch();
        }
        E::TurnStart => {}
        E::MessageStart { message } => {
            // Add user messages to chat when the agent loop processes them.
            // Covers both the initial prompt (non-streaming) and
            // steered/follow-up messages queued during streaming.
            if crate::agent::types::message_is_user(&message) {
                let text = crate::agent::types::message_text(&message);
                if !text.is_empty() {
                    // pi: add Spacer(1) before user messages when chat isn't empty
                    let mut chat = app.chat_container.borrow_mut();
                    if !chat.children().is_empty() {
                        chat.add_child(std::boxed::Box::new(Spacer::new(1)));
                    }
                    chat.add_child(std::boxed::Box::new(
                        crate::agent::ui::components::UserMessageComponent::new(&text),
                    ));
                }
            }
        }
        E::MessageUpdate { delta, .. } => {
            use yoagent::types::StreamDelta;
            match delta {
                StreamDelta::Text { delta } => {
                    if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                        weak.borrow_mut().append_text(&delta);
                    } else {
                        use crate::tui::components::rc_ref_cell_component::RcRefCellComponent;
                        let comp = Rc::new(RefCell::new(
                            crate::agent::ui::components::AssistantMessageComponent::new(&delta),
                        ));
                        if app.hide_thinking {
                            comp.borrow_mut().set_hide_thinking(true);
                        }
                        app.streaming_component = Some(Rc::downgrade(&comp));
                        app.chat_container
                            .borrow_mut()
                            .add_child(std::boxed::Box::new(RcRefCellComponent(comp)));
                    }
                }
                StreamDelta::Thinking { delta } => {
                    if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                        weak.borrow_mut()
                            .add_thinking(&delta, app.thinking_level.clone());
                    } else {
                        use crate::tui::components::rc_ref_cell_component::RcRefCellComponent;
                        let mut comp =
                            crate::agent::ui::components::AssistantMessageComponent::new("");
                        comp.add_thinking(&delta, app.thinking_level.clone());
                        if app.hide_thinking {
                            comp.set_hide_thinking(true);
                        }
                        let comp = Rc::new(RefCell::new(comp));
                        app.streaming_component = Some(Rc::downgrade(&comp));
                        app.chat_container
                            .borrow_mut()
                            .add_child(std::boxed::Box::new(RcRefCellComponent(comp)));
                    }
                }
                StreamDelta::ToolCallDelta { .. } => {}
            }
        }
        E::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            app.pending_tool_executions += 1;
            app.streaming_component = None;
            let name = tool_name;
            let renderer = super::find_tool_renderer(&app.extensions, &name);
            let started_at = std::time::Instant::now();
            let (invalidate_tx, invalidate_rx) =
                crate::agent::ui::components::ToolExecComponent::make_invalidation_channel();
            app.invalidate_rxs.push(invalidate_rx);
            let comp: Rc<RefCell<_>> = {
                let mut tool = crate::agent::ui::components::ToolExecComponent::new(
                    &name,
                    renderer,
                    args.clone(),
                    app.cwd.to_string_lossy().to_string(),
                    tool_call_id.clone(),
                );
                tool.set_started_at(std::time::Instant::now());
                tool.set_invalidate_tx(invalidate_tx);
                Rc::new(RefCell::new(tool))
            };
            comp.borrow_mut().set_expanded(app.tools_expanded);
            app.pending_tools
                .insert(tool_call_id.clone(), Rc::downgrade(&comp));
            app.tool_call_start_times
                .insert(tool_call_id.clone(), started_at);
            super::chat_add(
                app,
                std::boxed::Box::new(crate::agent::ui::components::RcToolExec(comp)),
            );
        }
        E::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => {
            // Forward partial results to the pending tool component (live streaming).
            let partial_text = super::extract_text_content(&partial_result.content);
            if !partial_text.is_empty()
                && let Some(weak) = app.pending_tools.get(&tool_call_id)
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut().append_output(&partial_text);
            }
        }
        E::ToolExecutionEnd {
            tool_call_id,
            tool_name: _,
            result,
            is_error,
        } => {
            app.pending_tool_executions = app.pending_tool_executions.saturating_sub(1);
            let content = super::extract_text_content(&result.content);
            if let Some(weak) = app.pending_tools.get(&tool_call_id)
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut()
                    .set_result_with_details(&content, is_error, Some(result.details));
                app.tool_call_start_times.remove(&tool_call_id);
            }
        }
        E::ProgressMessage {
            text, tool_name, ..
        } => {
            // Bang (!) command progress feeds into pending_tools["__bang__"]
            if let Some(weak) = app.pending_tools.get("__bang__")
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut().append_output(&text);
            } else if tool_name.is_empty() {
                // General progress message (not tool-specific) — show as status
                app.status_text = Some(text.trim().to_string());
            }
        }
        E::TurnEnd { message, .. } => {
            app.streaming_component = None;
            // Refresh footer stats after each turn completes (sooner than
            // AgentEnd, but still bounded by turn frequency — not every frame).
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }
            // Surface provider errors carried by the turn's final message.
            if let Some(err) = crate::agent::types::message_error(&message) {
                super::show_status(app, format!("Provider error: {}", err));
            }
            // Pi-compatible shouldStopAfterTurn: if stop was requested, save
            // the steer/follow-up queues so they're preserved for the next run
            // and the agent loop exits (queues are empty).
            if app
                .stop_requested
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                if let Some(ref agent) = app.agent {
                    app.saved_queued_msgs
                        .append(&mut agent.take_steering_queue());
                    app.saved_queued_msgs
                        .append(&mut agent.take_follow_up_queue());
                }
                app.stop_requested
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        E::AgentEnd { messages } => {
            app.streaming_component = None;
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
            // Refresh footer cached stats from session at turn end (pull-based)
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }
            // Pi-compatible: schedule auto-compaction check after agent ends.
            app.pending_auto_compact = app.auto_compact;
            // Detect silent stops / provider errors: surface any assistant message
            // that ended without visible output (empty content or provider error).
            for msg in messages.iter().rev() {
                if let Some(yoagent::types::Message::Assistant {
                    content,
                    stop_reason,
                    error_message,
                    ..
                }) = msg.as_llm()
                    && stop_reason != &yoagent::types::StopReason::ToolUse
                {
                    if let Some(err) = error_message {
                        super::show_status(app, format!("Provider error: {}", err));
                        break;
                    }
                    // Check for any visible content: non-empty text or tool calls.
                    let has_visible = content.iter().any(|c| match c {
                        yoagent::types::Content::Text { text } => !text.trim().is_empty(),
                        yoagent::types::Content::ToolCall { .. } => true,
                        _ => false,
                    });
                    if !has_visible {
                        super::show_status(
                            app,
                            "The agent returned an empty response. \
                                 This can happen when the provider's context \
                                 limit is exceeded or the model declined to \
                                 respond. Try sending a new message."
                                .to_string(),
                        );
                        break;
                    }
                }
            }
        }
        E::MessageEnd { message } => {
            // Special cases: persist as extension (excluded from LLM context).
            // Normal persistence handled by if-let above before the display match.
            if let Some(err) = crate::agent::types::message_error(&message) {
                super::show_status(app, err.to_string());
                let ext = crate::agent::types::extension_message("error", err, true);
                if let Some(ref mut s) = app.session {
                    s.persist_extension_message(&ext);
                }
            } else if crate::agent::types::message_is_system_stop(&message) {
                let text = crate::agent::types::message_text(&message);
                super::show_status(app, text.clone());
                if let Some(ref mut s) = app.session {
                    let ext = crate::agent::types::extension_message("system_stop", text, true);
                    s.persist_extension_message(&ext);
                }
            } else if crate::agent::types::message_is_extension(&message) {
                // Extension messages: display in chat (persisted by on_agent_event).
                if let Some(text) = crate::agent::types::message_extension_text(&message) {
                    super::show_status(app, text);
                }
            }
        }
        E::InputRejected { reason } => {
            let msg = format!("Input rejected: {}", reason);
            super::show_status(app, msg);
        }
    }
}
