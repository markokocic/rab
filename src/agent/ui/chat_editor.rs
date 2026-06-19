use crate::tui::Theme;
use crate::tui::components::Editor;
use crate::tui::components::editor::{EditorOptions, EditorTheme};

/// Rab-specific chat editor that wraps the core tui::Editor.
///
/// Adds command awareness and integrates with rab's extension system.
pub struct ChatEditor {
    pub editor: Editor,
    /// Available slash command names for autocomplete.
    slash_commands: Vec<String>,
    /// CWD for file-path completion.
    cwd: std::path::PathBuf,
}

impl ChatEditor {
    pub fn new(theme: &dyn Theme, cwd: std::path::PathBuf) -> Self {
        let editor_theme = EditorTheme {
            text: {
                let theme_text = theme.fg("text", "").to_string();
                // Extract just the escape prefix for reuse
                Box::new(move |s| {
                    if !theme_text.is_empty() && theme_text.starts_with('\x1b') {
                        let prefix = &theme_text[..theme_text.len().saturating_sub(1)]; // remove trailing reset
                        format!("{}m{}", &prefix[2..prefix.len()], s)
                    } else {
                        s.to_string()
                    }
                })
            },
            cursor: Box::new(|s| format!("\x1b[7m{}\x1b[27m", s)),
            border: Box::new(move |s| format!("\x1b[38;2;138;190;183m{}\x1b[39m", s)),
            scroll_indicator: Box::new(move |s| format!("\x1b[38;2;128;128;128m{}\x1b[39m", s)),
            autocomplete_selected: Box::new(|s| {
                format!("\x1b[7m\x1b[38;2;138;190;183m{}\x1b[27m\x1b[39m", s)
            }),
            autocomplete_normal: Box::new(|s| format!("\x1b[38;2;128;128;128m{}\x1b[39m", s)),
        };

        let editor = Editor::new(
            editor_theme,
            EditorOptions {
                padding_x: 1,
                max_visible_lines: 10,
            },
        );

        Self {
            editor,
            slash_commands: Vec::new(),
            cwd,
        }
    }

    /// Set the available slash commands for autocomplete.
    pub fn set_slash_commands(&mut self, commands: Vec<String>) {
        self.slash_commands = commands;
    }

    /// Update the working directory.
    pub fn set_cwd(&mut self, cwd: std::path::PathBuf) {
        self.cwd = cwd;
    }

    /// Check if the current input should trigger autocomplete.
    pub fn get_autocomplete_suggestions(&self) -> Vec<String> {
        let text = self.editor.get_text();

        // Slash command completion
        if text.starts_with('/') {
            let cmd_part = text.trim_start_matches('/');
            let matches: Vec<String> = self
                .slash_commands
                .iter()
                .filter(|c| c.starts_with(cmd_part))
                .cloned()
                .collect();
            return matches;
        }

        Vec::new()
    }
}
