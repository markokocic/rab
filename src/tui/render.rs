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

/// Resolve the scroll position from the current state.
/// Returns (scroll_line, new_auto_scroll).
pub(crate) fn resolve_scroll(
    total_lines: usize,
    viewport: usize,
    scroll_offset: usize,
    auto_scroll: bool,
) -> (usize, bool) {
    let max_scroll = total_lines.saturating_sub(viewport);

    if auto_scroll {
        (max_scroll, true)
    } else {
        let clamped = scroll_offset.min(max_scroll);
        let snapped = clamped >= max_scroll;
        (clamped, snapped)
    }
}

pub(crate) fn render_messages(frame: &mut Frame, area: Rect, app: &App) {
    let text = build_message_text(app);
    let total_lines = text.lines.len().saturating_sub(1);
    let viewport = area.height.saturating_sub(1) as usize;

    let (scroll, new_auto) = resolve_scroll(
        total_lines,
        viewport,
        app.scroll_offset.get(),
        app.auto_scroll.get(),
    );
    app.scroll_offset.set(scroll);
    app.auto_scroll.set(new_auto);

    let para = Paragraph::new(text).scroll((scroll as u16, 0));
    frame.render_widget(para, area);
}

pub(crate) fn render_editor(frame: &mut Frame, area: Rect, app: &App) {
    let block = app.editor.block();
    let inner = block.inner(area);
    let max_text = inner.height.max(1) as usize;
    let render = app.editor.render_with_max(inner.width, max_text);

    // Render editor text
    let text_lines: Vec<Line<'static>> = render
        .text_lines
        .iter()
        .map(|s| Line::from(s.to_string()))
        .collect();
    let text = Text::from(text_lines);
    let para = Paragraph::new(text).block(block.clone());
    frame.render_widget(para, area);

    // Hardware cursor position (visual coordinates from render_with_max)
    let cx = inner.x + render.cursor_col.min(inner.width.saturating_sub(1));
    let cy = inner.y + render.cursor_row.min(inner.height.saturating_sub(1));
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

#[cfg(test)]
mod tests {
    use super::resolve_scroll;

    #[test]
    fn auto_scroll_follows_bottom() {
        // Content fits exactly — max_scroll = 0, scroll stays at 0
        let (scroll, auto) = resolve_scroll(5, 5, 0, true);
        assert_eq!(scroll, 0);
        assert!(auto);

        // Content overflows — max_scroll = 3, auto_scroll snaps to it
        let (scroll, auto) = resolve_scroll(10, 5, 0, true);
        assert_eq!(scroll, 5);
        assert!(auto);
    }

    #[test]
    fn manual_scroll_stays_put() {
        // User scrolled up to line 3, content overflows
        let (scroll, auto) = resolve_scroll(20, 5, 3, false);
        assert_eq!(scroll, 3);
        assert!(!auto);
    }

    #[test]
    fn manual_scroll_clamped_to_max() {
        // scroll_offset beyond max_scroll is clamped to bottom, snaps to auto
        let (scroll, auto) = resolve_scroll(10, 5, 100, false);
        assert_eq!(scroll, 5);
        assert!(auto); // clamped to bottom → auto re-engages
    }

    #[test]
    fn snap_to_bottom_when_scrolled_to_end() {
        // User manually scrolled to exactly the bottom
        let (scroll, auto) = resolve_scroll(10, 5, 5, false);
        assert_eq!(scroll, 5);
        assert!(auto); // snaps back to auto
    }

    #[test]
    fn snap_when_content_shrinks_below_viewport() {
        // Content shrunk from large to below viewport — old scroll_offset
        // no longer exists, clamped to 0, snaps to auto
        let (scroll, auto) = resolve_scroll(3, 5, 50, false);
        assert_eq!(scroll, 0);
        assert!(auto);
    }

    #[test]
    fn auto_scroll_ignores_offset() {
        // offset is ignored when auto_scroll is true
        let (scroll, auto) = resolve_scroll(100, 5, 42, true);
        assert_eq!(scroll, 95);
        assert!(auto);
    }

    #[test]
    fn empty_content() {
        let (scroll, auto) = resolve_scroll(0, 5, 0, true);
        assert_eq!(scroll, 0);
        assert!(auto);
    }

    #[test]
    fn manual_scroll_survives_content_shrink() {
        // Content shrinks but scroll_offset still within new bounds
        let (scroll, auto) = resolve_scroll(50, 5, 30, false);
        assert_eq!(scroll, 30);
        assert!(!auto);
    }

    #[test]
    fn manual_scroll_up_then_content_grows() {
        // User scrolled up to line 5 when max_scroll was 10
        // Content grows, max_scroll becomes 15
        // scroll_offset should stay at 5, auto remains false
        let (scroll, auto) = resolve_scroll(20, 5, 5, false);
        assert_eq!(scroll, 5);
        assert!(!auto);
    }
}
