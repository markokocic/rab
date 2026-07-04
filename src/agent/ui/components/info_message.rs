use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::Container;
use crate::tui::components::Spacer;
use crate::tui::components::Text;

/// Component for info/status messages — simple dim text line.
/// Matches pi's `showStatus()` style with `theme.fg_key(ThemeKey::Dim, message)`.
/// Uses a Container with a Spacer internally for leading spacing (matching pi's
/// approach where each component manages its own vertical spacing).
pub struct InfoMessageComponent {
    container: Container,
}

impl InfoMessageComponent {
    pub fn new(message: impl Into<String>) -> Self {
        let theme = current_theme();
        let styled = theme.fg_key(ThemeKey::Dim, &format!(" {}", message.into()));
        let mut container = Container::new();
        container.add_child(std::boxed::Box::new(Spacer::new(1)));
        container.add_child(std::boxed::Box::new(Text::new(styled, 0, 0, None)));
        Self { container }
    }
}

impl Component for InfoMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
