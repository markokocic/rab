use crate::agent::ui::theme::RabTheme;
use crate::tui::Component;

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
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Reusable Text + Spacer for each section
        let push = |l: &mut Vec<String>, text: &str| {
            l.push(crate::tui::util::truncate_to_width(text, width, "", true));
        };

        push(
            &mut lines,
            &self.theme.bold(&self.theme.accent("  Keyboard Shortcuts")),
        );
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
            push(&mut lines, &line);
        }

        if !self.commands.is_empty() {
            lines.push(String::new());
            push(
                &mut lines,
                &self.theme.bold(&self.theme.accent("  Slash Commands")),
            );
            lines.push(String::new());
            for (name, desc) in &self.commands {
                let line = format!(
                    "  /{:<19} {}",
                    self.theme.bold(&self.theme.accent(name)),
                    self.theme.dim(desc)
                );
                push(&mut lines, &line);
            }
        }

        lines.push(String::new());
        push(
            &mut lines,
            &self.theme.dim("  Press any key to close help."),
        );

        lines
    }

    fn handle_input(&mut self, _key: &crossterm::event::KeyEvent) -> bool {
        // Any key closes help
        true
    }
}
