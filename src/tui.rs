use crate::agent::{AgentEvent, LoopConfig, run_agent_loop};
use crate::editor::{Editor, SlashCommandInfo};
use crate::extension::{AgentTool, CommandResult, Extension, SlashCommand};
use crate::provider::{Provider, ToolDef};
use crate::session::SessionManager;
use crate::theme::{DARK, Theme};
use crate::types::AgentMessage;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
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
    pub thinking_level: Option<String>,
    pub git_branch: Option<String>,
    pub available_models: Vec<String>,
    pub hide_thinking: bool,
    pub collapse_tool_output: bool,
}

// ── Display messages ───────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum DisplayMsg {
    User(String),
    AssistantText(String),
    Thinking(String),
    ToolCall { name: String, args: String },
    ToolResult { content: String, is_error: bool },
    Info(String),
}

// ── Editor creation ────────────────────────────────────────────────

fn create_editor_with(commands: &[SlashCommandInfo], cwd: &std::path::Path) -> Editor {
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

fn create_editor(app: &App) -> Editor {
    create_editor_with(&app.shared.command_infos, &app.cwd)
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
        "Enter  submit · Ctrl+C  interrupt/clear · Ctrl+D  quit · F1  help · Ctrl+L  model · Ctrl+T  thinking · Ctrl+O  tools\n\
         Shift+Enter  newline · Esc  clear · ↑↓  history · !  bash"
            .to_string(),
    ));
    msgs
}

/// Convert session AgentMessages to display messages for the TUI.
/// This is extracted for testability — verifies history is properly loaded.
pub(crate) fn session_messages_to_display(messages: &[AgentMessage]) -> Vec<DisplayMsg> {
    messages
        .iter()
        .map(|m| match m.role {
            crate::types::Role::User => DisplayMsg::User(m.content.clone()),
            crate::types::Role::Assistant => DisplayMsg::AssistantText(m.content.clone()),
            crate::types::Role::ToolResult => {
                let prefix = if m.is_error { "✗" } else { "✓" };
                DisplayMsg::ToolResult {
                    content: format!("{} {}", prefix, m.content),
                    is_error: m.is_error,
                }
            }
        })
        .collect()
}

// ── App state ──────────────────────────────────────────────────────

/// Data shared between the TUI main thread and spawned agent tasks.
struct SharedState {
    agent_tools: Vec<Box<dyn AgentTool>>,
    extensions: Vec<Box<dyn Extension>>,
    /// Flattened slash commands from all extensions.
    commands: Vec<SlashCommand>,
    /// Command info for the editor's autocomplete.
    command_infos: Vec<SlashCommandInfo>,
}

struct App {
    cwd: PathBuf,
    model: String,
    thinking_level: Option<String>,
    git_branch: Option<String>,
    system_prompt: String,
    shared: Arc<SharedState>,
    provider: Arc<dyn Provider>,
    theme: Theme,

    /// Conversation history (AgentMessage, not DisplayMsg)
    conversation: Vec<AgentMessage>,

    /// Rendered display messages
    messages: Vec<DisplayMsg>,
    /// Scroll state: top line index (from top of content). Managed via Cell for render access.
    scroll_line: Cell<usize>,
    auto_scroll: Cell<bool>,

    editor: Editor,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,

    is_streaming: bool,
    pending_text: Option<String>,
    pending_thinking: Option<String>,

    hide_thinking: bool,
    /// Tool output collapsed by default (matches pi). Ctrl+O to expand.
    tool_output_collapsed: bool,
    show_help: bool,

    should_quit: bool,
    last_usage: Option<crate::types::Usage>,

    /// Handle to abort the running agent task (for Ctrl+C interrupt).
    agent_abort: Option<tokio::task::AbortHandle>,

    /// History: index into conversation user messages for arrow-key recall.
    /// None = not navigating history; Some(i) = pointing at conversation[i].
    history_index: Option<usize>,
    /// Session for persistence (wrapped in Option for ownership).
    session: Option<SessionManager>,

    /// Frame counter for spinner animation.
    frame_count: u64,

    // ── Model selector state ──
    available_models: Vec<String>,
    show_model_selector: bool,
    model_search: String,
    model_selector_selection: usize,
}

// ── Public entry point ─────────────────────────────────────────────

pub async fn run(config: TuiConfig, session: SessionManager) -> anyhow::Result<()> {
    ratatui::crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    ratatui::crossterm::execute!(
        stdout,
        ratatui::crossterm::terminal::EnterAlternateScreen,
        ratatui::crossterm::cursor::Show,
        ratatui::crossterm::cursor::SetCursorStyle::BlinkingBlock
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let result = run_app(&mut terminal, config, session);

    ratatui::crossterm::terminal::disable_raw_mode()?;
    ratatui::crossterm::execute!(
        terminal.backend_mut(),
        ratatui::crossterm::terminal::LeaveAlternateScreen
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
        scroll_line: Cell::new(0),
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
                Event::Key(key) => handle_key(&mut app, key),
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

// ── Render ─────────────────────────────────────────────────────────

fn ui(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if app.show_model_selector {
        render_model_selector(frame, area, app);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),                      // messages
            Constraint::Length(working_height(app)), // working indicator
            Constraint::Length(editor_height(app)),  // editor
            Constraint::Length(2),                   // footer (2 lines: cwd + stats)
        ])
        .split(area);

    render_messages(frame, chunks[0], app);
    render_working(frame, chunks[1], app);
    render_editor(frame, chunks[2], app);
    render_footer(frame, chunks[3], app);
}

fn working_height(app: &App) -> u16 {
    if app.is_streaming { 1 } else { 0 }
}

fn editor_height(app: &App) -> u16 {
    let lines = app.editor.lines_raw().len().max(1);
    (lines + 2).clamp(3, 10) as u16
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

    if app.show_help {
        lines.extend(help_lines(app));
        return Text::from(lines);
    }

    let th = &app.theme;

    for msg in &app.messages {
        match msg {
            DisplayMsg::User(text) => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                for line in text.lines() {
                    lines.push(
                        Line::from(Span::styled(format!(" {line}"), th.user_msg_style()))
                            .style(th.user_msg_style()),
                    );
                }
            }
            DisplayMsg::AssistantText(text) => {
                for line in text.lines() {
                    if line.is_empty() {
                        lines.push(Line::from(""));
                    } else {
                        lines.push(Line::from(line.to_string()));
                    }
                }
            }
            DisplayMsg::Thinking(text) => {
                if app.hide_thinking {
                    if !lines.is_empty()
                        && !lines.last().is_none_or(|l| {
                            l.spans.is_empty() || l.spans.iter().all(|s| s.content.is_empty())
                        })
                    {
                        lines.push(Line::from(""));
                    }
                    lines.push(
                        Line::from(Span::styled(" Thinking…", th.thinking_label_style()))
                            .style(th.thinking_label_style()),
                    );
                    continue;
                }
                for line in text.lines() {
                    lines.push(
                        Line::from(Span::styled(format!(" {line}"), th.thinking_style()))
                            .style(th.thinking_style()),
                    );
                }
            }
            DisplayMsg::ToolCall { name, args, .. } => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                let truncated = if args.len() > 80 {
                    format!("{}…", &args[..80])
                } else {
                    args.clone()
                };
                let line_text = if truncated == "{}" || truncated.is_empty() {
                    format!(" {name} ")
                } else {
                    format!(" {name}  {truncated}")
                };
                lines.push(
                    Line::from(Span::styled(line_text, th.tool_pending_style()))
                        .style(th.tool_pending_style()),
                );
            }
            DisplayMsg::ToolResult {
                content, is_error, ..
            } => {
                let style = if *is_error {
                    th.tool_error_style()
                } else {
                    th.tool_success_style()
                };
                if app.tool_output_collapsed {
                    let first = content.lines().next().unwrap_or("");
                    let truncated: String = first.chars().take(120).collect();
                    let suffix = if first.len() > 120 { "…" } else { "" };
                    lines.push(
                        Line::from(Span::styled(format!(" {truncated}{suffix}"), style))
                            .style(style),
                    );
                } else {
                    for line_content in content.lines() {
                        let truncated: String = line_content.chars().take(140).collect();
                        lines.push(
                            Line::from(Span::styled(format!(" {truncated}"), style)).style(style),
                        );
                    }
                }
            }
            DisplayMsg::Info(text) => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(text.clone(), th.dim_style())));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Type a message and press Enter to send.",
            th.dim_style(),
        )));
    }

    Text::from(lines)
}

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let th = &app.theme;
    let dim = th.dim_style();
    let accent = th.accent_style();

    let mut lines = vec![
        Line::from(Span::styled("Keyboard Shortcuts", accent)),
        Line::from(""),
        Line::from(Span::styled("  Enter              Submit message", dim)),
        Line::from(Span::styled("  Shift+Enter        Newline", dim)),
        Line::from(Span::styled(
            "  Ctrl+C             Interrupt / clear editor",
            dim,
        )),
        Line::from(Span::styled(
            "  Ctrl+D             Quit (empty) / interrupt",
            dim,
        )),
        Line::from(Span::styled("  Escape             Clear editor", dim)),
        Line::from(Span::styled(
            "  Ctrl+L             Open model selector",
            dim,
        )),
        Line::from(Span::styled("  !<command>         Run bash inline", dim)),
        Line::from(Span::styled(
            "  !!<command>        Run bash (excluded from context)",
            dim,
        )),
        Line::from(Span::styled("  Ctrl+T             Toggle thinking", dim)),
        Line::from(Span::styled("  Ctrl+O             Toggle tool output", dim)),
        Line::from(Span::styled("  F1                 Show this help", dim)),
        Line::from(Span::styled(
            "  ↑↓                 History (editor empty)",
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
    let text = Text::from(app.editor.text());
    let block = app.editor.block();
    let para = Paragraph::new(text).block(block.clone());
    frame.render_widget(para, area);

    // Hardware cursor via Frame (no custom software cursor)
    let inner = block.inner(area);
    let (row, col) = app.editor.cursor();
    let cx = inner.x + col.min(inner.width.saturating_sub(1) as usize) as u16;
    let cy = inner.y + row.min(inner.height.saturating_sub(1) as usize) as u16;
    frame.set_cursor_position((cx, cy));
}

fn render_working(frame: &mut Frame, area: Rect, app: &App) {
    if !app.is_streaming {
        return;
    }
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let idx = (app.frame_count as usize / 8) % spinner.len();
    let text = Span::styled(
        format!(" {} Working…", spinner[idx]),
        app.theme.working_style(),
    );
    frame.render_widget(Paragraph::new(Line::from(text)), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let w = area.width as usize;

    // ── Line 1: working directory + git branch ──
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd_str = app.cwd.to_str().unwrap_or("?");
    let cwd_display = if !home.is_empty() && cwd_str.starts_with(&home) {
        format!("~{}", &cwd_str[home.len()..])
    } else {
        cwd_str.to_string()
    };
    let cwd_line = if let Some(ref branch) = app.git_branch {
        format!("{cwd_display} ({branch})")
    } else {
        cwd_display
    };
    let cwd_line = truncate_str(&cwd_line, w);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(cwd_line, th.footer_style()))),
        Rect::new(area.x, area.y, area.width, 1),
    );

    // ── Line 2: tokens + model ──
    let tokens_str = app.last_usage.as_ref().map_or(String::new(), |u| {
        let input = u.input_tokens.unwrap_or(0);
        let output = u.output_tokens.unwrap_or(0);
        format!("↑{} ↓{}", fmt_tokens(input), fmt_tokens(output))
    });

    let model_display = app.model.replace("opencode_go::", "");
    let thinking_str = app
        .thinking_level
        .as_deref()
        .filter(|t| *t != "off" && *t != "none")
        .map(|t| format!(" • {t}"))
        .unwrap_or_default();

    // Build line: tokens left, model right (pi-style)
    let model_str = if app.model.starts_with("opencode_go::") {
        format!("(opencode-go) {model_display}{thinking_str}")
    } else {
        format!("{model_display}{thinking_str}")
    };

    if tokens_str.is_empty() {
        let line = pad_right(&model_str, w);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(line, th.footer_style()))),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    } else {
        let min_pad = 2;
        let left = &tokens_str;
        let right = &model_str;
        let left_w = left.chars().count();
        let right_w = right.chars().count();
        let line = if left_w + min_pad + right_w <= w {
            let padding = w - left_w - right_w;
            format!("{left}{}{right}", " ".repeat(padding))
        } else {
            let available = w.saturating_sub(left_w + min_pad);
            if available > 0 {
                let truncated = truncate_str(right, available);
                let padding = w - left_w - truncated.chars().count();
                format!("{left}{}{truncated}", " ".repeat(padding))
            } else {
                left.to_string()
            }
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(line, th.footer_style()))),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
}

fn fmt_tokens(n: i32) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n < 1_000_000 {
        format!("{}k", n / 1000)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

fn pad_right(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(width - len), s)
    }
}

// ── Model selector ─────────────────────────────────────────────────

/// Filter available models by search query (case-insensitive substring match).
fn filter_models<'a>(models: &'a [String], query: &str) -> Vec<&'a str> {
    if query.is_empty() {
        return models.iter().map(|s| s.as_str()).collect();
    }
    let lower = query.to_lowercase();
    models
        .iter()
        .filter(|m| m.to_lowercase().contains(&lower))
        .map(|s| s.as_str())
        .collect()
}

/// Render the model selector as a centered overlay.
fn render_model_selector(frame: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;

    let filtered = filter_models(&app.available_models, &app.model_search);

    // Compute overlay dimensions
    let overlay_width = (area.width as usize).min(60) as u16;
    let overlay_height = (area.height as usize).min(filtered.len() + 6).max(8) as u16;
    let overlay_x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let overlay_y = area.y + (area.height.saturating_sub(overlay_height)) / 2;

    let overlay_area = Rect::new(overlay_x, overlay_y, overlay_width, overlay_height);

    // Dim background
    let bg = Paragraph::new(Text::raw("\n".repeat(area.height as usize))).style(
        Style::default()
            .bg(Color::Rgb(0x00, 0x00, 0x00))
            .fg(Color::Rgb(0x00, 0x00, 0x00)),
    );
    frame.render_widget(bg, area);

    // Overlay border
    let block = Block::default()
        .title(" Select Model ")
        .title_alignment(ratatui::layout::Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.accent));
    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    // Render content inside the overlay
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Search input line (hardware cursor handles blinking)
    let search_label = Span::styled("> ", Style::default().fg(th.accent));
    let search_value = Span::styled(app.model_search.clone() + " ", Style::default().fg(th.text));
    lines.push(Line::from(vec![search_label, search_value]));
    lines.push(Line::from(""));

    // Set hardware cursor at the end of the search input
    let cursor_col = inner.x + 2 + app.model_search.chars().count() as u16;
    let cursor_row = inner.y;
    frame.set_cursor_position((cursor_col, cursor_row));

    // Render visible models (scrolling)
    let max_visible = inner.height.saturating_sub(4) as usize;
    let selected = app.model_selector_selection;
    let start = selected.saturating_sub(max_visible / 2);
    let end = (start + max_visible).min(filtered.len());
    let start = end.saturating_sub(max_visible); // re-center

    for i in start..end {
        if i >= filtered.len() {
            break;
        }
        let model = filtered[i];
        let is_current = model == app.model || format!("opencode_go::{model}") == app.model;
        let is_selected = i == selected;

        let prefix = if is_selected { "→ " } else { "  " };
        let check = if is_current { " ✓" } else { "" };

        let style = if is_selected {
            Style::default()
                .fg(th.accent)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else if is_current {
            Style::default().fg(th.success)
        } else {
            Style::default().fg(th.text)
        };

        lines.push(Line::from(Span::styled(
            format!("{prefix}{model}{check}"),
            style,
        )));
    }

    // Empty state
    if filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No matching models",
            Style::default().fg(th.dim),
        )));
    }

    // Hint line
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter to select · Esc to cancel · Type to filter",
        Style::default().fg(th.dim),
    )));

    let text = Text::from(lines);
    let para = Paragraph::new(text).style(Style::default().bg(Color::Rgb(0x1e, 0x1e, 0x2e)));
    frame.render_widget(para, inner);
}

// ── History recall ────────────────────────────────────────────────

/// Recall a previous user message into the editor (pi-style arrow-key history).
/// direction: -1 for older, 1 for newer.
fn recall_history(app: &mut App, direction: isize) {
    // Collect user messages from conversation (newest last)
    let user_messages: Vec<&str> = app
        .conversation
        .iter()
        .filter_map(|m| {
            if m.role == crate::types::Role::User && !m.content.is_empty() {
                Some(m.content.as_str())
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
        app.editor = create_editor(app);
        app.history_index = None;
    } else {
        let mut editor = create_editor(app);
        editor.set_text(user_messages[new_index]);
        app.editor = editor;
        app.history_index = Some(new_index);
    }
}

// ── Scroll helpers ─────────────────────────────────────────────────

fn scroll_up(app: &mut App, lines: usize) {
    app.auto_scroll.set(false);
    let current = app.scroll_line.get();
    app.scroll_line.set(current.saturating_sub(lines));
}

fn scroll_down(app: &mut App, lines: usize) {
    if app.auto_scroll.get() {
        return;
    }
    let current = app.scroll_line.get();
    app.scroll_line.set(current.saturating_add(lines));
}

// ── Keyboard handling ──────────────────────────────────────────────

fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // ── Model selector input mode ──
    if app.show_model_selector {
        handle_model_selector_key(app, key);
        return;
    }

    match key.code {
        // Tab: slash command autocomplete (handle both Tab and Char('\t'))
        KeyCode::Tab | KeyCode::Char('\t') => {
            if app.show_help {
                app.show_help = false;
                return;
            }
            let text = app.editor.text();
            if text.trim().starts_with('/') {
                handle_slash_completion(app, &text);
                return;
            }
            app.editor
                .handle_key(key.code, key.modifiers.contains(KeyModifiers::CONTROL));
        }
        // Ctrl+C: interrupt streaming, or clear editor (pi: app.interrupt)
        KeyCode::Char('c') if ctrl => {
            if app.show_help {
                app.show_help = false;
            } else if app.is_streaming {
                if let Some(handle) = app.agent_abort.take() {
                    handle.abort();
                }
                app.is_streaming = false;
                app.messages.push(DisplayMsg::Info("Aborted".to_string()));
                app.auto_scroll.set(true);
            } else {
                app.editor = create_editor(app);
                app.history_index = None;
            }
        }
        // Ctrl+D: quit when streaming or editor empty
        KeyCode::Char('d') if ctrl => {
            if app.show_help {
                app.show_help = false;
            } else if app.is_streaming {
                if let Some(handle) = app.agent_abort.take() {
                    handle.abort();
                }
                app.is_streaming = false;
                app.messages.push(DisplayMsg::Info("Aborted".to_string()));
                app.auto_scroll.set(true);
            } else if app.editor.is_empty() {
                app.should_quit = true;
            }
        }
        // Escape: close help or abort streaming (pi: app.interrupt)
        KeyCode::Esc => {
            if app.show_help {
                app.show_help = false;
            } else if app.is_streaming {
                if let Some(handle) = app.agent_abort.take() {
                    handle.abort();
                }
                app.is_streaming = false;
                app.messages.push(DisplayMsg::Info("Aborted".to_string()));
                app.auto_scroll.set(true);
            }
        }
        // Ctrl+T: toggle thinking (pi-style, persisted to settings.json)
        KeyCode::Char('t') if ctrl => {
            app.hide_thinking = !app.hide_thinking;
            if let Ok(mut settings) = crate::settings::Settings::load(&app.cwd) {
                settings.hide_thinking = Some(app.hide_thinking);
                let _ = settings.save();
            }
            app.messages.push(DisplayMsg::Info(format!(
                "Thinking blocks: {}",
                if app.hide_thinking {
                    "hidden"
                } else {
                    "visible"
                }
            )));
            app.auto_scroll.set(true);
        }
        // Ctrl+L: open model selector (pi-style)
        KeyCode::Char('l') if ctrl => {
            app.show_model_selector = true;
            app.model_search.clear();
            app.model_selector_selection = app
                .available_models
                .iter()
                .position(|m| m == &app.model || format!("opencode_go::{m}") == app.model)
                .unwrap_or(0);
        }
        // Ctrl+O: toggle tool output (pi-style, persisted to settings.json)
        KeyCode::Char('o') if ctrl => {
            app.tool_output_collapsed = !app.tool_output_collapsed;
            if let Ok(mut settings) = crate::settings::Settings::load(&app.cwd) {
                settings.collapse_tool_output = Some(app.tool_output_collapsed);
                let _ = settings.save();
            }
            app.messages.push(DisplayMsg::Info(format!(
                "Tool output: {}",
                if app.tool_output_collapsed {
                    "collapsed"
                } else {
                    "expanded"
                }
            )));
            app.auto_scroll.set(true);
        }
        // Ctrl+J: newline (terminal-independent)
        KeyCode::Char('j') if ctrl => {
            app.editor.handle_key(KeyCode::Enter, false);
        }
        // F1: show help
        KeyCode::F(1) => {
            app.show_help = !app.show_help;
        }
        // Shift+Enter / Alt+Enter / Ctrl+Enter: newline
        KeyCode::Enter if shift || alt || ctrl => {
            app.editor.handle_key(KeyCode::Enter, false);
        }
        // Enter (no modifiers): submit
        KeyCode::Enter => {
            let text = app.editor.text();
            let trimmed = text.trim();
            if !trimmed.is_empty() && !app.is_streaming {
                submit_message(app, trimmed.to_string());
            }
        }
        // Arrow up: recall previous message when editor is empty
        KeyCode::Up if app.editor.is_empty() && !app.is_streaming => {
            recall_history(app, -1);
        }
        // Arrow down: recall next message when editor is empty
        KeyCode::Down if app.editor.is_empty() && !app.is_streaming => {
            recall_history(app, 1);
        }
        // PageUp/PageDown → scroll messages
        KeyCode::PageUp => scroll_up(app, 10),
        KeyCode::PageDown => scroll_down(app, 10),
        // Everything else: pass to editor
        _ => {
            app.editor
                .handle_key(key.code, key.modifiers.contains(KeyModifiers::CONTROL));
        }
    }
}

/// Handle keyboard input when the model selector is active.
fn handle_model_selector_key(app: &mut App, key: KeyEvent) {
    let filtered = filter_models(&app.available_models, &app.model_search);
    let max_index = filtered.len().saturating_sub(1);

    match key.code {
        KeyCode::Esc => {
            app.show_model_selector = false;
            app.model_search.clear();
        }
        KeyCode::Enter => {
            if app.model_selector_selection < filtered.len() {
                let selected = filtered[app.model_selector_selection].to_string();
                app.show_model_selector = false;
                app.model_search.clear();
                app.model = selected.clone();
                if let Ok(mut settings) = crate::settings::Settings::load(&app.cwd) {
                    settings.default_model = Some(selected.clone());
                    let _ = settings.save();
                }
                app.messages.push(DisplayMsg::Info(format!(
                    "Model: {}",
                    selected.replace("opencode_go::", "")
                )));
                app.auto_scroll.set(true);
            }
        }
        KeyCode::Up => {
            if !filtered.is_empty() {
                app.model_selector_selection = if app.model_selector_selection == 0 {
                    max_index
                } else {
                    app.model_selector_selection - 1
                };
            }
        }
        KeyCode::Down => {
            if !filtered.is_empty() {
                app.model_selector_selection = if app.model_selector_selection >= max_index {
                    0
                } else {
                    app.model_selector_selection + 1
                };
            }
        }
        // Tab cycles to top/bottom
        KeyCode::Tab => {
            if !filtered.is_empty() {
                app.model_selector_selection = 0;
            }
        }
        // Home / End jump to first / last
        KeyCode::Home => {
            if !filtered.is_empty() {
                app.model_selector_selection = 0;
            }
        }
        KeyCode::End => {
            app.model_selector_selection = max_index;
        }
        // Backspace: delete last char from search
        KeyCode::Backspace => {
            app.model_search.pop();
            app.model_selector_selection = app.model_selector_selection.min(max_index);
        }
        // Char: add to search, reset selection
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.model_search.push(c);
            app.model_selector_selection = 0;
        }
        _ => {}
    }
}

/// Handle Tab autocomplete for slash commands.
/// Tab only completes in-place, never executes. Execution happens on Enter.
fn handle_slash_completion(app: &mut App, text: &str) {
    let trimmed = text.trim();
    let commands = collect_commands(app);

    let space_idx = trimmed.find(' ');
    match space_idx {
        None => {
            let prefix = trimmed.trim_start_matches('/');
            let lower = prefix.to_lowercase();
            let matches: Vec<&&SlashCommand> = commands
                .iter()
                .filter(|c| c.name.to_lowercase().starts_with(&lower))
                .collect();

            if matches.len() == 1 {
                let new_text = format!("/{} ", matches[0].name);
                set_editor_text(app, &new_text);
            } else if matches.len() > 1 {
                let common =
                    common_prefix(&matches.iter().map(|c| c.name.as_str()).collect::<Vec<_>>());
                if common.len() > prefix.len() {
                    let new_text = format!("/{}", common);
                    set_editor_text(app, &new_text);
                } else {
                    let names: Vec<String> =
                        matches.iter().map(|c| format!("/{}", c.name)).collect();
                    app.messages.push(DisplayMsg::Info(names.join("  ")));
                    app.auto_scroll.set(true);
                }
            }
        }
        Some(idx) => {
            let cmd_name = trimmed[..idx].trim_start_matches('/');
            let arg_prefix = &trimmed[idx..].trim();

            if let Some(cmd) = commands.iter().find(|c| c.name == cmd_name) {
                let completions = cmd.handler.argument_completions(arg_prefix);
                if completions.len() == 1 {
                    let new_text = format!("/{} {}", cmd_name, completions[0].value);
                    set_editor_text(app, &new_text);
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

fn set_editor_text(app: &mut App, text: &str) {
    let mut editor = create_editor(app);
    editor.set_text(text);
    app.editor = editor;
}

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

/// Parse a ! or !! command from trimmed input.
/// Returns Some((command, is_excluded)) if input starts with ! or !! and has content.
fn parse_bang_command(input: &str) -> Option<(&str, bool)> {
    if let Some(rest) = input.strip_prefix("!!") {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd, true))
        }
    } else if let Some(rest) = input.strip_prefix('!') {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd, false))
        }
    } else {
        None
    }
}

// ── Command result handling ────────────────────────────────────────

fn submit_message(app: &mut App, message: String) {
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
                    app.auto_scroll.set(true);
                    None
                } else {
                    app.messages.push(DisplayMsg::Info(format!(
                        "Unknown command: /{}. Type / for available commands.",
                        cmd_name
                    )));
                    app.editor = create_editor(app);
                    app.auto_scroll.set(true);
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

fn apply_command_result(app: &mut App, result: anyhow::Result<CommandResult>) {
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
    app.auto_scroll.set(true);
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

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;

    fn make_msg(role: Role, content: &str, is_error: bool) -> AgentMessage {
        let is_tool = role == Role::ToolResult;
        AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role,
            content: content.to_string(),
            tool_calls: vec![],
            tool_call_id: if is_tool {
                Some("tc1".to_string())
            } else {
                None
            },
            usage: None,
            is_error,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    #[test]
    fn test_session_messages_to_display_empty() {
        let result = session_messages_to_display(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_session_messages_to_display_user() {
        let msgs = vec![make_msg(Role::User, "hello", false)];
        let result = session_messages_to_display(&msgs);
        assert_eq!(result.len(), 1);
        match &result[0] {
            DisplayMsg::User(content) => assert_eq!(content, "hello"),
            other => panic!("Expected User, got {other:?}"),
        }
    }

    #[test]
    fn test_session_messages_to_display_assistant() {
        let msgs = vec![make_msg(Role::Assistant, "hi there", false)];
        let result = session_messages_to_display(&msgs);
        assert_eq!(result.len(), 1);
        match &result[0] {
            DisplayMsg::AssistantText(content) => assert_eq!(content, "hi there"),
            other => panic!("Expected AssistantText, got {other:?}"),
        }
    }

    #[test]
    fn test_session_messages_to_display_tool_result_success() {
        let msgs = vec![make_msg(Role::ToolResult, "file contents", false)];
        let result = session_messages_to_display(&msgs);
        assert_eq!(result.len(), 1);
        match &result[0] {
            DisplayMsg::ToolResult { content, is_error } => {
                assert!(content.contains("file contents"));
                assert!(content.starts_with('✓'));
                assert!(!is_error);
            }
            other => panic!("Expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn test_session_messages_to_display_tool_result_error() {
        let msgs = vec![make_msg(Role::ToolResult, "permission denied", true)];
        let result = session_messages_to_display(&msgs);
        match &result[0] {
            DisplayMsg::ToolResult { content, is_error } => {
                assert!(content.starts_with('✗'));
                assert!(*is_error);
            }
            other => panic!("Expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn test_session_messages_to_display_mixed() {
        let msgs = vec![
            make_msg(Role::User, "question", false),
            make_msg(Role::Assistant, "answer", false),
            make_msg(Role::ToolResult, "tool output", false),
        ];
        let result = session_messages_to_display(&msgs);
        assert_eq!(result.len(), 3);
        assert!(matches!(&result[0], DisplayMsg::User(_)));
        assert!(matches!(&result[1], DisplayMsg::AssistantText(_)));
        assert!(matches!(&result[2], DisplayMsg::ToolResult { .. }));
    }
}
