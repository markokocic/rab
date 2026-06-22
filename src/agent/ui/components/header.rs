use crate::agent::ui::theme::current_theme;
use crate::tui::Component;

/// Header component matching pi's ExpandableText startup header.
/// Shows logo + keybinding hints in compact and expanded modes.
pub struct HeaderComponent {
    expanded: bool,
    cached_lines: std::cell::RefCell<Option<Vec<String>>>,
}

impl HeaderComponent {
    pub fn new() -> Self {
        Self {
            expanded: false,
            cached_lines: std::cell::RefCell::new(None),
        }
    }

    #[allow(unused_variables)]
    fn build_lines(&self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let logo = theme.bold(&theme.fg("accent", "rab"));
        drop(theme);

        if self.expanded {
            // Expanded: show keybinding hints
            let hint = |key: &str, desc: &str| -> String {
                current_theme().fg("dim", &format!("  {} — {}", key, desc))
            };
            vec![
                logo,
                String::new(),
                hint("Esc", "abort / clear editor"),
                hint("Ctrl+C twice", "exit"),
                hint("Ctrl+D", "exit (empty editor)"),
                hint("Up/Down", "history"),
                hint("Tab", "autocomplete"),
                hint("Ctrl+O", "toggle expand"),
                hint("Ctrl+T", "toggle thinking"),
                hint("Ctrl+P", "cycle thinking level"),
                hint("Ctrl+N / Ctrl+Shift+N", "cycle models"),
                hint("Alt+Enter", "queue follow-up"),
                hint("Alt+Up", "dequeue messages"),
                hint("Ctrl+E", "external editor"),
                hint("/", "commands"),
                hint("!", "run bash"),
                String::new(),
                current_theme().fg("dim", "Pi can explain its features — just ask."),
                String::new(),
            ]
        } else {
            // Compact: single line with key hints
            vec![logo, String::new()]
        }
    }
}

impl Default for HeaderComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for HeaderComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        *self.cached_lines.borrow_mut() = None;
    }

    fn render(&self, width: usize) -> Vec<String> {
        if let Some(ref cached) = *self.cached_lines.borrow() {
            return cached.clone();
        }
        let lines = self.build_lines(width);
        *self.cached_lines.borrow_mut() = Some(lines.clone());
        lines
    }

    fn invalidate(&mut self) {
        *self.cached_lines.borrow_mut() = None;
    }
}
