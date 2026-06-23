use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::components::Text;
/// Component for info/status messages — simple dim text line.
/// Matches pi's `showStatus()` style with `theme.fg_key(ThemeKey::Dim, message)`.
pub struct InfoMessageComponent {
    text: Text,
}

impl InfoMessageComponent {
    pub fn new(message: impl Into<String>) -> Self {
        let theme = current_theme();
        let styled = theme.fg_key(ThemeKey::Dim, &format!(" {}", message.into()));
        Self {
            text: Text::new(styled, 0, 0, None),
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
