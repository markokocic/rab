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

/// Format a resource section header like `[Context]` (matches pi's sectionHeader).
fn section_header(name: &str) -> String {
    let theme = current_theme();
    theme.fg_key(ThemeKey::MdHeading, &format!("[{}]", name))
}

/// Header component matching pi's ExpandableText startup header.
/// Shows logo, keybinding hints in compact/expanded modes, loaded resources,
/// and onboarding text.
pub struct HeaderComponent {
    expanded: bool,
    cached_lines: Option<Vec<String>>,
    /// Context file paths (AGENTS.md / CLAUDE.md) loaded for the session.
    context_files: Vec<String>,
    /// Skill names loaded for the session.
    skills: Vec<String>,
    /// Prompt template command names (e.g. "/explain", "/review").
    prompt_templates: Vec<String>,
    /// Extension names loaded for the session.
    extensions: Vec<String>,
    /// Custom theme names loaded for the session.
    themes: Vec<String>,
}

impl HeaderComponent {
    pub fn new() -> Self {
        Self {
            expanded: false,
            cached_lines: None,
            context_files: Vec::new(),
            skills: Vec::new(),
            prompt_templates: Vec::new(),
            extensions: Vec::new(),
            themes: Vec::new(),
        }
    }

    /// Create with initial expansion state (matching pi's getStartupExpansionState).
    pub fn new_with_expanded(expanded: bool) -> Self {
        Self {
            expanded,
            cached_lines: None,
            context_files: Vec::new(),
            skills: Vec::new(),
            prompt_templates: Vec::new(),
            extensions: Vec::new(),
            themes: Vec::new(),
        }
    }

    /// Set resource data for display in the header (pi-style loaded resources).
    pub fn set_resource_data(
        &mut self,
        context_files: Vec<String>,
        skills: Vec<String>,
        prompt_templates: Vec<String>,
        extensions: Vec<String>,
        themes: Vec<String>,
    ) {
        self.context_files = context_files;
        self.skills = skills;
        self.prompt_templates = prompt_templates;
        self.extensions = extensions;
        self.themes = themes;
        self.cached_lines = None;
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

        // Main onboarding text (matches pi: "Pi can explain its own features...")
        let onboarding = {
            let theme = current_theme();
            theme.fg(
                "dim",
                "rab can explain its own features and look up its docs. Ask it how to use or extend rab.",
            )
        };

        if self.expanded {
            // ── Expanded: full keybinding hints + resource sections + onboarding ──
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

            // ── Loaded resources sections (pi-style) ──
            if !self.context_files.is_empty() {
                lines.push(String::new());
                lines.push(section_header("Context"));
                let theme = current_theme();
                for cf in &self.context_files {
                    lines.push(theme.fg_key(ThemeKey::Dim, &format!("  {}", cf)));
                }
            }

            if !self.skills.is_empty() {
                lines.push(String::new());
                lines.push(section_header("Skills"));
                let theme = current_theme();
                for skill in &self.skills {
                    lines.push(theme.fg_key(ThemeKey::Dim, &format!("  {}", skill)));
                }
            }

            if !self.prompt_templates.is_empty() {
                lines.push(String::new());
                lines.push(section_header("Prompts"));
                let theme = current_theme();
                for tmpl in &self.prompt_templates {
                    lines.push(theme.fg_key(ThemeKey::Dim, &format!("  /{}", tmpl)));
                }
            }

            if !self.extensions.is_empty() {
                lines.push(String::new());
                lines.push(section_header("Extensions"));
                let theme = current_theme();
                for ext in &self.extensions {
                    lines.push(theme.fg_key(ThemeKey::Dim, &format!("  {}", ext)));
                }
            }

            if !self.themes.is_empty() {
                lines.push(String::new());
                lines.push(section_header("Themes"));
                let theme = current_theme();
                for t in &self.themes {
                    lines.push(theme.fg_key(ThemeKey::Dim, &format!("  {}", t)));
                }
            }

            // Onboarding text at the end (matches pi placement)
            if !self.context_files.is_empty()
                || !self.skills.is_empty()
                || !self.prompt_templates.is_empty()
                || !self.extensions.is_empty()
                || !self.themes.is_empty()
            {
                lines.push(String::new());
            }
            lines.push(onboarding);

            lines
        } else {
            // ── Compact: logo + compact hints + resource summary + onboarding ──
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

            // Build compact resource summary line (pi-style compact listing)
            let resource_parts: Vec<String> = {
                let mut parts = Vec::new();
                if !self.context_files.is_empty() {
                    parts.push(format!("Context: {}", self.context_files.join(", ")));
                }
                if !self.skills.is_empty() {
                    parts.push(format!("Skills: {}", self.skills.join(", ")));
                }
                if !self.prompt_templates.is_empty() {
                    parts.push(format!(
                        "Prompts: {}",
                        self.prompt_templates
                            .iter()
                            .map(|t| format!("/{}", t))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if !self.extensions.is_empty() {
                    parts.push(format!("Extensions: {}", self.extensions.join(", ")));
                }
                if !self.themes.is_empty() {
                    parts.push(format!("Themes: {}", self.themes.join(", ")));
                }
                parts
            };

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

            let mut result = vec![logo, compact_line, String::new()];

            // Resource summary line (between hints and onboarding, matching pi layout)
            if !resource_parts.is_empty() {
                let resource_line = {
                    let theme = current_theme();
                    theme.fg_key(ThemeKey::Dim, &resource_parts.join("  ·  "))
                };
                result.push(resource_line);
                result.push(String::new());
            }

            result.push(compact_onboarding);
            result.push(String::new());
            result.push(onboarding);

            result
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
