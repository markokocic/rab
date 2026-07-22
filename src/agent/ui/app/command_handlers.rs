//! Slash command dispatch and result handling.
//!
//! Extracted from `mod.rs` to reduce file size.

use std::path::PathBuf;
use std::sync::Arc;

use crate::builtin::export;
use crate::extension::{CommandResult, Extension, ToolRenderer};
use yoagent::types::AgentMessage;
use yoagent::types::AgentTool;

use super::App;
use super::chat::chat_add;
use super::chat::copy_to_clipboard;
use super::chat::show_status;
use crate::tui::components::Text;

/// Handle slash commands by dispatching through extension command handlers.
/// For commands that need TUI access (overlays), the result is stored in
/// `pending_command_result` and consumed in the main loop where TUI is available.
/// Simple results (Info, Quit, etc.) are handled immediately.
pub fn handle_slash_command(app: &mut App, input: &str) {
    let (cmd_name, args) = match input.split_once(' ') {
        Some((cmd, rest)) => (cmd.trim_start_matches('/'), rest),
        None => (input.trim_start_matches('/'), ""),
    };

    // Helper: is the extension that owns this command enabled?
    fn is_ext_enabled(ext: &dyn Extension, settings: &crate::settings::Settings) -> bool {
        crate::extension::is_extension_enabled(ext, settings)
    }

    // Find the command handler first (before mutable borrow on app)
    for ext in app.extensions.iter() {
        if !is_ext_enabled(ext.as_ref(), &app.settings) {
            continue;
        }
        for cmd in ext.commands() {
            if cmd.name == cmd_name {
                // Execute the handler here while we have immutably borrowed app,
                // then use the result after dropping the borrow.
                let result = cmd.handler.execute(args);
                match result {
                    Ok(result) => {
                        // Drop the iterator borrow before mutating app
                        drop((ext, cmd));
                        handle_command_result(app, result);
                        return;
                    }
                    Err(e) => {
                        drop((ext, cmd));
                        show_status(app, format!("Error executing /{}: {}", cmd_name, e));
                        return;
                    }
                }
            }
        }
    }

    // Unknown command
    let available: Vec<&str> = app.commands.iter().map(|(n, _)| n.as_str()).collect();
    app.status_text = Some(format!(
        "Unknown command: /{}. Available: {}",
        cmd_name,
        available.join(", ")
    ));
}

/// Handle a CommandResult from a slash command.
/// Simple results are applied immediately; overlay-requiring ones
/// are stored in `pending_command_result` for the main loop.
pub fn handle_command_result(app: &mut App, result: CommandResult) {
    match result {
        CommandResult::Info(msg) => {
            show_status(app, msg.clone());
        }
        CommandResult::Quit => {
            app.should_quit = true;
        }
        CommandResult::ModelChanged(model) => {
            // Handle "provider/model" format (e.g., "deepseek/deepseek-v4-flash")
            if let Some((provider, model_id)) = model.split_once('/') {
                let provider = provider.trim();
                let model_id = model_id.trim();
                app.current_provider = provider.to_string();
                app.model = model_id.to_string();
            } else {
                app.model = model.clone();
                app.current_provider = app
                    .registry
                    .provider_for_model(&model, Some(&app.current_provider))
                    .unwrap_or_default();
            }
            let model_ref = app.model.clone();
            app.record_model_change(&model_ref);
            app.status_text = Some(format!("Model: {}/{}", app.current_provider, app.model));
        }
        CommandResult::ShowHelp => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::OpenSessionSelector => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::Reloaded => {
            app.refresh_registry();

            // Refresh cached model list from the updated registry.
            {
                let models = app.registry.list_models();
                let provider_models: Vec<(String, String)> = app
                    .registry
                    .list_model_provider_tuples()
                    .into_iter()
                    .map(|(p, m, _)| (p, m))
                    .collect();
                app.available_models = models.clone();
                for ext in app.extensions.iter() {
                    if let Some(cmd) = ext
                        .as_any()
                        .downcast_ref::<crate::builtin::extension::BuiltinExtension>()
                    {
                        cmd.set_available_models(models.clone());
                        cmd.set_provider_models(provider_models.clone());
                        break;
                    }
                }
            }

            // 0. Notify extensions of imminent shutdown (pi-compatible: session_shutdown)
            for ext in app.extensions.iter() {
                ext.on_session_shutdown("reload");
            }

            // 1. Reload settings from disk (pi-compatible)
            let mut reload_parts: Vec<&str> = Vec::new();
            match app.settings.reload(&app.cwd) {
                Err(e) => {
                    app.status_text = Some(format!("Failed to reload settings: {}", e));
                }
                Ok(()) => {
                    reload_parts.push("settings");
                    // Apply reloaded settings to runtime state
                    if let Some(level) = app.settings.default_thinking_level.clone() {
                        app.thinking_level = Some(level.clone());
                        if let Some(ref mut s) = app.session {
                            s.on_thinking_level_change(&level);
                        }
                        if let Some(ref s) = app.session {
                            app.footer.borrow_mut().refresh_from_session(s.session());
                        }
                    }
                    app.hide_thinking = app.settings.hide_thinking.unwrap_or(true);
                    app.propagate_hide_thinking();
                    app.editor.borrow_mut().update_border_color(
                        app.thinking_level.as_deref(),
                        &app.theme as &dyn crate::tui::Theme,
                    );

                    // Apply reloaded auto_compact setting
                    app.auto_compact = app.settings.get_auto_compact();
                    if let Some(ref mut s) = app.session {
                        s.set_auto_compact(app.auto_compact);
                    }
                    app.footer.borrow_mut().set_auto_compact(app.auto_compact);

                    // Apply reloaded collapse_tool_output setting
                    app.collapse_tool_output = app.settings.collapse_tool_output.unwrap_or(false);
                    app.tools_expanded = !app.collapse_tool_output;

                    // 2. Re-apply theme from reloaded settings (pi-compatible)
                    if let Some(ref theme_name) = app.settings.theme
                        && crate::agent::ui::theme::set_theme(theme_name).is_ok()
                    {
                        app.theme = crate::agent::ui::theme::current_theme().clone();
                        reload_parts.push("theme");
                    }
                }
            }

            // 3. Reload keybindings from disk (pi-compatible)
            let mut kb = crate::tui::keybindings::Keybindings::with_defaults();
            if let Some(home) = directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".rab").join("keybindings.json"))
                && home.exists()
            {
                match crate::tui::keybindings::Keybindings::load(&home) {
                    Ok(custom) => kb.merge(custom),
                    Err(e) => {
                        app.status_text = Some(format!("Failed to load keybindings: {}", e));
                    }
                }
            }
            crate::tui::keybindings::init_keybindings(kb);
            reload_parts.push("keybindings");

            // 4. Skills are reloaded inside rebuild_ext_state below
            // 5. Reload prompt templates from disk (pi-compatible)
            app.prompt_templates =
                crate::agent::prompt_templates::load_prompt_templates(&app.prompt_template_dirs);
            // Only report if there are any template dirs configured
            if !app.prompt_template_dirs.is_empty() {
                reload_parts.push("prompts");
            }

            // 5. Reload context files and rebuild system prompt
            super::overlays::refresh_agent_config(app);
            reload_parts.push("skills");
            {
                let skill_names: Vec<String> = app.skills.iter().map(|s| s.name.clone()).collect();
                let template_names: Vec<String> = app
                    .prompt_templates
                    .iter()
                    .map(|t| t.name.clone())
                    .collect();
                let extension_names: Vec<(String, bool)> = app
                    .extensions
                    .iter()
                    .map(|e| {
                        let enabled =
                            crate::extension::is_extension_enabled(e.as_ref(), &app.settings);
                        (e.name().to_string(), enabled)
                    })
                    .collect();
                let theme_names: Vec<String> = crate::agent::ui::theme::get_available_themes()
                    .into_iter()
                    .filter(|n| n != "dark" && n != "light")
                    .collect();
                app.header.borrow_mut().set_resource_data(
                    app.context_files.clone(),
                    skill_names,
                    template_names,
                    extension_names,
                    theme_names,
                );
            }
            reload_parts.push("system prompt");
            reload_parts.push("context files");

            // 7. Notify extensions that reload is complete (pi-compatible: session_start)
            for ext in app.extensions.iter() {
                ext.on_session_start("reload");
            }

            show_status(app, format!("{} reloaded.", reload_parts.join(", ")));
        }
        CommandResult::NewSession => {
            // Matching pi's handleClearCommand:
            //   1. Stop loading animation
            //   2. Clear status container
            //   3. runtimeHost.newSession() -> session.new_session()
            //   4. renderCurrentSessionState() -> clear everything
            //   5. Add "✓ New session started" with accent color + spacer

            // Stop working indicator (matching pi's loadingAnimation.stop())
            app.working.stop();

            // Clear status section (matching pi's statusContainer.clear())
            app.status_text = None;

            // Create a new session via AgentSession (new ID, new file, resets tracked state)
            if let Some(ref mut agent_session) = app.session {
                agent_session.new_session();

                // Re-record current model/provider and thinking level in the fresh session
                // so the footer can pick them up via refresh_from_session.
                agent_session.on_model_change(&app.current_provider, &app.model);
                if let Some(ref level) = app.thinking_level {
                    agent_session.on_thinking_level_change(level);
                }
            }

            // Clear everything (matching pi's renderCurrentSessionState)
            app.agent = None;
            app.clear_session_state();

            // Refresh footer cached stats from the now-empty session
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }

            // Add "✓ New session started" with accent color, matching pi's
            // `new Text(theme.fg("accent", "✓ New session started"), 1, 1)`
            let styled = app.theme.fg("accent", "✓ New session started");
            chat_add(app, std::boxed::Box::new(Text::new(styled, 1, 1, None)));
        }
        CommandResult::SessionSwitched { path } => {
            let new_session = crate::agent::AgentSession::open(&path, None, Some(&app.cwd));
            app.switch_to_session(new_session);
            app.status_text = Some(format!("Switched to session: {}", path.display()));
        }
        CommandResult::SessionInfo { .. } => {
            // Compute from the live session on demand.
            let info = app
                .session
                .as_ref()
                .map(|s| crate::agent::session::format_session_info(s.session()))
                .unwrap_or_else(|| "No active session".to_string());
            show_status(app, info);
        }
        CommandResult::SessionNamed { name } => {
            // Persist name in session
            if let Some(ref mut s) = app.session {
                s.session_mut().append_session_info(&name);
            }

            // Check if name was normalized (pi-compatible normalization warning)
            let stored_name = app
                .session
                .as_ref()
                .and_then(|s| s.session().session_name().map(|n| n.to_string()));
            if let Some(ref stored) = stored_name
                && stored != &name
            {
                show_status(
                    app,
                    format!("Session name normalized from {:?} to {:?}", name, stored),
                );
            }

            show_status(
                app,
                format!(
                    "Session name set: {}",
                    stored_name.as_deref().unwrap_or(&name)
                ),
            );

            app.status_text = Some(format!(
                "Session name set: {}",
                stored_name.as_deref().unwrap_or(&name)
            ));

            // Refresh footer (refresh_from_session picks up the new name)
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }
        }
        CommandResult::OpenModelSelector => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::OpenSettings => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::OpenExtensions => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::ScopedModels => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::ExportSession { path } => {
            // Get session reference
            let result = (|| -> Result<PathBuf, String> {
                let agent_session = app.session.as_ref().ok_or("No active session")?;
                let session = agent_session.session();
                let system_prompt = Some(app.system_prompt.as_str());
                let theme = crate::agent::ui::theme::current_theme();
                let theme_name = Some(theme.name.as_str());

                let output_path = if path.as_ref().is_some_and(|p| p.ends_with(".jsonl")) {
                    export::export_to_jsonl(session, &app.cwd, path.as_deref())
                        .map_err(|e| format!("Export failed: {}", e))?
                } else {
                    export::export_to_html(
                        session,
                        system_prompt,
                        &app.cwd,
                        path.as_deref(),
                        theme_name,
                    )
                    .map_err(|e| format!("Export failed: {}", e))?
                };

                Ok(output_path)
            })();

            match result {
                Ok(path) => {
                    let display = crate::builtin::shorten_path(path.to_string_lossy().as_ref());
                    show_status(app, format!("✓ Session exported to: {}", display));
                }
                Err(msg) => {
                    show_status(app, format!("✗ {}", msg));
                }
            }
        }
        result @ CommandResult::ImportSession { .. } => {
            // Needs TUI overlay (confirmation) - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::ShareSession => {
            let msg = "Share session - not yet implemented.".to_string();
            show_status(app, msg.clone());
        }
        CommandResult::CopyLastMessage => {
            // Get last assistant message text (pi-compatible)
            let text = app.session.as_ref().and_then(|s| {
                let entries = s.session().get_entries();
                entries.iter().rev().find_map(|entry| {
                    #[allow(clippy::collapsible_if)]
                    if let Some(yoagent::types::Message::Assistant { stop_reason, .. }) =
                        entry.message.as_llm()
                    {
                        if *stop_reason != yoagent::types::StopReason::Aborted
                            || !crate::agent::types::message_text(&AgentMessage::Llm(
                                entry.message.as_llm().unwrap().clone(),
                            ))
                            .trim()
                            .is_empty()
                        {
                            let text = crate::agent::types::message_text(&AgentMessage::Llm(
                                entry.message.as_llm().unwrap().clone(),
                            ));
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                return Some(trimmed.to_string());
                            }
                        }
                    }
                    None
                })
            });

            let text = match text {
                Some(t) => t,
                None => {
                    show_status(app, "No agent messages to copy yet.");
                    return;
                }
            };

            // Pi-compatible clipboard copy (includes OSC 52 fallback)
            copy_to_clipboard(&text);
            show_status(app, "Copied last agent message to clipboard");
        }
        CommandResult::ShowChangelog => {
            let msg = "Changelog - not yet implemented.".to_string();
            show_status(app, msg.clone());
        }
        CommandResult::ForkSession { ref message_id } => {
            if message_id.is_none() {
                // No message ID provided — defer to show message selector overlay
                app.pending_command_result = Some(result);
            } else {
                // Clone the session info before modifying app.session
                let source_path = app
                    .session
                    .as_ref()
                    .and_then(|s| s.session().session_file());
                let session_dir = app.session.as_ref().map(|s| s.session_dir().to_path_buf());
                let cwd = app.cwd.clone();

                match (source_path, session_dir) {
                    (Some(source), Some(ref target_dir)) => {
                        match crate::agent::session::fork_session(
                            source,
                            target_dir,
                            message_id.as_deref(),
                            None,
                        ) {
                            Ok(new_id) => {
                                let dir_entries = std::fs::read_dir(target_dir).ok();
                                let new_path = dir_entries.and_then(|entries| {
                                    entries
                                        .flatten()
                                        .find(|e| {
                                            let filename = e.file_name();
                                            filename.to_string_lossy().contains(&new_id)
                                        })
                                        .map(|e| e.path())
                                });

                                match new_path {
                                    Some(ref path) => {
                                        let new_session = crate::agent::AgentSession::open(
                                            path,
                                            None,
                                            Some(&cwd),
                                        );
                                        app.switch_to_session(new_session);

                                        let styled = app.theme.fg(
                                            "accent",
                                            &format!("✓ Forked session: {}", path.display()),
                                        );
                                        chat_add(
                                            app,
                                            std::boxed::Box::new(Text::new(styled, 1, 1, None)),
                                        );
                                    }
                                    None => {
                                        let msg = format!(
                                            "Fork created but new file not found: {}",
                                            new_id
                                        );
                                        show_status(app, msg);
                                    }
                                }
                            }
                            Err(e) => {
                                let msg = format!("Fork failed: {}", e);
                                show_status(app, msg.clone());
                            }
                        }
                    }
                    _ => {
                        let msg = "No active session to fork".to_string();
                        show_status(app, msg.clone());
                    }
                }
            }
        }
        CommandResult::CloneSession => {
            // Clone the session at the current position (like fork with position "at").
            let source_path = app
                .session
                .as_ref()
                .and_then(|s| s.session().session_file());
            let session_dir = app.session.as_ref().map(|s| s.session_dir().to_path_buf());
            let leaf_id = app.session.as_ref().and_then(|s| s.session().get_leaf_id());
            let cwd = app.cwd.clone();

            let leaf_id = match leaf_id {
                Some(id) if !id.is_empty() => id,
                _ => {
                    let msg = "Nothing to clone yet".to_string();
                    show_status(app, msg);
                    return;
                }
            };

            match (source_path, session_dir) {
                (Some(source), Some(ref target_dir)) => {
                    match crate::agent::session::fork_session(
                        source,
                        target_dir,
                        Some(&leaf_id),
                        Some("at"),
                    ) {
                        Ok(new_id) => {
                            let dir_entries = std::fs::read_dir(target_dir).ok();
                            let new_path = dir_entries.and_then(|entries| {
                                entries
                                    .flatten()
                                    .find(|e| {
                                        let filename = e.file_name();
                                        filename.to_string_lossy().contains(&new_id)
                                    })
                                    .map(|e| e.path())
                            });

                            match new_path {
                                Some(ref path) => {
                                    let new_session =
                                        crate::agent::AgentSession::open(path, None, Some(&cwd));
                                    app.switch_to_session(new_session);

                                    let styled = app.theme.fg(
                                        "accent",
                                        &format!("✓ Cloned session: {}", path.display()),
                                    );
                                    chat_add(
                                        app,
                                        std::boxed::Box::new(Text::new(styled, 1, 1, None)),
                                    );
                                }
                                None => {
                                    let msg =
                                        format!("Clone created but new file not found: {}", new_id);
                                    show_status(app, msg);
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Clone failed: {}", e);
                            show_status(app, msg.clone());
                        }
                    }
                }
                _ => {
                    let msg = "No active session to clone".to_string();
                    show_status(app, msg.clone());
                }
            }
        }
        CommandResult::SessionTree => {
            // Needs TUI overlay — defer
            app.pending_command_result = Some(result);
        }
        CommandResult::TrustDecision { decision } => {
            let msg = format!("Trust decision '{}' saved.", decision);
            show_status(app, msg.clone());
        }
        CommandResult::Login {
            ref provider,
            ref api_key,
        } => {
            if let (Some(provider), Some(key)) = (provider, api_key) {
                super::auth::handle_login(app, provider, Some(key));
            } else {
                // Needs prompt — defer
                app.pending_command_result = Some(result);
            }
        }
        CommandResult::Logout { ref provider } => {
            if let Some(p) = provider {
                super::auth::handle_logout(app, Some(p));
            } else {
                // Needs provider selector — defer
                app.pending_command_result = Some(result);
            }
        }
        CommandResult::CompactSession(custom_instructions) => {
            app.pending_compact = Some(custom_instructions);
        }
        CommandResult::Stop => {
            app.stop_requested
                .store(true, std::sync::atomic::Ordering::Relaxed);
            app.status_text = Some("Stop requested — finishing current turn…".into());
        }
        CommandResult::NextTurn { text } => {
            if text.is_empty() {
                app.status_text = Some("Usage: /nextTurn <message>".into());
            } else {
                app.next_turn_queue.push(super::user_agent_message(&text));
                app.status_text = Some("Message queued for next turn".into());
            }
        }
    }
}

/// Look up a tool renderer by name from extensions (bundled in ToolDefinition.renderer).
pub fn find_tool_renderer(
    extensions: &[Box<dyn crate::extension::Extension>],
    name: &str,
) -> Option<Arc<dyn ToolRenderer>> {
    for ext in extensions {
        for tool in ext.tools() {
            if tool.name() == name {
                return tool.renderer;
            }
        }
    }
    None
}
