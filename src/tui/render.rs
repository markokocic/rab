use super::app::App;
use super::display::{build_message_text, fmt_tokens, pad_right, truncate_str};
use super::model_selector::render_model_selector;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

pub(crate) fn ui(frame: &mut Frame, app: &App) {
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

pub(crate) fn working_height(app: &App) -> u16 {
    if app.is_streaming { 1 } else { 0 }
}

pub(crate) fn editor_height(app: &App) -> u16 {
    let lines = app.editor.lines_raw().len().max(1);
    (lines + 2).clamp(3, 10) as u16
}

pub(crate) fn render_messages(frame: &mut Frame, area: Rect, app: &App) {
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

pub(crate) fn render_editor(frame: &mut Frame, area: Rect, app: &App) {
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

pub(crate) fn render_working(frame: &mut Frame, area: Rect, app: &App) {
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

pub(crate) fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
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
