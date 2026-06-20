use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::agent::extension::{AgentTool, Extension};
use crate::agent::provider::{Provider, ToolDef};
use crate::agent::session::SessionManager;
use crate::agent::types::{AgentMessage, Usage};
use crate::agent::ui::chat_editor::ChatEditor;
use crate::agent::ui::footer::Footer;
use crate::agent::ui::help::HelpOverlay;
use crate::agent::ui::messages::{DisplayMsg, render_messages, session_messages_to_display};
use crate::agent::ui::model_selector::ModelSelector;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::working::WorkingIndicator;
use crate::agent::{AgentEvent, LoopConfig, run_agent_loop};
use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::keys::{Key, matches_key};
use crate::tui::screen::Screen;
use crate::tui::terminal::{self, Terminal};
use crossterm::event::{KeyCode, KeyEvent};
use tokio::sync::mpsc;

/// Configuration for the UI app.
pub struct AppConfig {
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
    pub interactive: bool,
    pub settings: crate::agent::settings::Settings,
    /// Context files (AGENTS.md / CLAUDE.md) loaded for the session.
    pub context_files: Vec<String>,
    /// Skills loaded for the session (used for /skill:name expansion).
    pub skills: Vec<crate::agent::Skill>,
}

/// Main application state.
pub struct App {
    cwd: PathBuf,
    model: String,
    #[allow(dead_code)]
    thinking_level: Option<String>,
    system_prompt: String,
    provider: Arc<dyn Provider>,
    theme: RabTheme,

    /// Slash commands from all extensions.
    #[allow(dead_code)]
    commands: Vec<(String, String)>,

    /// Available models for the model selector.
    available_models: Vec<String>,

    /// Conversation history (AgentMessage).
    conversation: Vec<AgentMessage>,

    /// Rendered display messages.
    messages: Vec<DisplayMsg>,

    /// The chat editor.
    editor: ChatEditor,

    /// Agent event channel.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,

    /// Streaming state.
    is_streaming: bool,
    pending_text: Option<String>,
    pending_thinking: Option<String>,

    /// Display settings.
    hide_thinking: bool,
    collapse_tool_output: bool,

    /// Overlay states.
    show_help: bool,
    help_overlay: HelpOverlay,
    show_model_selector: bool,
    model_selector: Option<ModelSelector>,

    /// Exit flag.
    should_quit: bool,

    /// Token usage from last response.
    last_usage: Option<Usage>,

    /// Agent abort handle for Ctrl+C.
    agent_abort: Option<tokio::task::AbortHandle>,

    /// History navigation.
    history_index: Option<usize>,

    /// Session persistence.
    session: Option<SessionManager>,

    /// Footer.
    footer: Footer,

    /// Working indicator.
    working: WorkingIndicator,

    /// Transient status text (pi-style: replaces previous status, not added to chat).
    status_text: Option<String>,

    /// Agent tools (for tool execution).
    agent_tools: Arc<Vec<Box<dyn AgentTool>>>,
    /// Extensions.
    extensions: Arc<Vec<Box<dyn Extension>>>,

    /// Messages queued while streaming — submitted when current response finishes.
    queued_messages: Vec<String>,

    /// Skills loaded for the session (/skill:name expansion).
    skills: Vec<crate::agent::Skill>,

    /// Settings reference for persisting toggle changes.
    settings: crate::agent::settings::Settings,
}

impl App {
    fn new(config: AppConfig, session: SessionManager) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let theme = RabTheme;

        let mut editor = ChatEditor::new(&theme, config.cwd.clone());

        // Collect slash commands
        let commands: Vec<(String, String)> = config
            .extensions
            .iter()
            .flat_map(|e| e.commands())
            .map(|c| (c.name, c.description))
            .collect();
        editor.set_slash_commands(commands.iter().map(|(n, _)| n.clone()).collect());

        let mut footer = Footer::new(config.cwd.to_string_lossy().to_string());
        footer.set_git_branch(config.git_branch.clone());
        footer.set_model(&config.model);

        let mut help_overlay = HelpOverlay::new(&theme);
        help_overlay.set_commands(commands.clone());

        // Load session messages
        let context = session.build_session_context();
        let history_messages = context.messages.clone();
        let history_display = session_messages_to_display(&history_messages);

        // Startup info: context files, skills, tools (pi-style loaded resources listing)
        let mut startup_info: Vec<DisplayMsg> = Vec::new();

        let mut resource_parts: Vec<String> = Vec::new();

        if !config.context_files.is_empty() {
            let ctx = config.context_files.join(", ");
            resource_parts.push(format!("Context: {}", ctx));
        }

        if !config.skills.is_empty() {
            let skill_names: Vec<&str> = config.skills.iter().map(|s| s.name.as_str()).collect();
            resource_parts.push(format!("Skills: {}", skill_names.join(", ")));
        }

        if !resource_parts.is_empty() {
            startup_info.push(DisplayMsg::Info(resource_parts.join("  ·  ")));
        }

        // Combine startup info with history
        let messages = if startup_info.is_empty() {
            history_display
        } else {
            let mut combined = startup_info;
            combined.push(DisplayMsg::Separator);
            combined.extend(history_display);
            combined
        };

        Self {
            cwd: config.cwd,
            model: config.model,
            thinking_level: config.thinking_level,
            system_prompt: config.system_prompt,
            provider: Arc::from(config.provider),
            theme,
            commands,
            available_models: config.available_models,
            conversation: history_messages,
            messages,
            editor,
            event_tx: tx,
            event_rx: rx,
            is_streaming: false,
            pending_text: None,
            pending_thinking: None,
            hide_thinking: config.hide_thinking,
            collapse_tool_output: config.collapse_tool_output,
            show_help: false,
            help_overlay,
            show_model_selector: false,
            model_selector: None,
            should_quit: false,
            last_usage: None,
            agent_abort: None,
            history_index: None,
            session: Some(session),
            footer,
            working: WorkingIndicator::new(),
            agent_tools: Arc::new(config.agent_tools),
            extensions: Arc::new(config.extensions),
            queued_messages: Vec::new(),
            skills: config.skills,
            settings: config.settings,
            status_text: None,
        }
    }
}

/// Run the interactive UI.
pub async fn run(config: AppConfig, session: SessionManager) -> anyhow::Result<()> {
    let mut term = Terminal::new();
    term.enter_raw_mode()?;
    let mut stdout = std::io::stdout();

    // Main-screen mode (like pi) — no alternate screen, no clear.
    // Content writes from current cursor position (after shell prompt).
    // Terminal scrolls naturally, editor/footer appear at the bottom.
    Terminal::hide_cursor(&mut stdout)?;

    let mut screen = Screen::new();
    let mut app = App::new(config, session);

    loop {
        // Poll for events first (pi-style: process input before rendering)
        // Short timeout keeps typing responsive; 8ms ≈ 120fps polling rate.
        let timeout = if app.is_streaming || app.working.active {
            Duration::from_millis(8)
        } else {
            Duration::from_millis(16)
        };

        if let Some(key) = terminal::poll_key_event(Some(timeout))? {
            handle_input(&mut app, &key);
        }

        // Drain agent events
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
        }

        // Get terminal size
        let (cols, rows) = Terminal::size()?;

        // Compose UI after all pending input/events processed
        // (single render per state change, no wasted before-input frames)
        let lines = compose_ui(&mut app, cols as usize, rows as usize);

        // Render to screen
        screen.render(lines, cols, rows, &mut stdout)?;

        // Pi: clear transient status after rendering
        app.status_text = None;

        // Tick the working indicator
        app.working.tick();

        if app.should_quit {
            break;
        }
    }

    // Cleanup — move cursor past all rendered content so the shell prompt
    // appears on a fresh line after the footer (matching pi's stop() behavior).
    screen.finalize(&mut stdout)?;
    Terminal::show_cursor(&mut stdout)?;
    stdout.flush()?;
    term.leave_raw_mode()?;

    Ok(())
}

/// Compose the full UI from app state — matching pi's main screen layout.
///
/// Layout (top to bottom):
///   header → messages → spacer → status → editor → footer
fn compose_ui(app: &mut App, width: usize, _height: usize) -> Vec<String> {
    let mut lines = Vec::new();

    if app.show_help {
        lines.extend(app.help_overlay.render(width));
        return lines;
    }

    if app.show_model_selector {
        if let Some(ref ms) = app.model_selector {
            lines.extend(ms.render(width));
        }
        return lines;
    }

    // ── Header (pi-style: logo + keybinding hints at top) ──
    let header = format!(
        "{} {}",
        app.theme.bold(&app.theme.fg("accent", "rab")),
        app.theme.fg(
            "dim",
            &format!("· model {}", app.model.replace("opencode_go::", ""))
        )
    );
    lines.push(header);

    let hints = app.theme.fg(
        "dim",
        "Enter submit · Ctrl+J · Esc clear · Ctrl+C · Ctrl+D · ↑↓ · F1 help · Ctrl+L model",
    );
    lines.push(format!(" {}", hints));
    lines.push(String::new());

    // ── Messages ──
    let rendered = render_messages(
        &app.messages,
        width,
        app.hide_thinking,
        app.collapse_tool_output,
        &app.theme,
    );
    lines.extend(rendered);

    // ── Pending (streaming) text ──
    if let Some(ref text) = app.pending_text
        && !text.is_empty()
    {
        let inner = width.saturating_sub(2);
        for line in text.lines() {
            if line.is_empty() {
                lines.push(String::new());
            } else {
                let wrapped = crate::tui::util::wrap_text_with_ansi(line, inner);
                for w in wrapped {
                    let line = format!(" {}", w);
                    lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
                }
            }
        }
    }
    if let Some(ref text) = app.pending_thinking
        && !text.is_empty()
    {
        if app.hide_thinking {
            let content = format!(" {}", app.theme.fg("thinking_text", " Thinking…"));
            let padded = crate::agent::ui::messages::pad_to_width(&content, width);
            lines.push(app.theme.bg("thinking_bg", &padded));
        } else {
            for line in text.lines() {
                let content = format!(" {}", app.theme.fg("thinking_text", line));
                let padded = crate::agent::ui::messages::pad_to_width(&content, width);
                lines.push(app.theme.bg("thinking_bg", &padded));
            }
        }
    }

    // ── Transient status text (pi-style: replaces previous status, not added to chat) ──
    if let Some(ref status) = app.status_text {
        let line = app.theme.fg("dim", &format!(" {}", status));
        lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
    }

    // ── Queued messages (pi-style: shown between chat and editor) ──
    if !app.queued_messages.is_empty() {
        for msg in &app.queued_messages {
            let line = app.theme.fg("dim", &format!(" ◷ {}", msg));
            lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
        }
        let hint = app
            .theme
            .fg("dim", " ↳ queued — will send when current finishes");
        lines.push(crate::agent::ui::messages::pad_to_width(&hint, width));
    }

    // ── Spacer before editor (pi inserts blank line between messages and editor) ──
    if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
        lines.push(String::new());
    }

    // ── Working indicator (always rendered — empty line when inactive, keeps line count stable) ──
    lines.extend(app.working.render(width));

    // ── Editor ──
    lines.extend(app.editor.editor.render(width));

    // ── Footer ──
    lines.extend(app.footer.render(width));

    lines
}

/// Handle keyboard input.
fn handle_input(app: &mut App, key: &KeyEvent) {
    // Global keys
    if matches_key(key, &Key::Ctrl('d')) && app.editor.editor.get_text().is_empty() {
        app.should_quit = true;
        return;
    }

    // Overlay handling
    if app.show_help {
        app.show_help = false;
        return;
    }

    if app.show_model_selector {
        if let Some(ref mut ms) = app.model_selector {
            ms.handle_input(key);
            if let Some(ref model) = ms.selected_model {
                app.model = model.clone();
                app.footer.set_model(model);
                app.status_text = Some(format!("Model: {}", model.replace("opencode_go::", "")));
            }
            app.show_model_selector = false;
            app.model_selector = None;
        }
        return;
    }

    // Normal mode keys
    if matches_key(key, &Key::Ctrl('c')) {
        // Interrupt streaming
        if app.is_streaming {
            if let Some(handle) = app.agent_abort.take() {
                handle.abort();
            }
            app.is_streaming = false;
            app.working.stop();
            app.footer.set_streaming(false);

            // Restore queued messages to editor (pi-style)
            if !app.queued_messages.is_empty() {
                let queued = app.queued_messages.join("\n\n");
                app.editor.editor.set_text(&queued);
                app.queued_messages.clear();
            }

            app.status_text = Some("Interrupted".into());
        } else {
            // Clear editor
            app.editor.editor.set_text("");
        }
        return;
    }

    if matches_key(key, &Key::Escape) {
        // Pi-style: close autocomplete first if active
        if app.editor.editor.autocomplete_active {
            app.editor.editor.clear_autocomplete();
            return;
        }
        // Pi-style: Escape aborts current operation
        if app.is_streaming {
            // Abort agent task (same as Ctrl+C)
            if let Some(handle) = app.agent_abort.take() {
                handle.abort();
            }
            app.is_streaming = false;
            app.working.stop();
            app.footer.set_streaming(false);

            // Restore queued messages to editor
            if !app.queued_messages.is_empty() {
                let queued = app.queued_messages.join("\n\n");
                app.editor.editor.set_text(&queued);
                app.queued_messages.clear();
            }

            app.status_text = Some("Interrupted".into());
        } else {
            app.editor.editor.set_text("");
            app.history_index = None;
        }
        return;
    }

    if matches_key(key, &Key::Ctrl('l')) {
        // Open model selector
        let models = app.available_models.clone();
        let current = app.model.clone();
        app.model_selector = Some(ModelSelector::new(models, &current, &app.theme));
        app.show_model_selector = true;
        return;
    }

    if matches_key(key, &Key::Ctrl('t')) {
        app.hide_thinking = !app.hide_thinking;
        // Persist to settings (pi-style: settings survive restart)
        app.settings.hide_thinking = Some(app.hide_thinking);
        if let Err(e) = app.settings.save() {
            app.messages.push(DisplayMsg::Info(format!(
                "Failed to save thinking setting: {}",
                e
            )));
        }
        app.messages.push(DisplayMsg::Info(if app.hide_thinking {
            "Thinking blocks: hidden".into()
        } else {
            "Thinking blocks: visible".into()
        }));
        return;
    }

    if matches_key(key, &Key::Ctrl('o')) {
        app.collapse_tool_output = !app.collapse_tool_output;
        // Persist to settings (pi-style: setting survives restart)
        app.settings.collapse_tool_output = Some(app.collapse_tool_output);
        if let Err(e) = app.settings.save() {
            app.messages.push(DisplayMsg::Info(format!(
                "Failed to save tool output setting: {}",
                e
            )));
        }
        app.messages
            .push(DisplayMsg::Info(if app.collapse_tool_output {
                "Tool output: collapsed".into()
            } else {
                "Tool output: expanded".into()
            }));
        return;
    }

    // Tab = autocomplete slash command (pi-style)
    // Tab = trigger or navigate autocomplete (pi-style)
    if matches_key(key, &Key::Tab) && !app.editor.editor.autocomplete_active {
        let text = app.editor.editor.get_text();
        if text.starts_with('/') {
            let suggestions = app.editor.get_autocomplete_suggestions();
            app.editor.editor.set_autocomplete(suggestions);
        }
        return;
    }

    // F1 = help
    if key.code == KeyCode::F(1) {
        app.show_help = true;
        return;
    }

    // Ctrl+J = literal newline in editor
    if matches_key(key, &Key::Ctrl('j')) {
        app.editor.editor.insert_text_at_cursor("\n");
        return;
    }

    // Enter = submit
    if matches_key(key, &Key::Enter) {
        let text = app.editor.editor.get_text();
        if !text.trim().is_empty() {
            app.editor.editor.add_to_history(&text);
            submit_message(app, text);
        }
        app.editor.editor.set_text("");
        return;
    }

    // Up/Down for history (pi: not when autocomplete is active)
    if !app.editor.editor.autocomplete_active {
        if matches_key(key, &Key::Up) && app.editor.editor.get_text().is_empty() {
            recall_history(app, -1);
            return;
        }

        if matches_key(key, &Key::Down) && app.editor.editor.get_text().is_empty() {
            recall_history(app, 1);
            return;
        }
    }

    // PageUp/Down for scroll
    if matches_key(key, &Key::PageUp) {
        return;
    }
    if matches_key(key, &Key::PageDown) {
        return;
    }

    // Delegate to editor
    app.editor.editor.handle_input(key);
}

/// Submit or queue a user message. When streaming, queues instead of spawning
/// a concurrent agent loop (matching pi's behavior).
fn submit_message(app: &mut App, message: String) {
    app.history_index = None;
    let trimmed = message.trim().to_string();

    // Handle /skill:name [args] expansion (pi-style: before command dispatch)
    if trimmed.starts_with("/skill:") {
        let expanded = crate::agent::skills::expand_skill_command(&trimmed, &app.skills);
        app.messages.push(DisplayMsg::User(expanded.clone()));
        if app.is_streaming {
            app.queued_messages.push(expanded);
            return;
        }
        start_agent_loop(app, expanded);
        return;
    }

    // Handle /commands
    if trimmed.starts_with('/') {
        handle_slash_command(app, &trimmed);
        return;
    }

    // Handle ! and !! bang commands
    if let Some((cmd, _exclude)) = parse_bang_command(&trimmed) {
        handle_bang_command(app, cmd);
        return;
    }

    // Normal message submission to LLM
    app.messages.push(DisplayMsg::User(trimmed.clone()));

    if app.is_streaming {
        // Queue — will be submitted when current response finishes (pi-style)
        app.queued_messages.push(trimmed);
        return;
    }

    start_agent_loop(app, trimmed);
}

/// Actually start an agent loop (not queued).
fn start_agent_loop(app: &mut App, message: String) {
    let provider = Arc::clone(&app.provider);
    let model = app.model.clone();
    let system_prompt = app.system_prompt.clone();
    let tools = collect_tool_defs(app);
    let tx = app.event_tx.clone();
    let history = app.conversation.clone();
    let agent_tools = Arc::clone(&app.agent_tools);
    let extensions = Arc::clone(&app.extensions);

    app.is_streaming = true;
    app.working.start();
    app.footer.set_streaming(true);
    app.pending_text = None;
    app.pending_thinking = None;

    let handle = tokio::spawn(async move {
        let config = LoopConfig {
            model: model.clone(),
            system_prompt,
            tools,
            agent_tools: &agent_tools,
            extensions: &extensions,
        };

        let mut emit = |event: AgentEvent| {
            let _ = tx.send(event);
        };

        let prompt = AgentMessage::user(message);
        let _ = run_agent_loop(vec![prompt], history, &config, &*provider, &mut emit).await;
    });
    app.agent_abort = Some(handle.abort_handle());
}

/// Handle slash commands.
fn handle_slash_command(app: &mut App, input: &str) {
    let (cmd_name, args) = match input.split_once(' ') {
        Some((cmd, rest)) => (cmd.trim_start_matches('/'), rest),
        None => (input.trim_start_matches('/'), ""),
    };

    // /model opens model selector
    if cmd_name == "model" || cmd_name.starts_with("mod") && args.is_empty() {
        let models = app.available_models.clone();
        let current = app.model.clone();
        app.model_selector = Some(ModelSelector::new(models, &current, &app.theme));
        app.show_model_selector = true;
        return;
    }

    // /help
    if cmd_name == "help" || cmd_name == "h" {
        app.show_help = true;
        return;
    }

    // /quit
    if cmd_name == "quit" || cmd_name == "q" {
        app.should_quit = true;
        return;
    }

    // Unknown command
    app.status_text = Some(format!(
        "Unknown command: /{}. Type /help for available commands.",
        cmd_name
    ));
}

/// Handle ! and !! bang commands.
fn handle_bang_command(app: &mut App, command: String) {
    let cwd = app.cwd.clone();
    let tx = app.event_tx.clone();

    app.messages
        .push(DisplayMsg::User(format!("! {}", command)));
    app.is_streaming = true;
    app.working.start();
    app.footer.set_streaming(true);

    let handle = tokio::spawn(async move {
        let started = std::time::Instant::now();
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
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
                let result = if combined.is_empty() {
                    "(no output)".to_string()
                } else {
                    combined.trim().to_string()
                };

                let _ = tx.send(AgentEvent::ToolResult {
                    id: String::new(),
                    name: "bash".into(),
                    content: format!(
                        "$ {}\n\n{}\n\n[{}s]",
                        command,
                        result,
                        elapsed.as_secs_f64()
                    ),
                    compact: None,
                    is_error: !output.status.success(),
                });
                let _ = tx.send(AgentEvent::AgentEnd { messages: vec![] });
            }
            Err(e) => {
                let _ = tx.send(AgentEvent::ToolResult {
                    id: String::new(),
                    name: "bash".into(),
                    content: format!("Failed to execute: {:#}", e),
                    compact: None,
                    is_error: true,
                });
                let _ = tx.send(AgentEvent::AgentEnd { messages: vec![] });
            }
        }
    });
    app.agent_abort = Some(handle.abort_handle());
}

/// Handle agent events from the channel.
fn handle_agent_event(app: &mut App, event: AgentEvent) {
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
        AgentEvent::ToolCall { name, args, .. } => {
            flush_all(app);
            app.messages.push(DisplayMsg::ToolCall {
                name,
                args: serde_json::to_string(&args).unwrap_or_default(),
            });
        }
        AgentEvent::ToolResult {
            content,
            compact,
            is_error,
            ..
        } => {
            app.messages.push(DisplayMsg::ToolResult {
                content,
                compact,
                is_error,
            });
        }
        AgentEvent::TurnEnd => {
            flush_all(app);
        }
        AgentEvent::AgentEnd { ref messages } => {
            flush_all(app);
            app.is_streaming = false;
            app.working.stop();
            app.footer.set_streaming(false);
            app.agent_abort = None;

            // Persist new messages to session and update conversation state
            // (user message is in prompts, not duplicated in app.conversation)
            if let Some(ref mut session) = app.session {
                for msg in messages {
                    session.append_message(msg);
                }
            }
            // Extend app.conversation so subsequent turns have full context
            for msg in messages {
                if !app.conversation.iter().any(|m| m.id == msg.id) {
                    app.conversation.push(msg.clone());
                }
            }
            if let Some(last) = messages.iter().rev().find(|m| m.usage.is_some()) {
                app.last_usage = last.usage.clone();
                app.footer.accumulate_usage(last.usage.as_ref().unwrap());
            }

            // Process next queued message (pi-style: batch-submit after current finishes)
            if !app.queued_messages.is_empty() {
                let next = app.queued_messages.remove(0);
                start_agent_loop(app, next);
            }
        }
    }
}

fn flush_text(app: &mut App) {
    if let Some(text) = app.pending_text.take()
        && !text.is_empty()
    {
        app.messages.push(DisplayMsg::AssistantText(text));
    }
}

fn flush_thinking(app: &mut App) {
    if let Some(text) = app.pending_thinking.take()
        && !text.is_empty()
    {
        app.messages.push(DisplayMsg::Thinking(text));
    }
}

fn flush_all(app: &mut App) {
    flush_text(app);
    flush_thinking(app);
}

/// Collect tool definitions from the app's agent tools.
fn collect_tool_defs(app: &App) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    for tool in app.agent_tools.iter() {
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

/// Recall history from previous user messages.
fn recall_history(app: &mut App, direction: isize) {
    let user_messages: Vec<String> = app
        .conversation
        .iter()
        .filter_map(|m| {
            if m.role == crate::agent::types::Role::User && !m.content.is_empty() {
                Some(m.content.clone())
            } else {
                None
            }
        })
        .collect();

    if user_messages.is_empty() {
        return;
    }

    let len = user_messages.len();
    let current = app.history_index.unwrap_or(len);

    let new_index = if direction < 0 {
        if current == 0 {
            return;
        }
        current.saturating_sub(1)
    } else {
        if current >= len {
            return;
        }
        current + 1
    };

    if new_index >= len {
        app.editor.editor.set_text("");
        app.history_index = None;
    } else {
        app.editor.editor.set_text(&user_messages[new_index]);
        app.history_index = Some(new_index);
    }
}

/// Parse a ! or !! bang command from input.
fn parse_bang_command(input: &str) -> Option<(String, bool)> {
    if let Some(rest) = input.strip_prefix("!!") {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd.to_string(), true))
        }
    } else if let Some(rest) = input.strip_prefix('!') {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd.to_string(), false))
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::provider::StreamEvent;
    use crate::agent::types::AgentMessage;
    use async_trait::async_trait;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use futures::Stream;
    use std::pin::Pin;
    use tempfile::tempdir;

    struct MockProvider;
    #[async_trait]
    impl Provider for MockProvider {
        async fn stream(
            &self,
            _model: &str,
            _system: &str,
            _msgs: &[AgentMessage],
            _tools: &[ToolDef],
        ) -> anyhow::Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>> {
            unimplemented!()
        }
    }

    #[test]
    fn test_compose_ui_stable_line_count() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        let width = 80;

        // First compose
        let before = compose_ui_test(&mut app, width);
        // Type "/"
        let slash = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        app.editor.editor.handle_input(&slash);
        // Second compose
        let after = compose_ui_test(&mut app, width);

        assert_eq!(
            before.len(),
            after.len(),
            "Line count changed from {} to {}",
            before.len(),
            after.len()
        );

        // Find the border lines and verify they exist in both
        let before_has_top = before.iter().any(|l| l.contains('─'));
        let before_has_bottom = before.iter().any(|l| l.contains('─'));
        let after_has_top = after.iter().any(|l| l.contains('─'));
        let after_has_bottom = after.iter().any(|l| l.contains('─'));

        assert!(before_has_top, "Before: missing top border");
        assert!(before_has_bottom, "Before: missing bottom border");
        assert!(after_has_top, "After: missing top border");
        assert!(after_has_bottom, "After: missing bottom border");

        // The changed line should be the same index in both
        for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
            if b != a {
                eprintln!("Changed line {}: '{}' -> '{}'", i, b, a);
            }
        }
    }

    fn compose_ui_test(app: &mut App, width: usize) -> Vec<String> {
        let theme = &app.theme;
        let mut lines = Vec::new();

        // Header (matches compose_ui)
        let header = format!(
            "{} {}",
            theme.bold(&theme.fg("accent", "rab")),
            theme.fg(
                "dim",
                &format!("· model {}", app.model.replace("opencode_go::", ""))
            )
        );
        lines.push(header);
        let hints = theme.fg("dim", "hint");
        lines.push(format!(" {}", hints));
        lines.push(String::new());

        let rendered = render_messages(
            &app.messages,
            width,
            app.hide_thinking,
            app.collapse_tool_output,
            theme,
        );
        lines.extend(rendered);

        // Pending (streaming) text — matches compose_ui
        if let Some(ref text) = app.pending_text
            && !text.is_empty()
        {
            let inner = width.saturating_sub(2);
            for line in text.lines() {
                if line.is_empty() {
                    lines.push(String::new());
                } else {
                    let wrapped = crate::tui::util::wrap_text_with_ansi(line, inner);
                    for w in wrapped {
                        let line = format!(" {}", w);
                        lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
                    }
                }
            }
        }
        if let Some(ref text) = app.pending_thinking
            && !text.is_empty()
            && !app.hide_thinking
        {
            for line in text.lines() {
                let content = format!(" {}", theme.fg("thinking_text", line));
                let padded = crate::agent::ui::messages::pad_to_width(&content, width);
                lines.push(theme.bg("thinking_bg", &padded));
            }
        }

        // Queued messages — matches compose_ui
        if !app.queued_messages.is_empty() {
            for msg in &app.queued_messages {
                let line = theme.fg("dim", &format!(" ◷ {}", msg));
                lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
            }
            let hint = theme.fg("dim", " ↳ queued — will send when current finishes");
            lines.push(crate::agent::ui::messages::pad_to_width(&hint, width));
        }

        if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.extend(app.working.render(width));
        lines.extend(app.editor.editor.render(width));
        lines.extend(app.footer.render(width));
        lines
    }

    // ── New tests ──

    #[test]
    fn test_submit_queues_when_streaming() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);

        // Simulate streaming in progress
        app.is_streaming = true;
        app.queued_messages.clear();

        // Submit a message while streaming
        submit_message(&mut app, "hello".into());

        assert!(
            app.queued_messages.contains(&"hello".to_string()),
            "Message should be queued when streaming"
        );
        assert!(
            app.is_streaming,
            "is_streaming should remain true after queuing"
        );
    }

    #[tokio::test]
    async fn test_submit_starts_loop_when_not_streaming() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        app.queued_messages.clear();

        // Submit a message while NOT streaming
        submit_message(&mut app, "hello".into());

        // Should have one user message (no startup info messages)
        assert_eq!(app.messages.len(), 1, "Should have just the user message");
        assert!(
            matches!(app.messages.last(), Some(DisplayMsg::User(_))),
            "Last message should be User"
        );
    }
    #[test]
    fn test_compose_ui_shows_queued_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        app.queued_messages.push("queued-msg-1".into());
        app.queued_messages.push("queued-msg-2".into());

        let lines = compose_ui_test(&mut app, 80);

        let all = lines.join("\n");
        assert!(
            all.contains("queued-msg-1"),
            "Compose UI should contain queued message 1"
        );
        assert!(
            all.contains("queued-msg-2"),
            "Compose UI should contain queued message 2"
        );
        assert!(
            all.contains("queued"),
            "Compose UI should contain 'queued' hint"
        );
    }

    #[test]
    fn test_compose_ui_shows_pending_text() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        app.pending_text = Some("streaming text content".into());

        let lines = compose_ui_test(&mut app, 80);
        let all = lines.join("\n");
        assert!(
            all.contains("streaming text"),
            "Compose UI should contain pending streaming text"
        );
    }

    #[test]
    fn test_compose_ui_shows_pending_thinking() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: false,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        app.pending_thinking = Some("thinking content".into());

        let lines = compose_ui_test(&mut app, 80);
        let all = lines.join("\n");
        assert!(
            all.contains("thinking content"),
            "Compose UI should contain pending thinking text when not hidden"
        );
    }

    #[test]
    fn test_pending_thinking_hidden_when_hide_thinking() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        app.pending_thinking = Some("hidden thinking".into());

        let lines = compose_ui_test(&mut app, 80);
        let all = lines.join("\n");
        assert!(
            !all.contains("hidden thinking"),
            "Compose UI should NOT contain thinking content when hide_thinking is true"
        );
    }

    #[tokio::test]
    async fn test_agent_end_processes_queued_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        app.queued_messages.push("next-msg".into());
        app.is_streaming = true;
        app.working.start();

        // Simulate AgentEnd — this will dequeue and start a new loop
        handle_agent_event(&mut app, AgentEvent::AgentEnd { messages: vec![] });

        // The queued message should have been dequeued
        assert!(
            app.queued_messages.is_empty(),
            "Queued messages should be dequeued after AgentEnd"
        );
    }

    #[test]
    fn test_ctrl_c_interrupt_restores_queued_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);
        app.queued_messages.push("q1".into());
        app.queued_messages.push("q2".into());
        app.is_streaming = true;

        // Simulate Ctrl+C
        handle_input(
            &mut app,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        assert!(
            app.queued_messages.is_empty(),
            "Queued messages should be cleared after interrupt"
        );
        assert!(
            app.editor.editor.get_text().contains("q1"),
            "Editor should contain restored queued messages"
        );
        assert!(
            app.editor.editor.get_text().contains("q2"),
            "Editor should contain both restored queued messages"
        );
    }

    #[test]
    fn test_render_messages_pads_assistant_text() {
        use crate::agent::ui::theme::RabTheme;

        let theme = RabTheme;
        let msgs = vec![DisplayMsg::AssistantText("short line".into())];

        let width = 60;
        let lines = render_messages(&msgs, width, false, false, &theme);

        for (i, line) in lines.iter().enumerate() {
            let vw = crate::tui::util::visible_width(line);
            assert!(
                vw <= width,
                "Line {} has visible_width {} > width {}: {:?}",
                i,
                vw,
                width,
                line
            );
            // The line should be padded to exactly width (no undershoot)
            if !line.is_empty() {
                assert!(
                    vw >= width.saturating_sub(2),
                    "Line {} has visible_width {} < width-2 {}: {:?}",
                    i,
                    vw,
                    width.saturating_sub(2),
                    line
                );
            }
        }
    }

    #[test]
    fn test_queued_messages_rendered_in_compose_ui_line_count_is_stable() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);

        // No queued messages — compose
        let before = compose_ui_test(&mut app, 80);

        // Add queued messages
        app.queued_messages.push("msg1".into());
        let after = compose_ui_test(&mut app, 80);

        // Should have more lines with queued messages
        assert!(
            after.len() > before.len(),
            "Line count should increase when queued messages are present"
        );

        // Queued messages appear between messages and editor, not at the end.
        // Search the entire output for the queued message text.
        let after_text = after.join("\n");
        assert!(
            after_text.contains("msg1"),
            "Output should contain queued message text"
        );
    }

    // ── Conversation / send behavior (matches pi) ────────────────

    #[test]
    fn test_submit_message_does_not_add_to_conversation() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);

        // Mark as streaming so submit_message queues instead of spawning a tokio task
        app.is_streaming = true;
        let initial_len = app.conversation.len();

        // Submit a message while streaming (queues, no tokio::spawn)
        submit_message(&mut app, "test message".into());

        // Pi: submit_message sends to agent loop, which emits message_end for persistence.
        // The message is NOT added to app.conversation directly — it flows through AgentEnd.
        assert_eq!(
            app.conversation.len(),
            initial_len,
            "submit_message must not add to app.conversation (avoids double-send)"
        );

        // But the display message IS added immediately (so the UI shows it)
        assert!(
            app.messages
                .iter()
                .any(|m| matches!(m, DisplayMsg::User(t) if t == "test message")),
            "submit_message must add DisplayMsg::User for immediate display"
        );
    }

    #[test]
    fn test_agent_end_populates_conversation() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);

        // Simulate the messages that AgentEnd receives from run_agent_loop
        let user_msg = AgentMessage::user("hello");
        let assistant_msg = AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role: crate::agent::types::Role::Assistant,
            content: "Hello back".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        let agent_messages = vec![user_msg.clone(), assistant_msg.clone()];

        // Fire AgentEnd
        handle_agent_event(
            &mut app,
            AgentEvent::AgentEnd {
                messages: agent_messages,
            },
        );

        // Pi: all messages from the turn are added to conversation on AgentEnd.
        // The user message appears once (not duplicated), followed by assistant responses.
        assert_eq!(
            app.conversation.len(),
            2,
            "conversation should have user + assistant"
        );
        assert_eq!(app.conversation[0].content, "hello");
        assert_eq!(app.conversation[0].role, crate::agent::types::Role::User);
        assert_eq!(app.conversation[1].content, "Hello back");
        assert_eq!(
            app.conversation[1].role,
            crate::agent::types::Role::Assistant
        );
    }

    #[test]
    fn test_agent_end_no_duplicate_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
        };

        let mut app = App::new(config, session);

        // Simulate a message already in conversation (e.g. from a previous turn)
        let existing = AgentMessage::user("existing");
        let existing_id = existing.id.clone();
        app.conversation.push(existing);

        // AgentEnd fires with the SAME message id — should NOT duplicate
        let dup_msg = AgentMessage {
            id: existing_id,
            parent_id: None,
            role: crate::agent::types::Role::User,
            content: "existing".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        };

        handle_agent_event(
            &mut app,
            AgentEvent::AgentEnd {
                messages: vec![dup_msg],
            },
        );

        // Should still have exactly 1 message (deduplicated by id)
        assert_eq!(
            app.conversation.len(),
            1,
            "AgentEnd must not duplicate messages already in conversation (pi-style)"
        );
    }
}
