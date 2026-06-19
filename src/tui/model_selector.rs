use super::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

pub(crate) fn filter_models<'a>(models: &'a [String], query: &str) -> Vec<&'a str> {
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
pub(crate) fn render_model_selector(frame: &mut Frame, area: Rect, app: &App) {
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
