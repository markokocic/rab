use crossterm::event::KeyEvent;

use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::keybindings;
use crate::tui::keybindings::get_keybindings;
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Helper: get the display key text for an action (matches pi's keyText).
fn key_text(action_id: &str) -> String {
    let keys = keybindings::get_keybindings().get_keys(action_id);
    if keys.is_empty() {
        String::new()
    } else {
        keys[0].clone()
    }
}

/// Format a key hint line: `<dim>key</dim><muted> description</muted>` (matches pi's keyHint).
fn key_hint(action_id: &str, description: &str) -> String {
    let kt = key_text(action_id);
    if kt.is_empty() {
        return String::new();
    }
    let theme = current_theme();
    let key_part = theme.fg_key(ThemeKey::Dim, &kt);
    let desc_part = theme.fg_key(ThemeKey::Muted, &format!(" {}", description));
    format!("{}{}", key_part, desc_part)
}

/// Format a raw key hint: `<dim>raw_key</dim><muted> description</muted>` (matches pi's rawKeyHint).
fn raw_key_hint(key: &str, description: &str) -> String {
    let theme = current_theme();
    let key_part = theme.fg_key(ThemeKey::Dim, key);
    let desc_part = theme.fg_key(ThemeKey::Muted, &format!(" {}", description));
    format!("{}{}", key_part, desc_part)
}

/// Header component matching pi's ExpandableText startup header.
/// Shows logo, keybinding hints in compact/expanded modes, and onboarding text.
pub struct HeaderComponent {
    expanded: bool,
    cached_lines: Option<Vec<String>>,
}

impl HeaderComponent {
    pub fn new() -> Self {
        Self {
            expanded: false,
            cached_lines: None,
        }
    }

    fn build_lines(&self, _width: usize) -> Vec<String> {
        let logo = {
            let theme = current_theme();
            format!(
                "{}{}",
                theme.bold(&theme.fg_key(ThemeKey::Accent, "rab")),
                theme.fg_key(ThemeKey::Dim, &format!(" v{}", VERSION)),
            )
        };

        if self.expanded {
            // Expanded: full keybinding hints (matching pi's expandedInstructions)
            let mut lines: Vec<String> = Vec::new();
            lines.push(logo);
            lines.push(String::new());

            lines.push(key_hint("app.interrupt", "to interrupt"));
            lines.push(key_hint("app.clear", "to clear"));
            lines.push(raw_key_hint(
                &format!("{} twice", key_text("app.clear")),
                "to exit",
            ));
            lines.push(key_hint("app.exit", "to exit (empty)"));
            lines.push(key_hint("app.suspend", "to suspend"));
            lines.push(key_hint("tui.editor.deleteToLineEnd", "to delete to end"));
            lines.push(key_hint("app.thinking.cycle", "to cycle thinking level"));
            lines.push(raw_key_hint(
                &format!(
                    "{}/{}",
                    key_text("app.model.cycleForward"),
                    key_text("app.model.cycleBackward")
                ),
                "to cycle models",
            ));
            lines.push(key_hint("app.model.select", "to select model"));
            lines.push(key_hint("app.tools.expand", "to expand tools"));
            lines.push(key_hint("app.thinking.toggle", "to expand thinking"));
            lines.push(key_hint("app.editor.external", "for external editor"));
            lines.push(raw_key_hint("/", "for commands"));
            lines.push(raw_key_hint("!", "to run bash"));
            lines.push(raw_key_hint("!!", "to run bash (no context)"));
            lines.push(key_hint("app.message.followUp", "to queue follow-up"));
            lines.push(key_hint(
                "app.message.dequeue",
                "to edit all queued messages",
            ));
            lines.push(raw_key_hint("drop files", "to attach"));

            lines
        } else {
            // Compact: single-line key hints joined by " · " (matching pi's compactInstructions)
            let parts = [
                key_hint("app.interrupt", "interrupt"),
                raw_key_hint(
                    &format!("{}/{}", key_text("app.clear"), key_text("app.exit")),
                    "clear/exit",
                ),
                raw_key_hint("/", "commands"),
                raw_key_hint("!", "bash"),
                key_hint("app.tools.expand", "more"),
            ];
            let separator = {
                let theme = current_theme();
                theme.fg_key(ThemeKey::Muted, " · ")
            };
            let compact_line = parts.join(&separator);

            let compact_onboarding = {
                let theme = current_theme();
                theme.fg(
                    "dim",
                    &format!(
                        "Press {} to show full startup help and loaded resources.",
                        key_text("app.tools.expand"),
                    ),
                )
            };

            vec![logo, compact_line, String::new(), compact_onboarding]
        }
    }
}

impl Default for HeaderComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for HeaderComponent {
    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();
        if kb.matches(key, keybindings::ACTION_APP_TOOLS_EXPAND) {
            self.expanded = !self.expanded;
            self.cached_lines = None;
            // Don't consume - let the app-level handler also process Ctrl+O
            // so tool messages and global state (tools_expanded) stay in sync.
            return false;
        }
        // Escape collapses expanded header
        if self.expanded && kb.matches(key, keybindings::ACTION_APP_ESCAPE) {
            self.expanded = false;
            self.cached_lines = None;
            return true;
        }
        false
    }

    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.cached_lines = None;
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        if let Some(ref cached) = self.cached_lines {
            return cached.clone();
        }
        let lines = self.build_lines(width);
        self.cached_lines = Some(lines.clone());
        lines
    }

    fn invalidate(&mut self) {
        self.cached_lines = None;
    }
}
