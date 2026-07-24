//! Keyboard input handling and event dispatch.
//!
//! Extracted from `mod.rs` to reduce file size.

use crate::tui::TUI;
use crate::tui::terminal::{ProcessTerminal, TerminalTrait};
use crossterm::event::KeyEvent;
use std::io::Write;

use super::App;
use super::agent::{handle_session_picker_input, submit_message};
use super::chat::{rebuild_chat_from_messages, show_status};
use super::overlays::{open_model_selector, show_help_overlay};
use crate::agent::ui::chat_editor::InputAction;
use crate::agent::ui::theme::color;
use crate::extension::CommandResult;
use crate::tui::Component;

/// Thinking level cycle order (matching pi's thinking level enum). Cycles from
/// highest to lowest so the first press from the default (max) goes to "high"
/// (a step down), not to "off".
const ALL_THINKING_LEVELS: &[&str] = &["max", "high", "medium", "low", "off"];

/// Get the available thinking levels for the current model, filtered by
/// the model's `thinkingLevelMap`. Matches pi's `getSupportedThinkingLevels`.
/// Levels mapped to `null` are unsupported. "max" requires explicit presence
/// in the map (other levels are available unless nulled).
pub fn available_thinking_levels(app: &App) -> Vec<&'static str> {
    let thinking_map: Option<std::collections::HashMap<String, serde_json::Value>> = app
        .registry
        .resolve(&app.model, Some(&app.current_provider))
        .ok()
        .and_then(|r| r.thinking_map);

    match thinking_map {
        Some(map) => ALL_THINKING_LEVELS
            .iter()
            .filter(|level| {
                let mapped = map.get(**level);
                if matches!(mapped, Some(v) if v.is_null()) {
                    return false;
                }
                if **level == "max" {
                    return mapped.is_some();
                }
                true
            })
            .copied()
            .collect(),
        None => ALL_THINKING_LEVELS.to_vec(),
    }
}

/// Update UI section components from app state.
/// Each section is a child of TUI.root rendered in the correct order.
///
/// Layout (top to bottom):
///   header → chat_container (messages) → pending (queued steer/follow-up) → status → working → spacer → editor → footer
pub fn compose_ui(app: &mut App, width: usize) {
    // ── Session picker ──
    if let Some(ref picker) = app.session_picker {
        let (_lines, _cursor_y) = picker.render(width, &app.theme as &dyn crate::tui::Theme);
        app.chat_container.borrow_mut().clear();
        app.pending_section.borrow_mut().set_lines(vec![]);
        app.status_section.borrow_mut().set_lines(vec![]);
        app.working_section.borrow_mut().set_lines(vec![]);
        return;
    }

    // ── Transient status text (pi-style) ──
    let mut status_lines = Vec::new();
    if let Some(ref status) = app.status_text {
        let line = app.theme.fg(color::Dim, &format!(" {}", status));
        status_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
    }
    app.status_section.borrow_mut().set_lines(status_lines);

    // ── Pending messages section (pi-style pendingMessagesContainer) ──
    let mut pending_lines = Vec::new();
    let has_next_turn = !app.next_turn_queue.is_empty();
    let has_saved = !app.saved_queued_msgs.is_empty();
    let has_pending = if let Some(ref agent) = app.agent {
        agent.steering_queue_len() > 0
            || agent.follow_up_queue_len() > 0
            || app.pending_submit.is_some()
            || has_next_turn
            || has_saved
    } else {
        app.pending_submit.is_some() || has_next_turn || has_saved
    };
    if has_pending {
        pending_lines.push(String::new());

        for msg in &app.next_turn_queue {
            let text = crate::agent::types::message_text(msg);
            let preview = if text.len() > width.saturating_sub(14) {
                format!("{}…", &text[..width.saturating_sub(14)])
            } else {
                text
            };
            if !preview.is_empty() {
                let line = app
                    .theme
                    .fg(color::Dim, &format!(" Next turn: {}", preview));
                pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
            }
        }

        for msg in &app.saved_queued_msgs {
            let text = crate::agent::types::message_text(msg);
            let preview = if text.len() > width.saturating_sub(14) {
                format!("{}…", &text[..width.saturating_sub(14)])
            } else {
                text
            };
            if !preview.is_empty() {
                let line = app.theme.fg(color::Dim, &format!(" Saved: {}", preview));
                pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
            }
        }

        if let Some(ref msg) = app.pending_submit {
            let preview = if msg.len() > width.saturating_sub(12) {
                format!("{}…", &msg[..width.saturating_sub(12)])
            } else {
                msg.clone()
            };
            let line = app
                .theme
                .fg(color::Dim, &format!(" 📝 queued: {}", preview));
            pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
        }

        if let Some(ref agent) = app.agent {
            for msg in agent.steering_queue_snapshot() {
                let text = crate::agent::types::message_text(&msg);
                let preview = if text.len() > width.saturating_sub(14) {
                    format!("{}…", &text[..width.saturating_sub(14)])
                } else {
                    text
                };
                if !preview.is_empty() {
                    let line = app.theme.fg(color::Dim, &format!(" Steering: {}", preview));
                    pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
                }
            }
            for msg in agent.follow_up_queue_snapshot() {
                let text = crate::agent::types::message_text(&msg);
                let preview = if text.len() > width.saturating_sub(14) {
                    format!("{}…", &text[..width.saturating_sub(14)])
                } else {
                    text
                };
                if !preview.is_empty() {
                    let line = app
                        .theme
                        .fg(color::Dim, &format!(" Follow-up: {}", preview));
                    pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
                }
            }

            let dequeue_keys = crate::tui::keybindings::get_keybindings()
                .get_keys(crate::tui::keybindings::ACTION_APP_MESSAGE_DEQUEUE);
            if !dequeue_keys.is_empty() {
                let hint = app.theme.fg(
                    color::Dim,
                    &format!(" ↳ {} to edit all queued messages", dequeue_keys[0]),
                );
                pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&hint, width));
            }
        }
    }
    app.pending_section.borrow_mut().set_lines(pending_lines);

    // ── Working indicator (pi-style) ──
    let mut working_lines = Vec::new();
    working_lines.extend(app.working.render(width));
    app.working_section.borrow_mut().set_lines(working_lines);
}

/// Create an AgentMessage for a user text input (used for steer/follow_up).
pub fn user_agent_message(text: &str) -> yoagent::types::AgentMessage {
    yoagent::types::AgentMessage::Llm(yoagent::types::Message::User {
        content: vec![yoagent::types::Content::Text {
            text: text.to_string(),
        }],
        timestamp: yoagent::types::now_ms(),
    })
}

/// Handle keyboard input. Mirrors pi's InteractiveMode key dispatch.
pub fn handle_input(app: &mut App, tui: &mut TUI, term: &mut ProcessTerminal, key: &KeyEvent) {
    // ── Session picker input handling ──
    if app.session_picker.is_some() {
        handle_session_picker_input(app, key);
        return;
    }

    // ── Check if any TUI overlay is active ──
    if tui.has_overlays() && matches!(key.code, crossterm::event::KeyCode::Esc) {
        tui.pop_overlay();
        return;
    }
    if tui.has_overlays() {
        return;
    }

    // ── Route input to root container children ──
    if tui.handle_input(key) {
        return;
    }

    // ── Dispatch to ChatEditor ──
    let action = app.editor.borrow_mut().handle_input(key);
    match action {
        InputAction::Handled => {}
        InputAction::Escape => {
            if app.is_streaming {
                interrupt_streaming(app);
            } else {
                app.editor.borrow_mut().editor.set_text("");
            }
        }
        InputAction::Clear => {
            handle_clear(app);
        }
        InputAction::Exit => {
            app.should_quit = true;
        }
        InputAction::ThinkingCycle => {
            handle_thinking_cycle(app);
        }
        InputAction::ModelSelector => {
            open_model_selector(app, tui);
        }
        InputAction::ModelCycleForward => {
            handle_model_cycle(app, 1);
        }
        InputAction::ModelCycleBackward => {
            handle_model_cycle(app, -1);
        }
        InputAction::ToggleThinking => {
            app.hide_thinking = !app.hide_thinking;
            app.propagate_hide_thinking();
            app.settings.set_hide_thinking(Some(app.hide_thinking));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save thinking visibility: {}", e));
            }
            show_status(
                app,
                if app.hide_thinking {
                    "Thinking blocks: hidden"
                } else {
                    "Thinking blocks: visible"
                },
            );
        }
        InputAction::ToolsExpand => {
            handle_tools_expand(app);
        }
        InputAction::EditorExternal => {
            handle_editor_external(app, tui, term);
        }
        InputAction::Help => {
            show_help_overlay(app, tui);
        }
        InputAction::Submit(text) => {
            submit_message(app, text);
        }
        InputAction::FollowUp(text) => {
            handle_follow_up(app, text);
        }
        InputAction::Dequeue => {
            if let Some(msg) = app.pending_submit.take() {
                app.editor.borrow_mut().editor.set_text(&msg);
                app.status_text = Some("Queued message restored to editor".into());
            } else {
                app.status_text = Some("No queued message".into());
            }
        }
        InputAction::CompactToggle => {
            handle_compact_toggle(app);
        }
        InputAction::SessionResume => {
            app.pending_command_result = Some(CommandResult::OpenSessionSelector);
        }
    }
}

/// Handle Ctrl+C: clear editor (double-press within 500ms = exit).
pub(crate) fn handle_clear(app: &mut App) {
    let now = std::time::Instant::now();
    let elapsed = now.duration_since(app.last_clear_time);
    app.last_clear_time = now;

    if app.is_streaming {
        interrupt_streaming(app);
    } else if elapsed.as_millis() < 500 {
        app.should_quit = true;
    } else {
        app.editor.borrow_mut().editor.set_text("");
        app.status_text = Some("Cleared".into());
    }
}

/// Cycle thinking level through the levels available for the current model.
pub fn handle_thinking_cycle(app: &mut App) {
    if app.available_models.is_empty() && app.model.is_empty() {
        app.status_text = Some("No model selected".into());
        return;
    }

    let levels = available_thinking_levels(app);
    if levels.is_empty() {
        return;
    }

    let current = app.thinking_level.as_deref().unwrap_or("off");
    let next = match levels.iter().position(|&l| l == current) {
        Some(pos) => levels[(pos + 1) % levels.len()],
        None => "off",
    };

    app.thinking_level = Some(next.to_string());
    app.editor
        .borrow_mut()
        .update_border_color(Some(next), &app.theme as &dyn crate::tui::Theme);
    app.settings
        .set_default_thinking_level(Some(next.to_string()));
    if let Err(e) = app.settings.save() {
        app.status_text = Some(format!("Failed to save thinking level: {}", e));
    }
    if let Some(ref mut agent_session) = app.session {
        agent_session.on_thinking_level_change(next);
    }
    if let Some(ref s) = app.session {
        app.footer.borrow_mut().refresh_from_session(s.session());
    }
    show_status(app, format!("Thinking level: {}", next));
}

/// Cycle model forward (dir=1) or backward (dir=-1).
pub fn handle_model_cycle(app: &mut App, dir: isize) {
    let authenticated_models = app.registry.list_authenticated_model_ids();
    let model_pool: Vec<String> = if let Some(ref scoped) = app.scoped_model_ids
        && !scoped.is_empty()
    {
        scoped
            .iter()
            .filter_map(|full_id| {
                let (_provider, model_id) = full_id.split_once('/')?;
                if authenticated_models.iter().any(|m| m == model_id) {
                    Some(model_id.to_string())
                } else {
                    None
                }
            })
            .collect()
    } else {
        authenticated_models
    };

    let n = model_pool.len();
    if n == 0 {
        app.status_text = Some("No models available".into());
        return;
    }

    let current_idx = model_pool.iter().position(|m| m == &app.model);
    let next_idx = match current_idx {
        Some(idx) => (idx as isize + dir).rem_euclid(n as isize) as usize,
        None => 0,
    };

    let model = model_pool[next_idx].clone();
    app.model = model.clone();
    app.current_provider = app
        .registry
        .provider_for_model(&model, Some(&app.current_provider))
        .unwrap_or_default();
    app.record_model_change(&model);
    let provider = &app.current_provider;
    if !provider.is_empty() {
        app.settings
            .set_default_model_and_provider(provider, &model);
    } else {
        app.settings.set_default_model(Some(model.clone()));
    }
    if let Err(e) = app.settings.save() {
        eprintln!("Warning: failed to save default model: {}", e);
    }
    show_status(app, format!("Model: {}", app.model));
}

/// Toggle all tool output expansion (Ctrl+O).
pub fn handle_tools_expand(app: &mut App) {
    app.tools_expanded = !app.tools_expanded;
    app.collapse_tool_output = !app.tools_expanded;

    app.header.borrow_mut().set_expanded(app.tools_expanded);

    let mut chat = app.chat_container.borrow_mut();
    for child in chat.children_mut().iter_mut() {
        child.set_expanded(app.tools_expanded);
    }
    drop(chat);

    app.settings
        .set_collapse_tool_output(Some(app.collapse_tool_output));
    if let Err(e) = app.settings.save() {
        app.status_text = Some(format!("Failed to save tool output setting: {}", e));
    }
    show_status(
        app,
        if app.tools_expanded {
            "Tool output: expanded"
        } else {
            "Tool output: collapsed"
        },
    );
}

/// Open external editor ($VISUAL / $EDITOR) for current editor content.
pub fn handle_editor_external(app: &mut App, tui: &mut TUI, term: &mut ProcessTerminal) {
    let editor_cmd = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_default();

    if editor_cmd.is_empty() {
        app.status_text = Some("No editor configured. Set $VISUAL or $EDITOR.".into());
        return;
    }

    let tmp_dir = std::env::temp_dir();
    let tmp_file = tmp_dir.join(format!(
        "rab-editor-{}.md",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    let current_text = app.editor.borrow().editor.get_text();
    if let Err(e) = std::fs::write(&tmp_file, &current_text) {
        app.status_text = Some(format!("Failed to write temp file: {}", e));
        return;
    }

    let parts: Vec<&str> = editor_cmd.split(' ').collect();
    let (editor, args) = parts.split_first().unwrap_or((&"", &[]));

    // ── Suspend TUI ──
    app.status_text = Some(format!("Opening {} ...", editor_cmd));
    let mut suspend_buf = Vec::new();
    let _ = term.stop(&mut suspend_buf);
    let _ = term.show_cursor(&mut suspend_buf);
    if !suspend_buf.is_empty() {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(&suspend_buf);
        let _ = handle.flush();
    }

    crate::tui::terminal::stop_stdin_reader();
    crate::tui::terminal::join_stdin_reader();

    // ── Run editor ──
    let status = std::process::Command::new(editor)
        .args(args)
        .arg(&tmp_file)
        .status();

    // ── Resume TUI ──
    let mut resume_buf = Vec::new();
    let _ = term.start(&mut resume_buf);
    let _ = term.hide_cursor(&mut resume_buf);
    if !resume_buf.is_empty() {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(&resume_buf);
        let _ = handle.flush();
    }
    crate::tui::terminal::start_stdin_reader();
    tui.request_render();

    match status {
        Ok(status) if status.success() => {
            if let Ok(new_content) = std::fs::read_to_string(&tmp_file) {
                let trimmed = new_content.trim_end_matches('\n').to_string();
                app.editor.borrow_mut().editor.set_text(&trimmed);
                app.editor.borrow_mut().check_autocomplete();
            }
            let _ = std::fs::remove_file(&tmp_file);
            app.status_text = Some("Editor closed".into());
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&tmp_file);
            app.status_text = Some("Editor exited with non-zero status".into());
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_file);
            app.status_text = Some(format!("Failed to launch editor: {}", e));
        }
    }
}

/// Toggle auto-compact indicator (Ctrl+Shift+C).
pub fn handle_compact_toggle(app: &mut App) {
    app.auto_compact = !app.auto_compact;
    app.footer.borrow_mut().set_auto_compact(app.auto_compact);

    if let Some(ref mut s) = app.session {
        s.set_auto_compact(app.auto_compact);
    }

    app.settings.set_auto_compact(Some(app.auto_compact));
    if let Err(e) = app.settings.save() {
        eprintln!("Warning: failed to save auto_compact setting: {}", e);
    }

    app.status_text = Some(
        if app.auto_compact {
            "Auto-compact: on"
        } else {
            "Auto-compact: off"
        }
        .into(),
    );
}

/// Queue a follow-up message (Alt+Enter) during streaming.
pub fn handle_follow_up(app: &mut App, text: String) {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return;
    }

    if app.is_streaming && app.agent.as_ref().is_some_and(|a| a.is_streaming()) {
        let follow_msg = user_agent_message(&trimmed);
        if let Some(ref agent) = app.agent {
            agent.follow_up(follow_msg);
            app.status_text = Some("Follow-up queued — will send when agent finishes".into());
        }
        return;
    }

    if let Some(ref agent) = app.agent
        && !agent.is_streaming()
        && (agent.steering_queue_len() > 0 || agent.follow_up_queue_len() > 0)
    {
        let mut msgs = agent.take_steering_queue();
        msgs.extend(agent.take_follow_up_queue());
        msgs.push(user_agent_message(&trimmed));
        app.pending_preloaded_msgs = Some(msgs);
        app.pending_submit = Some(trimmed);
        app.status_text = Some("Queued messages + follow-up will be sent next".into());
        return;
    }

    if app.is_streaming {
        app.is_streaming = false;
    }
    submit_message(app, trimmed);
}

/// Interrupt streaming agent and restore queued messages to editor.
pub fn interrupt_streaming(app: &mut App) {
    if let Some(ref agent) = app.agent {
        agent.abort();
    }
    if let Some(handle) = app.forward_handle.take() {
        handle.abort();
    }
    if let Some(handle) = app.bash_abort_handle.take() {
        handle.abort();
    }
    app.agent = None;
    app.is_streaming = false;
    app.working.stop();
    app.footer.borrow_mut().set_streaming(false);

    // If the session ended with an orphaned user message (no assistant
    // response), prune it to avoid sending consecutive user messages
    // to the LLM on the next turn (causes HTTP 400 on strict providers).
    if let Some(ref mut s) = app.session {
        let session = s.session_mut();
        let pruned = session.prune_orphan_user_message();
        if pruned {
            s.ensure_flushed();
        }
    }

    if let Some(ref s) = app.session {
        let ctx = s.session().build_context();
        let mut chat = app.chat_container.borrow_mut();
        rebuild_chat_from_messages(
            &mut chat,
            &ctx.messages,
            &app.cwd.to_string_lossy(),
            app.hide_thinking,
            app.collapse_tool_output,
            &app.extensions,
        );
    }

    app.status_text = Some("Interrupted".into());
}
