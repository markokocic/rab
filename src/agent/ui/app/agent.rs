//! Agent lifecycle — message submission, agent creation, compaction.
//!
//! Extracted from `mod.rs` to reduce file size.

use std::sync::atomic::Ordering;

use super::App;
use super::chat::handle_bang_command;
use super::chat::show_status;
use super::command_handlers::handle_slash_command;
use super::helpers::{expand_skill_command, parse_bang_command};

/// Submit or queue a user message.
/// When streaming, sets pending_submit which is deferred until the current
/// turn finishes (the main loop skips start_agent_loop while is_streaming).
/// When idle, starts a new agent loop immediately.
pub fn submit_message(app: &mut App, message: String) {
    let trimmed = message.trim().to_string();

    // Don't submit empty messages (pi-style)
    if trimmed.is_empty() {
        return;
    }

    // Step 1: Expand /skill:name [args] (pi-style: skill before template)
    let after_skill = if trimmed.starts_with("/skill:") {
        expand_skill_command(&trimmed, &app.skills)
    } else {
        trimmed.clone()
    };

    // Step 2: Expand prompt templates (/name) on the result (pi-compatible order)
    let expanded =
        crate::agent::prompt_templates::expand_prompt_template(&after_skill, &app.prompt_templates);

    // If anything expanded (skill or template), submit the expanded content
    if expanded != after_skill || after_skill != trimmed {
        // Handle streaming for expanded content (same logic as below)
        if app.is_streaming && app.agent.as_ref().is_some_and(|a| a.is_streaming()) {
            let steer_msg = super::user_agent_message(&expanded);
            if let Some(ref agent) = app.agent {
                agent.steer(steer_msg);
                app.status_text = Some("Skill/template steering message sent".into());
            }
            return;
        }
        if app.is_streaming {
            // Stale streaming flag — reset
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
        }
        app.pending_submit = Some(expanded);
        return;
    }

    // Handle /commands (need TUI from app for overlays)
    if trimmed.starts_with('/') {
        handle_slash_command(app, &trimmed);
        return;
    }

    // Handle ! and !! bang commands
    if let Some((cmd, _exclude)) = parse_bang_command(&trimmed) {
        handle_bang_command(app, cmd);
        return;
    }

    if app.is_streaming {
        // When streaming, queue via steer(). The agent loop picks it up
        // between tool calls or after the current assistant turn, then
        // continues processing. Do NOT add to chat here — MessageStart
        // handler adds it when the agent loop processes the queued message.
        if app.agent.as_ref().is_some_and(|a| a.is_streaming()) {
            let steer_msg = super::user_agent_message(&trimmed);
            if let Some(ref agent) = app.agent {
                agent.steer(steer_msg);
                app.status_text = Some("Steering message sent — will be processed next".into());
            }
            // Reset overflow recovery for the steer'd message
            if let Some(ref mut s) = app.session {
                s.reset_overflow_recovery();
            }
            return; // Don't set pending_submit — agent loop handles this
        } else {
            // Stale streaming flag — agent task finished but is_streaming
            // not reset. Fall through to normal submission path.
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
        }
    }

    // Pi-compatible: reset overflow recovery state at the start of each turn
    if let Some(ref mut s) = app.session {
        s.reset_overflow_recovery();
    }

    // Queue for async start in the main loop
    app.pending_submit = Some(trimmed);
}

/// Build a `yoagent::RetryConfig` from rab's user-facing settings.
fn retry_config_from_settings(settings: &crate::settings::Settings) -> yoagent::RetryConfig {
    let Some(r) = &settings.retry else {
        return yoagent::RetryConfig::default();
    };

    if r.enabled == Some(false) {
        return yoagent::RetryConfig::none();
    }

    let max_retries = r.max_retries.map(|v| v as usize).unwrap_or(3);
    let initial_delay_ms = r.base_delay_ms.unwrap_or(1000);
    let max_delay_ms = r
        .provider
        .as_ref()
        .and_then(|p| p.max_retry_delay_ms)
        .unwrap_or(30_000);

    yoagent::RetryConfig {
        max_retries,
        initial_delay_ms,
        max_delay_ms,
        ..yoagent::RetryConfig::default()
    }
}

/// Build a fresh Agent from the App's current configuration.
pub fn build_fresh_agent(
    app: &App,
    api_key: &str,
    messages: Vec<yoagent::types::AgentMessage>,
) -> yoagent::agent::Agent {
    use yoagent::provider::model::ApiProtocol;

    let preferred = if !app.current_provider.is_empty() {
        Some(app.current_provider.as_str())
    } else {
        app.settings.default_provider.as_deref()
    };

    let resolved = app.registry.resolve(&app.model, preferred).ok();
    let mut mc = resolved
        .as_ref()
        .map(|r| r.model_config.clone())
        .unwrap_or_else(|| crate::agent::base_model_config(&app.model));
    let api_key = resolved
        .as_ref()
        .map(|r| r.api_key.as_str())
        .filter(|k| !k.is_empty())
        .unwrap_or(api_key);

    // Inject provider attribution/session headers (pi-compatible).
    let session_id = app.session.as_ref().map(|s| s.session_id());
    let enable_telemetry = app.settings.enable_install_telemetry.unwrap_or(false);
    crate::provider::inject_provider_attribution_headers(
        &mut mc,
        session_id.as_deref(),
        enable_telemetry,
    );

    let rab_compat = resolved
        .as_ref()
        .map(|r| r.rab_compat.clone())
        .unwrap_or_default();

    let tools: Vec<Box<dyn yoagent::types::AgentTool>> = app
        .extensions
        .iter()
        .filter(|ext| crate::extension::is_extension_enabled(ext.as_ref(), &app.settings))
        .flat_map(|ext| ext.tools())
        .map(|twm| Box::new(twm) as Box<dyn yoagent::types::AgentTool>)
        .collect();

    let agent = match mc.api {
        ApiProtocol::OpenAiCompletions => yoagent::agent::Agent::from_provider(
            crate::provider::openai_compat::RabOpenAiCompatProvider::new(rab_compat),
            mc.clone(),
        ),
        ApiProtocol::AnthropicMessages => yoagent::agent::Agent::from_provider(
            crate::provider::anthropic::RabAnthropicProvider,
            mc.clone(),
        ),
        ApiProtocol::OpenAiResponses => yoagent::agent::Agent::from_config(mc.clone()),
        ApiProtocol::GoogleGenerativeAi => yoagent::agent::Agent::from_config(mc.clone()),
        _ => yoagent::agent::Agent::from_config(mc.clone()),
    };

    let retry_config = retry_config_from_settings(&app.settings);
    let thinking_level = map_thinking_level(app.thinking_level.as_deref());

    let context_window = mc.context_window;
    let execution_limits = yoagent::context::ExecutionLimits {
        max_total_tokens: usize::MAX,
        max_turns: usize::MAX,
        max_duration: std::time::Duration::from_secs(u64::MAX),
    };
    let context_config = yoagent::context::ContextConfig::from_context_window(context_window);

    agent
        .with_api_key(api_key)
        .with_system_prompt(&app.system_prompt)
        .with_thinking(thinking_level)
        .with_retry_config(retry_config)
        .with_messages(messages)
        .with_tools(tools)
        .with_context_config(context_config)
        .with_execution_limits(execution_limits)
        .on_before_turn({
            let stop_flag = app.stop_requested.clone();
            move |_, _| !stop_flag.load(Ordering::Relaxed)
        })
}

/// Map rab's thinking level string to yoagent's ThinkingLevel enum.
pub fn map_thinking_level(level: Option<&str>) -> yoagent::types::ThinkingLevel {
    match level {
        Some("off") => yoagent::types::ThinkingLevel::Off,
        Some("low") => yoagent::types::ThinkingLevel::Low,
        Some("medium") => yoagent::types::ThinkingLevel::Medium,
        Some("high") | Some("max") | Some("xhigh") => yoagent::types::ThinkingLevel::High,
        _ => yoagent::types::ThinkingLevel::High,
    }
}

/// Start an agent turn asynchronously. Called from the main loop only when
/// the agent is idle (the main loop guards with `!app.is_streaming`).
/// Reuses the existing agent across turns (single-agent model) so that
/// steer/follow-up queues and in-flight tool state survive across turns.
/// If no agent exists yet (first turn), creates a fresh one.
/// Messages are always synced from the session (error-filtered source) at
/// the start of each turn to avoid leaking transient provider errors.
pub async fn start_agent_loop(
    app: &mut App,
    message: String,
    preloaded: Option<Vec<yoagent::types::AgentMessage>>,
) {
    if app.session.is_none() {
        return;
    }

    // Reset stop flag — new turn starting
    app.stop_requested.store(false, Ordering::Relaxed);

    // Compose preloaded messages from all sources
    let mut all_preloaded: Vec<yoagent::types::AgentMessage> = Vec::new();
    // 1. Next-turn queue (queued while idle via /nextTurn)
    all_preloaded.append(&mut app.next_turn_queue);
    // 2. Saved queued messages from a previous stop-requested
    all_preloaded.append(&mut app.saved_queued_msgs);
    // 3. Explicit preloaded (from steer/follow-up drain at idle)
    if let Some(msgs) = preloaded {
        all_preloaded.extend(msgs);
    }

    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);

    // Build or reuse agent. On the first turn the session has no messages;
    // on subsequent turns the reused agent already has messages restored
    // by agent.finish() — no need to sync from session here.
    let msgs = app
        .session
        .as_ref()
        .map(|s| s.session().build_context().messages)
        .unwrap_or_default();

    // Record model/thinking changes in the session before borrowing agent
    let model = app.model.clone();
    app.record_model_change(&model);
    if let Some(ref mut session) = app.session {
        session.on_thinking_level_change(app.thinking_level.as_deref().unwrap_or("off"));
    }

    // Refresh OAuth token if expired (e.g. GitHub Copilot tokens live ~15 min).
    // This covers both the first turn (token expired before rab started) and
    // subsequent turns (token expired mid-session).
    let fresh_oauth_key = {
        let provider = app.current_provider.clone();
        if crate::provider::oauth::is_built_in(&provider) {
            crate::provider::auth::refresh_oauth_token(&provider).await
        } else {
            None
        }
    };

    let agent: &mut yoagent::agent::Agent = match &mut app.agent {
        Some(existing) => {
            // Reuse existing agent — messages are already correct from
            // agent.finish(). Compaction sync is handled separately by
            // handle_auto_compact / handle_compact_command.
            // Update api_key in case the OAuth token was just refreshed.
            if let Some(ref key) = fresh_oauth_key {
                existing.api_key = key.clone();
            }
            existing
        }
        None => {
            let fallback_key = fresh_oauth_key.as_deref().unwrap_or(&app.api_key);
            app.agent = Some(build_fresh_agent(app, fallback_key, msgs));
            // SAFETY: we just set app.agent to Some(...)
            app.agent.as_mut().unwrap()
        }
    };

    // Apply steering/follow-up queue modes from settings (pi-compatible)
    if let Some(ref mode_str) = app.settings.steering_mode {
        let mode = match mode_str.as_str() {
            "all" => yoagent::agent::QueueMode::All,
            _ => yoagent::agent::QueueMode::OneAtATime,
        };
        agent.set_steering_mode(mode);
    }
    if let Some(ref mode_str) = app.settings.follow_up_mode {
        let mode = match mode_str.as_str() {
            "all" => yoagent::agent::QueueMode::All,
            _ => yoagent::agent::QueueMode::OneAtATime,
        };
        agent.set_follow_up_mode(mode);
    }

    // Start the turn.
    // When preloaded messages are provided, use prompt_messages so they're
    // all injected in one turn; the main message is appended to the list.
    let user_msg = super::user_agent_message(&message);
    let mut rx = if !all_preloaded.is_empty() {
        all_preloaded.push(user_msg);
        agent.prompt_messages(all_preloaded).await
    } else {
        agent.prompt(message).await
    };

    // Forward events from the agent's receiver to the UI channel.
    // This runs concurrently while the agent loop processes the turn.
    let tx = app.event_tx.clone();
    let handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    app.forward_handle = Some(handle);
}

/// Handle keyboard input for the session picker.
pub fn handle_session_picker_input(app: &mut App, key: &crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;

    let Some(ref mut picker) = app.session_picker else {
        return;
    };

    // Handle rename mode input first
    if picker.is_rename_mode() {
        match key.code {
            KeyCode::Esc => {
                picker.cancel_rename();
            }
            KeyCode::Enter => {
                picker.handle_rename_char('\n');
                // Process pending rename — open the target session, write name, drop
                if let Some((path, name)) = picker.take_pending_rename() {
                    let mut session = crate::agent::session::Session::open(&path, Some(&app.cwd));
                    session.append_session_info(&name);
                    app.status_text = Some(format!("Session renamed to: {}", name));
                }
            }
            KeyCode::Char(c) => {
                picker.handle_rename_char(c);
            }
            KeyCode::Backspace => {
                picker.handle_rename_char('\x7f');
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.session_picker = None;
            app.status_text = None;
        }
        KeyCode::Enter => {
            if let Some(path) = picker.selected_path() {
                let path = path.clone();
                app.session_picker = None;
                app.status_text = None;
                // Delegate to the shared SessionSwitched handler
                app.pending_command_result =
                    Some(crate::extension::CommandResult::SessionSwitched { path });
            }
        }
        KeyCode::Up => {
            picker.select_prev();
        }
        KeyCode::Down => {
            picker.select_next();
        }
        KeyCode::Char('/') => {
            picker.set_filter("");
        }
        KeyCode::Char(c) if c == 'r' || c == 'R' => {
            // Start rename mode for selected session
            picker.start_rename();
        }
        KeyCode::Char(c) => {
            let mut filter = picker.filter().to_string();
            filter.push(c);
            picker.set_filter(&filter);
        }
        KeyCode::Backspace => {
            let mut filter = picker.filter().to_string();
            filter.pop();
            picker.set_filter(&filter);
        }
        _ => {}
    }
}

/// Handle manual compaction asynchronously.
/// Called from the main loop when pending_compact is set.
pub async fn handle_compact_command(app: &mut App, custom_instructions: Option<String>) {
    if app.session.is_none() {
        show_status(app, "No active session to compact".to_string());
        return;
    }

    // Pi-compatible: disconnect from agent and abort streaming before compact.
    // This ensures compact runs in a consistent state (pi's compact() calls
    // _disconnectFromAgent() + abort() as its first internal steps).
    if app.is_streaming {
        super::handlers::interrupt_streaming(app);
    }

    let agent_session = app.session.as_mut().unwrap();

    app.working.start();

    match agent_session
        .run_manual_compact(custom_instructions.as_deref())
        .await
    {
        Ok(summary) => {
            app.working.stop();
            app.status_text = None;
            if summary.is_empty() {
                // Nothing was compacted — check why (matching pi)
                let reason = "Nothing to compact (session too small)";
                show_status(app, reason.to_string());
            } else {
                app.rebuild_from_session_context();
                show_status(app, "Compaction completed".to_string());
            }
        }
        Err(e) => {
            app.working.stop();
            app.status_text = None;
            show_status(app, format!("Compaction failed: {}", e));
        }
    }
}

/// Pi-compatible: auto-compaction check after agent ends.
/// Calls `check_auto_compact()` on the session. If compaction was performed,
/// rebuilds the chat from the updated session context and updates agent state.
pub async fn handle_auto_compact(app: &mut App) {
    if app.session.is_none() {
        return;
    }

    let agent_session = app.session.as_mut().unwrap();

    match agent_session.check_auto_compact().await {
        Ok(true) => {
            app.rebuild_from_session_context();
            // Refresh footer stats (token counts may have changed)
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }
            app.status_text = Some("Auto-compaction completed".to_string());
        }
        Ok(false) => {
            // No compaction needed — nothing to do
        }
        Err(e) => {
            eprintln!("Warning: Auto-compaction failed: {}", e);
            app.status_text = Some(format!("Auto-compaction skipped: {}", e));
        }
    }
}
