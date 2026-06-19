use crate::tui::Theme;

/// Rab's concrete color theme — emits ANSI escape codes directly.
/// Color palette matches pi's dark theme exactly.
pub struct RabTheme;

impl Theme for RabTheme {
    fn fg(&self, color: &str, text: &str) -> String {
        let code = match color {
            "text" => "38;2;212;212;212",
            "dim" => "38;2;102;102;102",
            "muted" => "38;2;128;128;128",
            "accent" => "38;2;138;190;183",
            "success" => "38;2;181;189;104",
            "error" => "38;2;204;102;102",
            "warning" => "38;2;255;255;0",
            // Message roles
            "user_message_text" => "38;2;212;212;212",
            "user_message_bg" => "48;2;52;53;65",
            "custom_message_text" => "38;2;212;212;212",
            "custom_message_label" => "38;2;138;190;183",
            // Tools
            "tool_title" => "38;2;212;212;212",
            "tool_output" => "38;2;128;128;128",
            "tool_pending_bg" => "48;2;40;40;50",
            "tool_success_bg" => "48;2;40;50;40",
            "tool_error_bg" => "48;2;60;40;40",
            // Thinking
            "thinking_text" => "38;2;128;128;128",
            "thinking_bg" => "48;2;44;44;54",
            // Borders
            "border" => "38;2;138;190;183",
            "border_accent" => "38;2;138;190;183",
            "border_muted" => "38;2;80;80;80",
            // Status
            "working" => "38;2;128;128;128",
            "idle" => "38;2;80;80;80",
            _ => "39", // default foreground
        };
        format!("\x1b[{}m{}\x1b[39m", code, text)
    }

    fn bg(&self, color: &str, text: &str) -> String {
        let code = match color {
            "selected_bg" => "48;2;52;53;65",
            "user_message_bg" => "48;2;52;53;65",
            "tool_pending_bg" => "48;2;40;40;50",
            "tool_success_bg" => "48;2;40;50;40",
            "tool_error_bg" => "48;2;60;40;40",
            "thinking_bg" => "48;2;44;44;54",
            _ => "49", // default background
        };
        format!("\x1b[{}m{}\x1b[49m", code, text)
    }

    fn bold(&self, text: &str) -> String {
        format!("\x1b[1m{}\x1b[22m", text)
    }
}

/// Helper functions for direct use in UI components.
impl RabTheme {
    pub fn accent(&self, text: &str) -> String {
        self.fg("accent", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.fg("dim", text)
    }

    pub fn muted(&self, text: &str) -> String {
        self.fg("muted", text)
    }

    pub fn success(&self, text: &str) -> String {
        self.fg("success", text)
    }

    pub fn error(&self, text: &str) -> String {
        self.fg("error", text)
    }

    pub fn text(&self, text: &str) -> String {
        self.fg("text", text)
    }

    pub fn border(&self, text: &str) -> String {
        self.fg("border", text)
    }

    /// Apply user message background.
    pub fn user_msg_bg(&self, text: &str) -> String {
        self.bg("user_message_bg", text)
    }

    /// Apply thinking block background.
    pub fn thinking_bg(&self, text: &str) -> String {
        self.bg("thinking_bg", text)
    }

    /// Bold wrapper.
    pub fn bold_text(&self, text: &str) -> String {
        self.bold(text)
    }
}
