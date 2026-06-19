use crate::tui::Component;
use crate::tui::Theme;
use crate::tui::util::visible_width;
use crate::ui::theme::RabTheme;

/// Help overlay showing available commands and keybindings.
pub struct HelpOverlay {
    theme: RabTheme,
    commands: Vec<(String, String)>,
}

impl HelpOverlay {
    pub fn new(theme: &RabTheme) -> Self {
        Self {
            theme: theme.clone(),
            commands: Vec::new(),
        }
    }

    pub fn set_commands(&mut self, commands: Vec<(String, String)>) {
        self.commands = commands;
    }
}

impl Component for HelpOverlay {
    fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let _w = width.saturating_sub(4);

        // Title
        lines.push(format!(
            "  {}",
            self.theme.bold(&self.theme.accent("Keyboard Shortcuts"))
        ));
        lines.push(String::new());

        let shortcuts = [
            ("Enter", "Submit message"),
            ("Ctrl+J", "Newline"),
            ("Ctrl+C", "Interrupt / clear editor"),
            ("Ctrl+D", "Quit (empty) / interrupt"),
            ("Escape", "Clear editor"),
            ("Ctrl+L", "Open model selector"),
            ("!<command>", "Run bash inline"),
            ("!!<command>", "Run bash (excluded from context)"),
            ("Ctrl+T", "Toggle thinking visibility"),
            ("Ctrl+O", "Toggle tool output"),
            ("F1", "Show this help"),
            ("↑↓", "History (editor empty)"),
            ("PgUp / PgDn", "Scroll messages"),
        ];

        for (key, desc) in &shortcuts {
            let line = format!(
                "  {:20} {}",
                self.theme.bold(&self.theme.accent(key)),
                self.theme.dim(desc)
            );
            lines.push(line);
        }

        // Slash commands
        if !self.commands.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "  {}",
                self.theme.bold(&self.theme.accent("Slash Commands"))
            ));
            lines.push(String::new());
            for (name, desc) in &self.commands {
                let line = format!(
                    "  /{:<19} {}",
                    self.theme.bold(&self.theme.accent(name)),
                    self.theme.dim(desc)
                );
                lines.push(line);
            }
        }

        lines.push(String::new());
        lines.push(self.theme.dim("  Press any key to close help."));

        // Pad all lines to width
        lines.iter_mut().for_each(|l| {
            let vw = visible_width(l);
            if vw < width {
                l.push_str(&" ".repeat(width - vw));
            }
        });

        lines
    }

    fn handle_input(&mut self, _key: &crossterm::event::KeyEvent) -> bool {
        // Any key closes help
        true
    }
}
