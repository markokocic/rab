use crate::agent::{AgentEvent, LoopConfig, run_agent_loop};
use crate::extension::{AgentTool, CommandResult, Extension, SlashCommand};
use crate::provider::{Provider, ToolDef};
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

// ── Editor state ────────────────────────────────────────────────────

/// Minimal multi-line editor state.
struct Editor {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    block: Block<'static>,
}

impl Editor {
    fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            block: Block::default(),
        }
    }

    fn set_block(&mut self, block: Block<'static>) {
        self.block = block;
    }

    fn block(&self) -> &Block<'static> {
        &self.block
    }

    fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn lines(&self) -> &[String] {
        &self.lines
    }

    fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        self.cursor_row = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    fn insert_at_cursor(&mut self, s: &str) {
        let line = &mut self.lines[self.cursor_row];
        line.insert_str(self.cursor_col, s);
        self.cursor_col += s.len();
    }

    fn handle_key(&mut self, code: ratatui::crossterm::event::KeyCode, ctrl: bool) {
        match code {
            // ── Emacs navigation (ctrl) ──
            ratatui::crossterm::event::KeyCode::Char('a') if ctrl => {
                self.cursor_col = 0;
            }
            ratatui::crossterm::event::KeyCode::Char('e') if ctrl => {
                self.cursor_col = self.lines[self.cursor_row].len();
            }
            ratatui::crossterm::event::KeyCode::Char('b') if ctrl => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                }
            }
            ratatui::crossterm::event::KeyCode::Char('f') if ctrl => {
                if self.cursor_col < self.lines[self.cursor_row].len() {
                    self.cursor_col += 1;
                } else if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = 0;
                }
            }
            ratatui::crossterm::event::KeyCode::Char('p') if ctrl => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            ratatui::crossterm::event::KeyCode::Char('n') if ctrl => {
                if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            // ── Emacs editing (ctrl) ──
            ratatui::crossterm::event::KeyCode::Char('k') if ctrl => {
                let line = &mut self.lines[self.cursor_row];
                if self.cursor_col < line.len() {
                    line.truncate(self.cursor_col);
                } else if self.cursor_row + 1 < self.lines.len() {
                    self.lines.remove(self.cursor_row + 1);
                }
            }
            ratatui::crossterm::event::KeyCode::Char('u') if ctrl => {
                let line = &mut self.lines[self.cursor_row];
                line.drain(..self.cursor_col);
                self.cursor_col = 0;
            }
            ratatui::crossterm::event::KeyCode::Char('w') if ctrl => {
                let line = &mut self.lines[self.cursor_row];
                let start = line[..self.cursor_col]
                    .rfind(|c: char| c.is_ascii_punctuation() || c.is_ascii_whitespace())
                    .map(|i| i + 1)
                    .unwrap_or(0);
                if start < self.cursor_col {
                    line.drain(start..self.cursor_col);
                    self.cursor_col = start;
                }
            }
            // ── Regular character ──
            ratatui::crossterm::event::KeyCode::Char(c) if !ctrl => {
                self.insert_at_cursor(&c.to_string());
            }
            ratatui::crossterm::event::KeyCode::Backspace => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                    self.lines[self.cursor_row].remove(self.cursor_col);
                } else if self.cursor_row > 0 {
                    let rest = self.lines[self.cursor_row].clone();
                    self.lines.remove(self.cursor_row);
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                    self.lines[self.cursor_row].push_str(&rest);
                }
            }
            ratatui::crossterm::event::KeyCode::Delete => {
                let line = &mut self.lines[self.cursor_row];
                if self.cursor_col < line.len() {
                    line.remove(self.cursor_col);
                } else if self.cursor_row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cursor_row + 1);
                    self.lines[self.cursor_row].push_str(&next);
                }
            }
            ratatui::crossterm::event::KeyCode::Left => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                }
            }
            ratatui::crossterm::event::KeyCode::Right => {
                if self.cursor_col < self.lines[self.cursor_row].len() {
                    self.cursor_col += 1;
                } else if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = 0;
                }
            }
            ratatui::crossterm::event::KeyCode::Up => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            ratatui::crossterm::event::KeyCode::Down => {
                if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            ratatui::crossterm::event::KeyCode::Enter => {
                self.newline();
            }
            ratatui::crossterm::event::KeyCode::Home => {
                self.cursor_col = 0;
            }
            ratatui::crossterm::event::KeyCode::End => {
                self.cursor_col = self.lines[self.cursor_row].len();
            }
            _ => {}
        }
    }

    fn newline(&mut self) {
        let rest = self.lines[self.cursor_row][self.cursor_col..].to_string();
        self.lines[self.cursor_row].truncate(self.cursor_col);
        self.lines.insert(self.cursor_row + 1, rest);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }
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
         Shift+Enter  newline · Esc  clear · ↑↓  history"
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

    thinking_collapsed: bool,
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

    /// Frame counter for spinner animation.
    frame_count: u64,

    // ── Model selector state ──
    available_models: Vec<String>,
    show_model_selector: bool,
    model_search: String,
    model_selector_selection: usize,
}

// ── Public entry point ─────────────────────────────────────────────

pub async fn run(config: TuiConfig) -> anyhow::Result<()> {
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

    let result = run_app(&mut terminal, config);

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
        cwd: config.cwd.clone(),
        model: config.model.clone(),
        thinking_level: config.thinking_level.clone(),
        git_branch: config.git_branch.clone(),
        system_prompt: config.system_prompt,
        shared,
        provider: Arc::from(config.provider),
        theme: DARK,
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
        tool_output_collapsed: true,
        show_help: false,
        should_quit: false,
        last_usage: None,
        agent_abort: None,
        history_index: None,
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

fn create_editor() -> Editor {
    let mut editor = Editor::new();
    editor.set_block(
        Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(0x8a, 0xbe, 0xb7))),
    );
    editor
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
    let lines = app.editor.lines().len().max(1);
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
                    lines.push(Line::from(Span::styled(
                        format!(" {line}"),
                        th.user_msg_style(),
                    )));
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
                if app.thinking_collapsed {
                    if !lines.is_empty()
                        && !lines.last().is_none_or(|l| {
                            l.spans.is_empty() || l.spans.iter().all(|s| s.content.is_empty())
                        })
                    {
                        lines.push(Line::from(""));
                    }
                    lines.push(Line::from(Span::styled(
                        " Thinking…",
                        th.thinking_label_style(),
                    )));
                    continue;
                }
                for line in text.lines() {
                    lines.push(Line::from(Span::styled(
                        format!(" {line}"),
                        th.thinking_style(),
                    )));
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
                lines.push(Line::from(Span::styled(line_text, th.tool_pending_style())));
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
                    lines.push(Line::from(Span::styled(
                        format!(" {truncated}{suffix}"),
                        style,
                    )));
                } else {
                    for line in content.lines() {
                        let truncated: String = line.chars().take(140).collect();
                        lines.push(Line::from(Span::styled(format!(" {truncated}"), style)));
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
        app.editor = create_editor();
        app.history_index = None;
    } else {
        let mut editor = create_editor();
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
                app.editor = create_editor();
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
        // Ctrl+T: toggle thinking
        KeyCode::Char('t') if ctrl => {
            app.thinking_collapsed = !app.thinking_collapsed;
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
        // Ctrl+O: toggle tool output
        KeyCode::Char('o') if ctrl => {
            app.tool_output_collapsed = !app.tool_output_collapsed;
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
    let mut editor = create_editor();
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

// ── Command result handling ────────────────────────────────────────

fn submit_message(app: &mut App, message: String) {
    app.history_index = None;
    let trimmed = message.trim();

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
                    app.editor = create_editor();
                    app.auto_scroll.set(true);
                    None
                } else {
                    app.messages.push(DisplayMsg::Info(format!(
                        "Unknown command: /{}. Type / for available commands.",
                        cmd_name
                    )));
                    app.editor = create_editor();
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
                app.editor = create_editor();
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

    app.messages.push(DisplayMsg::User(trimmed.to_string()));
    app.auto_scroll.set(true);

    let prompt = AgentMessage::user(trimmed);
    app.conversation.push(prompt.clone());

    app.editor = create_editor();
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

        let _ = run_agent_loop(vec![prompt], &loop_config, &*provider, &mut emit).await;
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
