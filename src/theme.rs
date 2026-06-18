use ratatui::style::{Color, Modifier, Style};

/// Theme colors matching pi's dark theme exactly.
pub struct Theme {
    // ── Text ──
    pub text: Color,
    pub dim: Color,
    pub muted: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,

    // ── Chat messages ──
    pub user_msg_bg: Color,
    pub user_msg_text: Color,

    // ── Tool execution ──
    pub tool_pending_bg: Color,
    pub tool_success_bg: Color,
    pub tool_error_bg: Color,
    pub tool_title: Color,
    pub tool_output: Color,

    // ── Thinking ──
    pub thinking_text: Color,
    pub thinking_bg: Color, // background for thinking blocks and label

    // ── Editor ──
    pub editor_border: Color,

    // ── Footer / status ──
    pub footer_text: Color,
    pub streaming_dot: Color,
    pub idle_dot: Color,
    pub working_text: Color,
}

pub const DARK: Theme = Theme {
    text: Color::Rgb(0xd4, 0xd4, 0xd4),
    dim: Color::Rgb(0x66, 0x66, 0x66),
    muted: Color::Rgb(0x80, 0x80, 0x80),
    accent: Color::Rgb(0x8a, 0xbe, 0xb7),
    success: Color::Rgb(0xb5, 0xbd, 0x68),
    error: Color::Rgb(0xcc, 0x66, 0x66),
    warning: Color::Rgb(0xff, 0xff, 0x00),

    user_msg_bg: Color::Rgb(0x34, 0x35, 0x41),
    user_msg_text: Color::Rgb(0xd4, 0xd4, 0xd4),

    tool_pending_bg: Color::Rgb(0x28, 0x28, 0x32),
    tool_success_bg: Color::Rgb(0x28, 0x32, 0x28),
    tool_error_bg: Color::Rgb(0x3c, 0x28, 0x28),
    tool_title: Color::Rgb(0xd4, 0xd4, 0xd4),
    tool_output: Color::Rgb(0x80, 0x80, 0x80),

    thinking_text: Color::Rgb(0x80, 0x80, 0x80),
    thinking_bg: Color::Rgb(0x2c, 0x2c, 0x36),

    editor_border: Color::Rgb(0x8a, 0xbe, 0xb7),

    footer_text: Color::Rgb(0x66, 0x66, 0x66),
    streaming_dot: Color::Rgb(0x8a, 0xbe, 0xb7),
    idle_dot: Color::Rgb(0x50, 0x50, 0x50),
    working_text: Color::Rgb(0x80, 0x80, 0x80),
};

impl Theme {
    pub fn user_msg_style(&self) -> Style {
        Style::default().fg(self.user_msg_text).bg(self.user_msg_bg)
    }

    pub fn tool_pending_style(&self) -> Style {
        Style::default()
            .fg(self.tool_title)
            .bg(self.tool_pending_bg)
    }

    pub fn tool_success_style(&self) -> Style {
        Style::default()
            .fg(self.tool_output)
            .bg(self.tool_success_bg)
    }

    pub fn tool_error_style(&self) -> Style {
        Style::default().fg(self.error).bg(self.tool_error_bg)
    }

    pub fn thinking_style(&self) -> Style {
        Style::default()
            .fg(self.thinking_text)
            .bg(self.thinking_bg)
            .add_modifier(Modifier::ITALIC)
    }

    pub fn thinking_label_style(&self) -> Style {
        Style::default()
            .fg(self.thinking_text)
            .bg(self.thinking_bg)
            .add_modifier(Modifier::ITALIC)
    }

    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim)
    }

    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn accent_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn editor_border_style(&self) -> Style {
        Style::default().fg(self.editor_border)
    }

    pub fn footer_style(&self) -> Style {
        Style::default().fg(self.footer_text)
    }

    pub fn streaming_dot_style(&self) -> Style {
        Style::default().fg(self.streaming_dot)
    }

    pub fn idle_dot_style(&self) -> Style {
        Style::default().fg(self.idle_dot)
    }

    pub fn working_style(&self) -> Style {
        Style::default().fg(self.working_text)
    }
}
