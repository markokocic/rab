use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::Style;
use crate::tui::components::Text;

/// Component for info/status messages — simple dim text line.
/// Matches pi's `showStatus()` which adds a bare `Text(theme.fg("dim", message), 1, 0)`
/// directly to the chat container — no extra Container or Spacer wrapping.
pub struct InfoMessageComponent {
    text: Text,
}

impl InfoMessageComponent {
    pub fn new(message: impl Into<String>) -> Self {
        let theme = current_theme();
        let dim_style = Style::new().fg(theme.fg_ansi(ThemeKey::Dim.as_str()).to_string());
        Self {
            text: Text::new(format!(" {}", message.into()), 1, 0, Some(dim_style)),
        }
    }
}

impl Component for InfoMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.text.render(width)
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
}
