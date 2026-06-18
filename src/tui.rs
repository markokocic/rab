use crate::agent::{AgentEvent, LoopConfig, run_agent_loop};
use crate::extension::{AgentTool, CommandResult, Extension, SlashCommand};
use crate::provider::{Provider, ToolDef};
use crate::types::AgentMessage;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
};
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
}

// ── Display messages ───────────────────────────────────────────────

enum DisplayMsg {
    User(String),
    AssistantText(String),
    Thinking(String),
    ToolCall { name: String, args: String },
    ToolResult { content: String, is_error: bool },
    Info(String),
}

fn welcome_messages(config: &TuiConfig) -> Vec<DisplayMsg> {
    let model_display = config.model.replace("opencode_go::", "");
    let cwd_str = config.cwd.to_str().unwrap_or("?");
    let tool_names: Vec<String> = config.tools.iter().map(|t| t.name.clone()).collect();

    // Collect slash commands from all extensions
    let commands: Vec<SlashCommand> = config
        .extensions
        .iter()
        .flat_map(|e| e.commands())
        .collect();
    let cmd_names: Vec<String> = commands.iter().map(|c| format!("/{}", c.name)).collect();

    let mut msgs = Vec::new();
    msgs.push(DisplayMsg::Info(format!(
        "rab · model {model_display} · {cwd_str}"
    )));
    msgs.push(DisplayMsg::Info(format!(
        "Tools: {}",
        tool_names.join(", ")
    )));
    if !cmd_names.is_empty() {
        msgs.push(DisplayMsg::Info(format!(
            "Commands: {}",
            cmd_names.join(", ")
        )));
    }
    msgs.push(DisplayMsg::Info(
        "Enter  submit · Ctrl+D  quit · Ctrl+C  clear · F1  help · Ctrl+T  thinking · Ctrl+O  tools\n\
         Shift+Enter  newline · Esc  clear · ↑↓ PgUp/PgDn  scroll"
            .to_string(),
    ));
    msgs
}

// ── App state ──────────────────────────────────────────────────────

/// Data shared between the TUI main thread and spawned agent tasks.
struct SharedState {
    agent_tools: Vec<Box<dyn AgentTool>>,
    extensions: Vec<Box<dyn Extension>>,
    /// Flattened slash commands from all extensions.
    commands: Vec<SlashCommand>,
}

struct App {
    cwd: PathBuf,
    model: String,
    system_prompt: String,
    shared: Arc<SharedState>,
    provider: Arc<dyn Provider>,

    /// Conversation history (AgentMessage, not DisplayMsg)
    conversation: Vec<AgentMessage>,

    /// Rendered display messages
    messages: Vec<DisplayMsg>,
    /// Scroll state: top line index (from top of content). Managed via Cell for render access.
    scroll_line: Cell<usize>,
    auto_scroll: Cell<bool>,

    editor: tui_textarea::TextArea<'static>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,

    is_streaming: bool,
    pending_text: Option<String>,
    pending_thinking: Option<String>,

    thinking_collapsed: bool,
    tool_output_collapsed: bool,
    show_help: bool,

    should_quit: bool,
    last_usage: Option<crate::types::Usage>,
}

// ── Public entry point ─────────────────────────────────────────────

pub async fn run(config: TuiConfig) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let result = run_app(&mut terminal, config);

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    result
}

// ── Main event loop ────────────────────────────────────────────────

fn run_app(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    config: TuiConfig,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();

    let welcome = welcome_messages(&config);

    // Collect slash commands from all extensions
    let commands: Vec<SlashCommand> = config
        .extensions
        .iter()
        .flat_map(|e| e.commands())
        .collect();

    let shared = Arc::new(SharedState {
        agent_tools: config.agent_tools,
        extensions: config.extensions,
        commands,
    });

    let mut app = App {
        cwd: config.cwd,
        model: config.model.clone(),
        system_prompt: config.system_prompt,
        shared,
        provider: Arc::from(config.provider),
        conversation: Vec::new(),
        messages: welcome,
        scroll_line: Cell::new(0),
        auto_scroll: Cell::new(true),
        editor: create_editor(),
        event_tx: tx,
        event_rx: rx,
        is_streaming: false,
        pending_text: None,
        pending_thinking: None,
        thinking_collapsed: false,
        tool_output_collapsed: false,
        show_help: false,
        should_quit: false,
        last_usage: None,
    };

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Poll for keyboard events
        if crossterm::event::poll(Duration::from_millis(10))? {
            match crossterm::event::read()? {
                Event::Key(key) => handle_key(&mut app, key),
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => scroll_up(&mut app, 3),
                    MouseEventKind::ScrollDown => scroll_down(&mut app, 3),
                    _ => {}
                },
                Event::Resize(..) => {}
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

fn create_editor() -> tui_textarea::TextArea<'static> {
    let mut editor = tui_textarea::TextArea::default();
    editor.set_cursor_line_style(Style::default());
    editor.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    editor.set_block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    editor
}

// ── Render ─────────────────────────────────────────────────────────

fn ui(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                   // header
            Constraint::Min(1),                      // messages
            Constraint::Length(editor_height(app)),  // editor
            Constraint::Length(working_height(app)), // working indicator
            Constraint::Length(1),                   // footer
        ])
        .split(area);

    render_header(frame, chunks[0], app);
    render_messages(frame, chunks[1], app);
    render_editor(frame, chunks[2], app);
    render_working(frame, chunks[3], app);
    render_footer(frame, chunks[4], app);
}

fn working_height(app: &App) -> u16 {
    if app.is_streaming { 1 } else { 0 }
}

fn editor_height(app: &App) -> u16 {
    let lines = app.editor.lines().len().max(1);
    // +1 for top border, clamp to 3..10
    (lines + 1).clamp(3, 10) as u16
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let model_display = app.model.replace("opencode_go::", "");
    let header = Span::styled(
        format!(" rab · {} ", model_display),
        Style::default()
            .fg(Color::Black)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(Paragraph::new(Line::from(header)), area);
}

fn render_messages(frame: &mut Frame, area: Rect, app: &App) {
    let text = build_message_text(app);
    let total_lines = text.lines.len().saturating_sub(1);
    let viewport = area.height.saturating_sub(1) as usize;
    let bottom = total_lines.saturating_sub(viewport);

    let scroll = if app.auto_scroll.get() {
        app.scroll_line.set(bottom);
        bottom
    } else {
        let clamped = app.scroll_line.get().min(bottom);
        if clamped >= bottom {
            app.auto_scroll.set(true);
        }
        clamped
    };

    let para = Paragraph::new(text).scroll((scroll as u16, 0));
    frame.render_widget(para, area);
}

fn build_message_text(app: &App) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Help overlay
    if app.show_help {
        lines.extend(help_lines(app));
        return Text::from(lines);
    }

    for msg in &app.messages {
        match msg {
            DisplayMsg::User(text) => {
                lines.push(Line::from(Span::styled(
                    format!("▸ {}", text),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }
            DisplayMsg::Thinking(text) => {
                if app.thinking_collapsed {
                    continue;
                }
                for line in text.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            DisplayMsg::AssistantText(text) => {
                for line in text.lines() {
                    lines.push(Line::from(line.to_string()));
                }
                if !lines.is_empty()
                    && !lines
                        .last()
                        .is_none_or(|l| l.spans.iter().all(|s| s.content.is_empty()))
                {
                    lines.push(Line::from(""));
                }
            }
            DisplayMsg::ToolCall { name, args, .. } => {
                let args_display = if args.len() > 100 {
                    let truncated: String = args.chars().take(100).collect();
                    format!("{}…", truncated)
                } else {
                    args.clone()
                };
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(" ⚙ ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(args_display, Style::default().fg(Color::DarkGray)),
                ]));
            }
            DisplayMsg::ToolResult {
                content, is_error, ..
            } => {
                if app.tool_output_collapsed {
                    continue;
                }
                let (prefix, style) = if *is_error {
                    (" ✗ ", Style::default().fg(Color::Red))
                } else {
                    (" ✓ ", Style::default().fg(Color::DarkGray))
                };
                for (i, line) in content.lines().take(3).enumerate() {
                    let truncated: String = line.chars().take(140).collect();
                    let suffix = if line.len() > 140 || (i == 2 && content.lines().count() > 3) {
                        " …"
                    } else {
                        ""
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{}{}{}", prefix, truncated, suffix),
                        style,
                    )));
                }
            }
            DisplayMsg::Info(text) => {
                lines.push(Line::from(Span::styled(
                    text.clone(),
                    Style::default().fg(Color::Gray),
                )));
                lines.push(Line::from(""));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Type a message and press Enter to send.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    Text::from(lines)
}

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let accent = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(Span::styled("Keyboard Shortcuts", accent)),
        Line::from(""),
        Line::from(Span::styled("  Enter              Submit message", dim)),
        Line::from(Span::styled("  Shift+Enter        Newline", dim)),
        Line::from(Span::styled("  Ctrl+C             Clear editor", dim)),
        Line::from(Span::styled(
            "  Ctrl+D             Quit (editor empty)",
            dim,
        )),
        Line::from(Span::styled("  Escape             Clear editor", dim)),
        Line::from(Span::styled("  Ctrl+T             Toggle thinking", dim)),
        Line::from(Span::styled("  Ctrl+O             Toggle tool output", dim)),
        Line::from(Span::styled("  F1                 Show this help", dim)),
        Line::from(Span::styled(
            "  ↑↓                 Scroll (editor empty)",
            dim,
        )),
        Line::from(Span::styled("  PgUp / PgDn        Page scroll", dim)),
        Line::from(Span::styled("  Mouse wheel        Scroll", dim)),
    ];

    // List slash commands from extensions
    let commands = collect_commands(app);
    if !commands.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Slash Commands", accent)));
        lines.push(Line::from(""));
        for cmd in &commands {
            lines.push(Line::from(Span::styled(
                format!("  /{:<20} {}", cmd.name, cmd.description),
                dim,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press any key to close help.",
        dim,
    )));
    lines
}

fn render_editor(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(&app.editor, area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let status = if app.is_streaming {
        Span::styled(" ● ", Style::default().fg(Color::Yellow))
    } else {
        Span::styled(" ○ ", Style::default().fg(Color::Green))
    };

    let cwd_str = app.cwd.to_str().unwrap_or("?");
    let model_str = &app.model;

    let tokens_str = app.last_usage.as_ref().map_or(String::new(), |u| {
        let input = u.input_tokens.unwrap_or(0);
        let output = u.output_tokens.unwrap_or(0);
        format!("↑{} ↓{}", input, output)
    });

    let mut spans: Vec<Span> = vec![
        Span::styled(cwd_str, Style::default().fg(Color::DarkGray)),
        Span::raw(" · "),
        Span::styled(model_str, Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        status,
    ];

    // Collapse indicators
    if app.thinking_collapsed {
        spans.push(Span::styled(" T ", Style::default().fg(Color::Yellow)));
    }
    if app.tool_output_collapsed {
        spans.push(Span::styled(" O ", Style::default().fg(Color::Yellow)));
    }

    if !tokens_str.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            tokens_str,
            Style::default().fg(Color::DarkGray),
        ));
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(para, area);
}

fn render_working(frame: &mut Frame, area: Rect, app: &App) {
    if !app.is_streaming {
        return;
    }
    let text = Span::styled(" Working…", Style::default().fg(Color::Yellow));
    frame.render_widget(Paragraph::new(Line::from(text)), area);
}

// ── Scroll helpers ─────────────────────────────────────────────────

fn scroll_up(app: &mut App, lines: usize) {
    app.auto_scroll.set(false);
    let current = app.scroll_line.get();
    app.scroll_line.set(current.saturating_sub(lines));
}

fn scroll_down(app: &mut App, lines: usize) {
    if app.auto_scroll.get() {
        return; // already at bottom
    }
    let current = app.scroll_line.get();
    app.scroll_line.set(current.saturating_add(lines));
    // auto_scroll will resume when render detects we're at bottom
}

// ── Keyboard handling ──────────────────────────────────────────────

fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        // Tab: slash command autocomplete (handle both Tab and Char('\t'))
        KeyCode::Tab | KeyCode::Char('\t') => {
            if app.show_help {
                app.show_help = false;
                return;
            }
            let text = app.editor.lines().join("\n");
            if text.trim().starts_with('/') {
                handle_slash_completion(app, &text);
                return;
            }
            // Not a slash command — pass Tab through to editor
            app.editor.input(Event::Key(key));
        }
        // Ctrl+C: clear editor (pi: app.clear)
        KeyCode::Char('c') if ctrl => {
            if app.show_help {
                app.show_help = false;
            } else {
                app.editor = create_editor();
            }
        }
        // Ctrl+D: quit when editor empty (pi: app.exit)
        KeyCode::Char('d') if ctrl => {
            if app.editor.is_empty() && !app.show_help {
                app.should_quit = true;
            }
        }
        // Escape: clear editor or close help
        KeyCode::Esc => {
            if app.show_help {
                app.show_help = false;
            } else {
                app.editor = create_editor();
            }
        }
        // Ctrl+T: toggle thinking (pi: app.thinking.toggle)
        KeyCode::Char('t') if ctrl => {
            app.thinking_collapsed = !app.thinking_collapsed;
        }
        // Ctrl+O: toggle tool output (pi: app.tools.expand)
        KeyCode::Char('o') if ctrl => {
            app.tool_output_collapsed = !app.tool_output_collapsed;
        }
        // F1: show help
        KeyCode::F(1) => {
            app.show_help = !app.show_help;
        }
        // Enter: submit (Shift+Enter / Alt+Enter: newline)
        KeyCode::Enter if !shift && !alt && !ctrl => {
            let text = app.editor.lines().join("\n");
            let trimmed = text.trim();
            if !trimmed.is_empty() && !app.is_streaming {
                submit_message(app, trimmed.to_string());
            }
        }
        // PageUp/PageDown → scroll messages
        KeyCode::PageUp => scroll_up(app, 10),
        KeyCode::PageDown => scroll_down(app, 10),
        // Arrow up: scroll messages when editor empty, otherwise move cursor
        KeyCode::Up if app.editor.is_empty() => scroll_up(app, 1),
        // Arrow down: scroll messages when editor empty
        KeyCode::Down if app.editor.is_empty() => scroll_down(app, 1),
        // Everything else: pass to editor
        _ => {
            app.editor.input(Event::Key(key));
        }
    }
}

/// Handle Tab autocomplete for slash commands.
/// If the text is just `/` or a partial command name, completes the command.
/// If the text is `/cmd ` with partial args, uses argument_completions.
fn handle_slash_completion(app: &mut App, text: &str) {
    let trimmed = text.trim();
    let commands = collect_commands(app);

    let space_idx = trimmed.find(' ');
    match space_idx {
        None => {
            // No space: completing the command name (e.g. "/mo" → "/model")
            let prefix = trimmed.trim_start_matches('/');
            let lower = prefix.to_lowercase();
            let matches: Vec<&&SlashCommand> = commands
                .iter()
                .filter(|c| c.name.to_lowercase().starts_with(&lower))
                .collect();

            if matches.len() == 1 {
                // Exact one match: execute the command
                let cmd_text = format!("/{}", matches[0].name);
                submit_message(app, cmd_text);
            } else if matches.len() > 1 {
                // Multiple matches: show common prefix completion
                let common =
                    common_prefix(&matches.iter().map(|c| c.name.as_str()).collect::<Vec<_>>());
                if common.len() > prefix.len() {
                    let new_text = format!("/{}", common);
                    set_editor_text(app, &new_text);
                } else {
                    // Show available commands as info
                    let names: Vec<String> =
                        matches.iter().map(|c| format!("/{}", c.name)).collect();
                    app.messages.push(DisplayMsg::Info(names.join("  ")));
                    app.auto_scroll.set(true);
                }
            }
        }
        Some(idx) => {
            // Has space: completing the argument (e.g. "/model deep")
            let cmd_name = trimmed[..idx].trim_start_matches('/');
            let arg_prefix = &trimmed[idx..].trim();

            if let Some(cmd) = commands.iter().find(|c| c.name == cmd_name) {
                let completions = cmd.handler.argument_completions(arg_prefix);
                if completions.len() == 1 {
                    // Single match: execute the command
                    let cmd_text = format!("/{} {}", cmd_name, completions[0].value);
                    submit_message(app, cmd_text);
                } else if completions.len() > 1 {
                    let values: Vec<String> = completions.iter().map(|c| c.value.clone()).collect();
                    let common =
                        common_prefix(&values.iter().map(|s| s.as_str()).collect::<Vec<_>>());
                    if common.len() > arg_prefix.len() {
                        let new_text = format!("/{} {}", cmd_name, common);
                        set_editor_text(app, &new_text);
                    } else {
                        app.messages.push(DisplayMsg::Info(values.join("  ")));
                        app.auto_scroll.set(true);
                    }
                }
            }
        }
    }
}

/// Set editor text and move cursor to end.
fn set_editor_text(app: &mut App, text: &str) {
    let mut editor = create_editor();
    editor.insert_str(text);
    app.editor = editor;
    // tui_textarea doesn't have a direct set_text with cursor-to-end,
    // but insert_str on empty editor places cursor after the inserted text.
}

/// Find the longest common prefix of a list of strings.
fn common_prefix(strings: &[&str]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first = strings[0];
    let mut end = first.len();
    for s in &strings[1..] {
        end = end.min(
            first
                .chars()
                .zip(s.chars())
                .take(end)
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    first[..end].to_string()
}

/// Apply a command result to the app state.
fn apply_command_result(app: &mut App, result: anyhow::Result<CommandResult>) {
    match result {
        Ok(CommandResult::Info(text)) => {
            app.messages.push(DisplayMsg::Info(text));
        }
        Ok(CommandResult::Quit) => {
            app.messages
                .push(DisplayMsg::Info("/quit — exiting".to_string()));
            app.editor = create_editor();
            app.should_quit = true;
            return;
        }
        Ok(CommandResult::ModelChanged(new_model)) => {
            app.model = new_model.clone();
            app.messages.push(DisplayMsg::Info(format!(
                "Model: {}",
                new_model.replace("opencode_go::", "")
            )));
        }
        Err(e) => {
            app.messages
                .push(DisplayMsg::Info(format!("Command error: {:#}", e)));
        }
    }
    app.editor = create_editor();
    app.auto_scroll.set(true);
}

fn submit_message(app: &mut App, message: String) {
    let trimmed = message.trim();

    // Handle slash commands via extension handlers
    if trimmed.starts_with('/') {
        let (cmd_name, args) = match trimmed.split_once(' ') {
            Some((cmd, rest)) => (cmd.trim_start_matches('/'), rest),
            None => (trimmed.trim_start_matches('/'), ""),
        };

        // Try exact match first
        let commands = collect_commands(app);
        if let Some(cmd) = commands.iter().find(|c| c.name == cmd_name) {
            let result = cmd.handler.execute(args);
            apply_command_result(app, result);
            return;
        }

        // Try prefix match — execute if exactly one, suggest if multiple
        let lower = cmd_name.to_lowercase();
        let prefix_matches: Vec<&&SlashCommand> = commands
            .iter()
            .filter(|c| c.name.to_lowercase().starts_with(&lower))
            .collect();

        if prefix_matches.len() == 1 {
            let result = prefix_matches[0].handler.execute(args);
            drop(prefix_matches);
            drop(commands);
            apply_command_result(app, result);
            return;
        }

        if prefix_matches.len() > 1 {
            let names: Vec<String> = prefix_matches
                .iter()
                .map(|c| format!("/{}", c.name))
                .collect();
            app.messages.push(DisplayMsg::Info(format!(
                "Did you mean: {}?",
                names.join(", ")
            )));
            app.editor = create_editor();
            app.auto_scroll.set(true);
            return;
        }

        // No match at all
        app.messages.push(DisplayMsg::Info(format!(
            "Unknown command: /{}. Type / for available commands.",
            cmd_name
        )));
        app.editor = create_editor();
        app.auto_scroll.set(true);
        return;
    }

    let provider = Arc::clone(&app.provider);
    let shared = Arc::clone(&app.shared);
    let model = app.model.clone();
    let system_prompt = app.system_prompt.clone();
    let tools = collect_tool_defs_from_shared(&shared);
    let tx = app.event_tx.clone();

    // Add user message to display and conversation
    app.messages.push(DisplayMsg::User(trimmed.to_string()));
    app.auto_scroll.set(true);

    let prompt = AgentMessage::user(trimmed);
    app.conversation.push(prompt.clone());

    // Clear editor and set streaming
    app.editor = create_editor();
    app.is_streaming = true;
    app.pending_text = None;
    app.pending_thinking = None;

    tokio::spawn(async move {
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

        let _ = run_agent_loop(vec![prompt], &loop_config, &*provider, &mut emit).await;
    });
}

/// Collect tool defs, avoiding duplicate names.
fn collect_tool_defs_from_shared(shared: &SharedState) -> Vec<ToolDef> {
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
fn collect_commands(app: &App) -> Vec<&SlashCommand> {
    let mut seen = std::collections::HashSet::new();
    let mut cmds: Vec<&SlashCommand> = Vec::new();
    for cmd in &app.shared.commands {
        if seen.insert(&cmd.name) {
            cmds.push(cmd);
        }
    }
    cmds
}

// ── Agent event handling ───────────────────────────────────────────

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
            app.auto_scroll.set(true);
        }
        AgentEvent::ThinkingDelta { delta } => {
            if let Some(ref mut text) = app.pending_thinking {
                text.push_str(&delta);
            } else {
                flush_text(app);
                app.pending_thinking = Some(delta);
            }
            app.auto_scroll.set(true);
        }
        AgentEvent::ToolCall {
            ref name, ref args, ..
        } => {
            flush_all(app);
            app.messages.push(DisplayMsg::ToolCall {
                name: name.clone(),
                args: serde_json::to_string(args).unwrap_or_default(),
            });
            app.auto_scroll.set(true);
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
            app.auto_scroll.set(true);
        }
        AgentEvent::TurnEnd => {
            flush_all(app);
        }
        AgentEvent::AgentEnd { ref messages } => {
            flush_all(app);
            app.is_streaming = false;
            // Extract usage from the last assistant message
            if let Some(last) = messages.iter().rev().find(|m| m.usage.is_some()) {
                app.last_usage = last.usage.clone();
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
