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

    /// Agent tools (for tool execution).
    agent_tools: Arc<Vec<Box<dyn AgentTool>>>,
    /// Extensions.
    extensions: Arc<Vec<Box<dyn Extension>>>,
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

        // Welcome messages
        let mut messages = history_display;
        messages.push(DisplayMsg::Info(format!(
            "rab · model {} · {}",
            config.model.replace("opencode_go::", ""),
            config.cwd.display()
        )));
        let tool_names: Vec<String> = config.tools.iter().map(|t| t.name.clone()).collect();
        messages.push(DisplayMsg::Info(format!(
            "Tools: {}",
            tool_names.join(", ")
        )));
        if !commands.is_empty() {
            let cmd_names: Vec<String> = commands.iter().map(|(n, _)| format!("/{}", n)).collect();
            messages.push(DisplayMsg::Info(format!(
                "Commands: {}",
                cmd_names.join(", ")
            )));
        }

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
        }
    }
}

/// Run the interactive UI.
pub async fn run(config: AppConfig, session: SessionManager) -> anyhow::Result<()> {
    let mut term = Terminal::new();
    term.enter_raw_mode()?;
    let mut stdout = std::io::stdout();

    // Enter alternate screen
    write!(stdout, "\x1b[?1049h")?;
    stdout.flush()?;

    Terminal::hide_cursor(&mut stdout)?;

    let mut screen = Screen::new();
    let mut app = App::new(config, session);

    loop {
        // Get terminal size
        let (cols, rows) = Terminal::size()?;

        // Compose UI
        let lines = compose_ui(&mut app, cols as usize, rows as usize);

        // Render to screen
        screen.render(lines, cols, rows, &mut stdout)?;

        // Poll for events
        let timeout = if app.is_streaming || app.working.active {
            Duration::from_millis(10)
        } else {
            Duration::from_millis(100)
        };

        if let Some(key) = terminal::poll_key_event(Some(timeout))? {
            handle_input(&mut app, &key);
        }

        // Drain agent events
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
        }

        // Tick the working indicator
        app.working.tick();

        if app.should_quit {
            break;
        }
    }

    // Cleanup
    Terminal::show_cursor(&mut stdout)?;
    write!(stdout, "\x1b[?1049l")?; // Leave alternate screen
    stdout.flush()?;
    term.leave_raw_mode()?;

    Ok(())
}

/// Compose the full UI from app state — matching pi's main screen layout.
///
/// Layout (top to bottom):
///   messages → spacer → status line → editor → key hints → working → footer
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

    // ── Messages ──
    let rendered = render_messages(
        &app.messages,
        width,
        app.hide_thinking,
        app.collapse_tool_output,
        &app.theme,
    );
    lines.extend(rendered);

    // ── Spacer before editor ──
    // Pi inserts a blank line between messages and editor
    if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
        lines.push(String::new());
    }

    // ── Status/working line ──
    if app.is_streaming {
        let spinner = app.working.render(width);
        lines.extend(spinner);
    }

    // ── Editor ──
    let editor_lines = app.editor.editor.render(width);
    lines.extend(editor_lines);

    // ── Keybinding hints ──
    let hint = app.theme.fg(
        "dim",
        "Enter submit · Ctrl+J newline · Esc clear · Ctrl+C interrupt · Ctrl+D quit · ↑↓ history · F1 help · Ctrl+L model",
    );
    lines.push(format!(" {}", hint));

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
                app.messages.push(DisplayMsg::Info(format!(
                    "Model: {}",
                    model.replace("opencode_go::", "")
                )));
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
            app.messages.push(DisplayMsg::Info("Interrupted".into()));
        } else {
            // Clear editor
            app.editor.editor.set_text("");
        }
        return;
    }

    if matches_key(key, &Key::Escape) {
        app.editor.editor.set_text("");
        app.history_index = None;
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
        return;
    }

    if matches_key(key, &Key::Ctrl('o')) {
        app.collapse_tool_output = !app.collapse_tool_output;
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

    // Up/Down for history
    if matches_key(key, &Key::Up) && app.editor.editor.get_text().is_empty() {
        recall_history(app, -1);
        return;
    }

    if matches_key(key, &Key::Down) && app.editor.editor.get_text().is_empty() {
        recall_history(app, 1);
        return;
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

/// Submit a user message to the agent loop.
fn submit_message(app: &mut App, message: String) {
    app.history_index = None;
    let trimmed = message.trim().to_string();

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
    let provider = Arc::clone(&app.provider);
    let model = app.model.clone();
    let system_prompt = app.system_prompt.clone();
    let tools = collect_tool_defs(app);
    let tx = app.event_tx.clone();
    let history = app.conversation.clone();
    let agent_tools = Arc::clone(&app.agent_tools);
    let extensions = Arc::clone(&app.extensions);

    app.messages.push(DisplayMsg::User(trimmed.clone()));
    app.conversation.push(AgentMessage::user(&trimmed));

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

        let prompt = AgentMessage::user(trimmed);
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
    app.messages.push(DisplayMsg::Info(format!(
        "Unknown command: /{}. Type /help for available commands.",
        cmd_name
    )));
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
                    is_error: !output.status.success(),
                });
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
            content, is_error, ..
        } => {
            app.messages
                .push(DisplayMsg::ToolResult { content, is_error });
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

            if let Some(ref mut session) = app.session {
                for msg in messages {
                    session.append_message(msg);
                }
            }
            if let Some(last) = messages.iter().rev().find(|m| m.usage.is_some()) {
                app.last_usage = last.usage.clone();
                app.footer.accumulate_usage(last.usage.as_ref().unwrap());
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
