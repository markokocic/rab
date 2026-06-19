use super::display::{DisplayMsg, session_messages_to_display, welcome_messages};
use super::editor::{Editor, SlashCommandInfo};
use super::keyboard::{handle_key, parse_bang_command, scroll_down, scroll_up};
use super::render::ui;
use crate::agent::{AgentEvent, LoopConfig, run_agent_loop};
use crate::extension::{AgentTool, CommandResult, Extension, SlashCommand};
use crate::provider::{Provider, ToolDef};
use crate::session::SessionManager;
use crate::theme::{DARK, Theme};
use crate::types::AgentMessage;
use ratatui::crossterm::event::{Event, MouseEventKind};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ── Public types ───────────────────────────────────────────────────

/// Configuration passed to the TUI.
pub struct TuiConfig {
    pub model: String,
    pub system_prompt: String,
    pub tools: Vec<ToolDef>,
    pub agent_tools: Vec<Box<dyn AgentTool>>,
    pub extensions: Vec<Box<dyn Extension>>,
    pub provider: Box<dyn Provider>,
    pub cwd: PathBuf,
    pub thinking_level: Option<String>,
    pub git_branch: Option<String>,
    pub available_models: Vec<String>,
    pub hide_thinking: bool,
    pub collapse_tool_output: bool,
}
// ── Editor creation ────────────────────────────────────────────────

pub(crate) fn create_editor_with(commands: &[SlashCommandInfo], cwd: &std::path::Path) -> Editor {
    let mut editor = Editor::new();
    editor.set_slash_commands(commands.to_vec());
    editor.set_cwd(cwd.to_path_buf());
    editor.set_block(
        Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(0x8a, 0xbe, 0xb7))),
    );
    editor
}

pub(crate) fn create_editor(app: &App) -> Editor {
    create_editor_with(&app.shared.command_infos, &app.cwd)
}
// ── App state ──────────────────────────────────────────────────────

/// Data shared between the TUI main thread and spawned agent tasks.
pub(crate) struct SharedState {
    pub(crate) agent_tools: Vec<Box<dyn AgentTool>>,
    pub(crate) extensions: Vec<Box<dyn Extension>>,
    /// Flattened slash commands from all extensions.
    pub(crate) commands: Vec<SlashCommand>,
    /// Command info for the editor's autocomplete.
    pub(crate) command_infos: Vec<SlashCommandInfo>,
}

pub(crate) struct App {
    pub(crate) cwd: PathBuf,
    pub(crate) model: String,
    pub(crate) thinking_level: Option<String>,
    pub(crate) git_branch: Option<String>,
    pub(crate) system_prompt: String,
    pub(crate) shared: Arc<SharedState>,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) theme: Theme,

    /// Conversation history (AgentMessage, not DisplayMsg)
    pub(crate) conversation: Vec<AgentMessage>,

    /// Rendered display messages
    pub(crate) messages: Vec<DisplayMsg>,
    /// Scroll state: top line index (from top of content). Managed via Cell for render access.
    pub(crate) scroll_offset: Cell<usize>,
    /// When true, new messages auto-scroll to bottom.
    pub(crate) auto_scroll: Cell<bool>,

    pub(crate) editor: Editor,
    pub(crate) event_tx: mpsc::UnboundedSender<AgentEvent>,
    pub(crate) event_rx: mpsc::UnboundedReceiver<AgentEvent>,

    pub(crate) is_streaming: bool,
    pub(crate) pending_text: Option<String>,
    pub(crate) pending_thinking: Option<String>,

    pub(crate) hide_thinking: bool,
    /// Tool output collapsed by default (matches pi). Ctrl+O to expand.
    pub(crate) tool_output_collapsed: bool,
    pub(crate) show_help: bool,

    pub(crate) should_quit: bool,
    pub(crate) last_usage: Option<crate::types::Usage>,

    /// Handle to abort the running agent task (for Ctrl+C interrupt).
    pub(crate) agent_abort: Option<tokio::task::AbortHandle>,

    /// History: index into conversation user messages for arrow-key recall.
    /// None = not navigating history; Some(i) = pointing at conversation[i].
    pub(crate) history_index: Option<usize>,
    /// Session for persistence (wrapped in Option for ownership).
    pub(crate) session: Option<SessionManager>,

    /// Frame counter for spinner animation.
    pub(crate) frame_count: u64,

    // ── Model selector state ──
    pub(crate) available_models: Vec<String>,
    pub(crate) show_model_selector: bool,
    pub(crate) model_search: String,
    pub(crate) model_selector_selection: usize,
}
pub async fn run(config: TuiConfig, session: SessionManager) -> anyhow::Result<()> {
    ratatui::crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    ratatui::crossterm::execute!(
        stdout,
        ratatui::crossterm::terminal::EnterAlternateScreen,
        ratatui::crossterm::cursor::Show,
        ratatui::crossterm::cursor::SetCursorStyle::BlinkingBlock,
        ratatui::crossterm::event::EnableBracketedPaste,
        ratatui::crossterm::event::EnableMouseCapture,
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let result = run_app(&mut terminal, config, session);

    ratatui::crossterm::terminal::disable_raw_mode()?;
    ratatui::crossterm::execute!(
        terminal.backend_mut(),
        ratatui::crossterm::terminal::LeaveAlternateScreen,
        ratatui::crossterm::event::DisableBracketedPaste,
        ratatui::crossterm::event::DisableMouseCapture,
    )?;
    result
}

// ── Main event loop ────────────────────────────────────────────────

fn run_app(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    config: TuiConfig,
    session: SessionManager,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Load session history and convert to display messages
    let context = session.build_session_context();
    let history_messages = context.messages.clone();
    let history_display = session_messages_to_display(&history_messages);

    let welcome = welcome_messages(&config);

    // Collect slash commands from all extensions
    let commands: Vec<SlashCommand> = config
        .extensions
        .iter()
        .flat_map(|e| e.commands())
        .collect();
    let command_infos: Vec<SlashCommandInfo> = commands
        .iter()
        .map(|c| SlashCommandInfo {
            name: c.name.clone(),
            description: c.description.clone(),
        })
        .collect();

    let shared = Arc::new(SharedState {
        agent_tools: config.agent_tools,
        extensions: config.extensions,
        commands,
        command_infos: command_infos.clone(),
    });

    let mut app = App {
        cwd: config.cwd.clone(),
        model: config.model.clone(),
        thinking_level: config.thinking_level.clone(),
        git_branch: config.git_branch.clone(),
        system_prompt: config.system_prompt,
        shared,
        provider: Arc::from(config.provider),
        theme: DARK,
        conversation: history_messages,
        messages: {
            let mut all = history_display;
            all.extend(welcome);
            all
        },
        scroll_offset: Cell::new(0),
        auto_scroll: Cell::new(true),
        editor: create_editor_with(&command_infos, &config.cwd),
        event_tx: tx,
        event_rx: rx,
        is_streaming: false,
        pending_text: None,
        pending_thinking: None,
        hide_thinking: config.hide_thinking,
        tool_output_collapsed: config.collapse_tool_output,
        show_help: false,
        should_quit: false,
        last_usage: None,
        agent_abort: None,
        history_index: None,
        session: Some(session),
        frame_count: 0,
        available_models: config.available_models,
        show_model_selector: false,
        model_search: String::new(),
        model_selector_selection: 0,
    };

    loop {
        app.frame_count = app.frame_count.wrapping_add(1);
        terminal.draw(|f| ui(f, &app))?;

        // Poll for keyboard events
        if ratatui::crossterm::event::poll(Duration::from_millis(10))? {
            match ratatui::crossterm::event::read()? {
                Event::Key(key) => {
                    handle_key(&mut app, key);
                }
                Event::Paste(data) => {
                    app.editor.handle_paste(&data);
                }
                Event::Resize(..) => {}
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        scroll_up(&mut app, 3);
                    }
                    MouseEventKind::ScrollDown => {
                        scroll_down(&mut app, 3);
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        // Drain agent events from the channel
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
pub(crate) fn submit_message(app: &mut App, message: String) {
    app.history_index = None;
    let trimmed = message.trim();

    // !command — execute bash inline (pi-style)
    // !!command — execute bash, excluded from agent context
    if let Some((command, exclude_from_context)) = parse_bang_command(trimmed) {
        let cwd = app.cwd.clone();
        let cmd = command.to_string();
        let tx = app.event_tx.clone();

        // Show the command as a user message
        let label = if exclude_from_context { "!!" } else { "!" };
        app.messages
            .push(DisplayMsg::User(format!("{label} {command}")));
        app.editor = create_editor(app);
        app.auto_scroll.set(true);

        app.is_streaming = true;
        app.pending_text = None;
        app.pending_thinking = None;

        let handle = tokio::spawn(async move {
            // We use tokio::process::Command directly (same as bash tool)
            let started = std::time::Instant::now();
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .current_dir(&cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;

            let elapsed = started.elapsed();

            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let combined = format!("{}{}", stdout, stderr);

                    let mut result = combined.clone();
                    let total_lines = result.lines().count();
                    const MAX_LINES: usize = 2000;
                    const MAX_BYTES: usize = 50 * 1024;

                    if result.len() > MAX_BYTES {
                        let byte_start = result.len().saturating_sub(MAX_BYTES);
                        result = if let Some(newline_pos) = result[byte_start..].find('\n') {
                            result[(byte_start + newline_pos + 1)..].to_string()
                        } else {
                            result[byte_start..].to_string()
                        };
                    }

                    let lines: Vec<&str> = result.lines().collect();
                    let shown_lines = lines.len();
                    if lines.len() > MAX_LINES {
                        let start = lines.len() - MAX_LINES;
                        result = lines[start..].join("\n");
                    }

                    if total_lines > shown_lines || combined.len() > MAX_BYTES {
                        let start_line = total_lines - shown_lines + 1;
                        result.push_str(&format!(
                            "\n\n[Showing lines {}-{} of {}.]",
                            start_line, total_lines, total_lines,
                        ));
                    }

                    if !output.status.success() {
                        let exit_code = output.status.code().unwrap_or(-1);
                        if !result.is_empty() {
                            result
                                .push_str(&format!("\n\n[Command exited with code {}]", exit_code));
                        } else {
                            result = format!("Command exited with code {}", exit_code);
                        }
                    } else if result.is_empty() {
                        result = "(no output)".into();
                    }

                    let _ = tx.send(AgentEvent::TurnStart);
                    // Emit tool call display
                    let _ = tx.send(AgentEvent::ToolCall {
                        id: String::new(),
                        name: "bash".into(),
                        args: serde_json::json!({}),
                    });
                    // Emit the output
                    let _ = tx.send(AgentEvent::ToolResult {
                        id: String::new(),
                        name: "bash".into(),
                        content: format!(
                            "$ {}\n\n{}\n\n[{}s]",
                            cmd,
                            result.trim(),
                            elapsed.as_secs_f64()
                        ),
                        is_error: !output.status.success(),
                    });
                    let _ = tx.send(AgentEvent::TurnEnd);
                    let _ = tx.send(AgentEvent::AgentEnd { messages: vec![] });
                }
                Err(e) => {
                    let _ = tx.send(AgentEvent::ToolResult {
                        id: String::new(),
                        name: "bash".into(),
                        content: format!("Failed to execute: {:#}", e),
                        is_error: true,
                    });
                    let _ = tx.send(AgentEvent::AgentEnd { messages: vec![] });
                }
            }
        });
        app.agent_abort = Some(handle.abort_handle());
        return;
    }

    if trimmed.starts_with('/') {
        let (cmd_name, args) = match trimmed.split_once(' ') {
            Some((cmd, rest)) => (cmd.trim_start_matches('/'), rest),
            None => (trimmed.trim_start_matches('/'), ""),
        };

        // First collect all commands into owned values to release the borrow on app
        // before we potentially mutate app for error messages or model selector.
        let cmds: Vec<(String, String)> = collect_commands(app)
            .into_iter()
            .map(|c| (c.name.clone(), c.description.clone()))
            .collect();

        // Resolve the command name: exact match first, then unique prefix
        let resolved_name: Option<String> = {
            if let Some(cmd) = cmds.iter().find(|(n, _)| n == cmd_name) {
                Some(cmd.0.clone())
            } else {
                let lower = cmd_name.to_lowercase();
                let prefix_matches: Vec<&(String, String)> = cmds
                    .iter()
                    .filter(|(n, _)| n.to_lowercase().starts_with(&lower))
                    .collect();
                if prefix_matches.len() == 1 {
                    Some(prefix_matches[0].0.clone())
                } else if prefix_matches.len() > 1 {
                    let names: Vec<String> =
                        prefix_matches.iter().map(|c| format!("/{}", c.0)).collect();
                    app.messages.push(DisplayMsg::Info(format!(
                        "Did you mean: {}?",
                        names.join(", ")
                    )));
                    app.editor = create_editor(app);
                    None
                } else {
                    app.messages.push(DisplayMsg::Info(format!(
                        "Unknown command: /{}. Type / for available commands.",
                        cmd_name
                    )));
                    app.editor = create_editor(app);
                    None
                }
            }
        };

        if let Some(ref name) = resolved_name {
            // /model (or prefix match to model) without args opens the model selector
            if name == "model" && args.is_empty() {
                app.show_model_selector = true;
                app.model_search.clear();
                app.model_selector_selection = app
                    .available_models
                    .iter()
                    .position(|m| m == &app.model || format!("opencode_go::{m}") == app.model)
                    .unwrap_or(0);
                app.editor = create_editor(app);
                return;
            }
            // Execute the command via handler
            let commands = collect_commands(app);
            if let Some(cmd) = commands.iter().find(|c| c.name == name.as_str()) {
                let result = cmd.handler.execute(args);
                apply_command_result(app, result);
                return;
            }
        }
        return;
    }

    let provider = Arc::clone(&app.provider);
    let shared = Arc::clone(&app.shared);
    let model = app.model.clone();
    let system_prompt = app.system_prompt.clone();
    let tools = collect_tool_defs_from_shared(&shared);
    let tx = app.event_tx.clone();
    let history = app.conversation.clone();

    app.messages.push(DisplayMsg::User(trimmed.to_string()));
    app.auto_scroll.set(true);

    let prompt = AgentMessage::user(trimmed);
    app.conversation.push(prompt.clone());

    app.editor = create_editor(app);
    app.is_streaming = true;
    app.pending_text = None;
    app.pending_thinking = None;

    let handle = tokio::spawn(async move {
        let loop_config = LoopConfig {
            model,
            system_prompt,
            tools,
            agent_tools: &shared.agent_tools,
            extensions: &shared.extensions,
        };

        let mut emit = |event: AgentEvent| {
            let _ = tx.send(event);
        };

        let _ = run_agent_loop(vec![prompt], history, &loop_config, &*provider, &mut emit).await;
    });
    app.agent_abort = Some(handle.abort_handle());
}
pub(crate) fn apply_command_result(app: &mut App, result: anyhow::Result<CommandResult>) {
    match result {
        Ok(CommandResult::Info(text)) => {
            app.messages.push(DisplayMsg::Info(text));
        }
        Ok(CommandResult::Quit) => {
            app.messages
                .push(DisplayMsg::Info("/quit — exiting".to_string()));
            app.editor = create_editor(app);
            app.should_quit = true;
            return;
        }
        Ok(CommandResult::ModelChanged(new_model)) => {
            app.model = new_model.clone();
            if let Ok(mut settings) = crate::settings::Settings::load(&app.cwd) {
                settings.default_model = Some(new_model.clone());
                let _ = settings.save();
            }
            app.messages.push(DisplayMsg::Info(format!(
                "Model: {}",
                new_model.replace("opencode_go::", "")
            )));
        }
        Ok(CommandResult::ShowHelp) => {
            app.show_help = true;
            app.editor = create_editor(app);
            return;
        }
        Ok(CommandResult::Reloaded) => {
            let mut changes = Vec::new();
            if let Ok(settings) = crate::settings::Settings::load(&app.cwd) {
                if let Some(model) = &settings.default_model
                    && *model != app.model
                {
                    app.model = model.clone();
                    changes.push(format!("model: {}", model));
                }
                if let Some(level) = &settings.default_thinking_level
                    && Some(level) != app.thinking_level.as_ref()
                {
                    app.thinking_level = Some(level.clone());
                    changes.push(format!("thinking: {}", level));
                }
            }
            if let Ok(auth) = crate::auth::AuthStorage::load()
                && auth.api_key("opencode-go").is_some()
            {
                changes.push("auth: reloaded".into());
            }
            if changes.is_empty() {
                app.messages.push(DisplayMsg::Info(
                    "/reload — no changes detected".to_string(),
                ));
            } else {
                app.messages.push(DisplayMsg::Info(format!(
                    "/reload — {}",
                    changes.join(", ")
                )));
            }
        }
        Ok(CommandResult::NewSession) => {
            app.conversation.clear();
            app.messages.clear();
            app.last_usage = None;
            app.messages
                .push(DisplayMsg::Info("/new — session cleared".to_string()));
        }
        Ok(CommandResult::SessionSwitched { path }) => {
            app.messages.push(DisplayMsg::Info(format!(
                "Switched to session: {}",
                path.display()
            )));
        }
        Ok(CommandResult::SessionInfo {
            session_id,
            file_path,
            name,
            message_count,
        }) => {
            let name_str = name.as_deref().unwrap_or("(unnamed)");
            let file_str = file_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(in-memory)".to_string());
            app.messages.push(DisplayMsg::Info(format!(
                "Session: {}\nName:    {}\nFile:    {}\nMessages: {}",
                session_id, name_str, file_str, message_count
            )));
        }
        Ok(CommandResult::OpenSessionSelector) => {
            app.messages.push(DisplayMsg::Info(
                "/resume — session selector not yet implemented".to_string(),
            ));
        }
        Ok(CommandResult::SessionNamed { name }) => {
            app.messages
                .push(DisplayMsg::Info(format!("Session named: {}", name)));
        }
        Err(e) => {
            app.messages
                .push(DisplayMsg::Info(format!("Command error: {:#}", e)));
        }
    }
    app.editor = create_editor(app);
}

pub(crate) fn collect_tool_defs_from_shared(shared: &SharedState) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    for tool in &shared.agent_tools {
        if !defs.iter().any(|d: &ToolDef| d.name == tool.name()) {
            defs.push(ToolDef {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters(),
            });
        }
    }
    defs
}

/// Collect slash commands from shared state (flattened, deduplicated by name).
pub(crate) fn collect_commands(app: &App) -> Vec<&SlashCommand> {
    let mut seen = std::collections::HashSet::new();
    let mut cmds: Vec<&SlashCommand> = Vec::new();
    for cmd in &app.shared.commands {
        if seen.insert(&cmd.name) {
            cmds.push(cmd);
        }
    }
    cmds
}
pub(crate) fn handle_agent_event(app: &mut App, event: AgentEvent) {
    match event {
        AgentEvent::AgentStart => {
            app.is_streaming = true;
            app.pending_text = None;
            app.pending_thinking = None;
        }
        AgentEvent::TurnStart => {}
        AgentEvent::TextDelta { delta } => {
            if let Some(ref mut text) = app.pending_text {
                text.push_str(&delta);
            } else {
                flush_thinking(app);
                app.pending_text = Some(delta);
            }
        }
        AgentEvent::ThinkingDelta { delta } => {
            if let Some(ref mut text) = app.pending_thinking {
                text.push_str(&delta);
            } else {
                flush_text(app);
                app.pending_thinking = Some(delta);
            }
        }
        AgentEvent::ToolCall {
            ref name, ref args, ..
        } => {
            flush_all(app);
            app.messages.push(DisplayMsg::ToolCall {
                name: name.clone(),
                args: serde_json::to_string(args).unwrap_or_default(),
            });
        }
        AgentEvent::ToolResult {
            ref content,
            is_error,
            ..
        } => {
            app.messages.push(DisplayMsg::ToolResult {
                content: content.clone(),
                is_error,
            });
        }
        AgentEvent::TurnEnd => {
            flush_all(app);
        }
        AgentEvent::AgentEnd { ref messages } => {
            flush_all(app);
            app.is_streaming = false;
            app.agent_abort = None;
            // Persist new messages to session
            if let Some(ref mut session) = app.session {
                for msg in messages {
                    session.append_message(msg);
                }
            }
            if let Some(last) = messages.iter().rev().find(|m| m.usage.is_some()) {
                app.last_usage = last.usage.clone();
            }
        }
    }
}

pub(crate) fn flush_text(app: &mut App) {
    if let Some(text) = app.pending_text.take()
        && !text.is_empty()
    {
        app.messages.push(DisplayMsg::AssistantText(text));
    }
}

pub(crate) fn flush_thinking(app: &mut App) {
    if let Some(text) = app.pending_thinking.take()
        && !text.is_empty()
    {
        app.messages.push(DisplayMsg::Thinking(text));
    }
}

pub(crate) fn flush_all(app: &mut App) {
    flush_text(app);
    flush_thinking(app);
}
