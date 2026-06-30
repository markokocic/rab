//! LoginDialog component — matching pi's LoginDialogComponent.
//!
//! Replaces the editor area during login flows. Shows a border, title,
//! dynamic content area, and an input field for prompting the user.
//!
//! Methods (matching pi):
//! - showPrompt(message, placeholder?) → Promise<string>
//! - showManualInput(prompt) → Promise<string>
//! - showInfo(lines)
//! - showWaiting(message)
//! - showProgress(message)
//! - showAuth(url, instructions?)
//! - showDeviceCode(info)

use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::keybindings::{
    ACTION_EDITOR_DELETE_CHAR_BACKWARD, ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM,
    get_keybindings,
};
use crossterm::event::{KeyCode, KeyEvent};

/// Internal state for what the dialog is currently doing.
#[allow(dead_code)]
enum DialogState {
    /// Showing informational text (no input).
    Info { lines: Vec<String> },
    /// Showing a prompt with an input field.
    Prompt {
        message: String,
        placeholder: Option<String>,
    },
    /// Prompt has been submitted, showing submitted value.
    Submitted { value: String },
    /// Showing an auth URL with optional instructions.
    Auth {
        url: String,
        instructions: Option<String>,
    },
    /// Showing device code flow info.
    DeviceCode {
        verification_uri: String,
        user_code: String,
    },
    /// Showing a waiting message (for polling flows).
    Waiting { message: String },
    /// Showing progress messages (appended one by one).
    Progress { messages: Vec<String> },
    /// Manual input prompt (for paste redirect URL).
    ManualInput { prompt: String },
    /// Dialog is done.
    Done,
}

/// Login dialog — replaces editor area during login (matching pi's LoginDialogComponent).
pub struct LoginDialog {
    provider_id: String,
    provider_name: String,
    state: DialogState,
    input_buffer: String,
    /// Callback when user submits the prompt.
    on_submit: Option<Box<dyn FnOnce(String)>>,
    /// Callback when user cancels.
    on_cancel: Option<Box<dyn FnOnce()>>,
    submitted: bool,
}

impl LoginDialog {
    pub fn new(provider_id: String, provider_name: String) -> Self {
        Self {
            provider_id,
            provider_name,
            state: DialogState::Info { lines: Vec::new() },
            input_buffer: String::new(),
            on_submit: None,
            on_cancel: None,
            submitted: false,
        }
    }

    /// Set the callback for when user submits input.
    pub fn on_submit<F>(&mut self, f: F)
    where
        F: FnOnce(String) + 'static,
    {
        self.on_submit = Some(Box::new(f));
    }

    /// Set the callback for when user cancels.
    pub fn on_cancel<F>(&mut self, f: F)
    where
        F: FnOnce() + 'static,
    {
        self.on_cancel = Some(Box::new(f));
    }

    /// Show a prompt and wait for input (matching pi's showPrompt).
    pub fn show_prompt(&mut self, message: &str, placeholder: Option<&str>) {
        self.state = DialogState::Prompt {
            message: message.to_string(),
            placeholder: placeholder.map(|s| s.to_string()),
        };
        self.input_buffer.clear();
    }

    /// Show informational text (matching pi's showInfo).
    pub fn show_info(&mut self, lines: &[&str]) {
        self.state = DialogState::Info {
            lines: lines.iter().map(|s| s.to_string()).collect(),
        };
    }

    /// Show an auth URL with optional instructions (matching pi's showAuth).
    /// Opens the URL in the browser if supported.
    pub fn show_auth(&mut self, url: &str, instructions: Option<&str>) {
        self.state = DialogState::Auth {
            url: url.to_string(),
            instructions: instructions.map(|s| s.to_string()),
        };
    }

    /// Show device code flow info (matching pi's showDeviceCode).
    pub fn show_device_code(&mut self, verification_uri: &str, user_code: &str) {
        self.state = DialogState::DeviceCode {
            verification_uri: verification_uri.to_string(),
            user_code: user_code.to_string(),
        };
    }

    /// Show a waiting message (matching pi's showWaiting).
    pub fn show_waiting(&mut self, message: &str) {
        self.state = DialogState::Waiting {
            message: message.to_string(),
        };
    }

    /// Show a progress message (matching pi's showProgress).
    /// Appends to any existing progress messages.
    pub fn show_progress(&mut self, message: &str) {
        match &mut self.state {
            DialogState::Progress { messages } => {
                messages.push(message.to_string());
            }
            _ => {
                self.state = DialogState::Progress {
                    messages: vec![message.to_string()],
                };
            }
        }
    }

    /// Show a manual input prompt (matching pi's showManualInput).
    /// Unlike showPrompt, this does NOT clear existing content — it appends
    /// the input field below whatever is currently shown.
    pub fn show_manual_input(&mut self, prompt: &str) {
        self.state = DialogState::ManualInput {
            prompt: prompt.to_string(),
        };
        self.input_buffer.clear();
    }

    /// Reset the dialog for reuse.
    pub fn reset(&mut self) {
        self.state = DialogState::Info { lines: Vec::new() };
        self.input_buffer.clear();
        self.submitted = false;
    }

    /// The provider ID.
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }
}

impl Component for LoginDialog {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        // Top border (matching pi's DynamicBorder)
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
        lines.push(String::new());

        // Title
        lines.push(format!(
            "  {}",
            theme.bold(&theme.fg_key(
                ThemeKey::Accent,
                &format!("Login to {}", self.provider_name)
            ))
        ));
        lines.push(String::new());

        match &self.state {
            DialogState::Info { lines: info_lines } => {
                if info_lines.is_empty() {
                    lines.push(format!("  {}", theme.dim("Ready.")));
                } else {
                    for line in info_lines {
                        lines.push(format!("  {}", line));
                    }
                }
            }
            DialogState::Prompt {
                message,
                placeholder,
            } => {
                // Prompt message
                lines.push(format!("  {}", theme.fg_key(ThemeKey::Text, message)));
                if let Some(placeholder) = placeholder {
                    lines.push(format!(
                        "  {}",
                        theme.dim(&format!("e.g., {}", placeholder))
                    ));
                }
                lines.push(String::new());

                // Input line with masked API key display
                let masked: String = if self.input_buffer.is_empty() {
                    String::new()
                } else {
                    "\u{2022}".repeat(self.input_buffer.len().min(50))
                };
                let cursor = "\u{2588}"; // full block
                lines.push(format!(
                    "  {}",
                    theme.fg_key(ThemeKey::Text, &format!("{} {}", masked, cursor))
                ));

                if !self.input_buffer.is_empty() {
                    lines.push(format!(
                        "  {}",
                        theme.dim(&format!("({} characters)", self.input_buffer.len()))
                    ));
                    lines.push(String::new());
                }

                // Key hints (matching pi's keyHint)
                lines.push(format!("  {}", theme.dim("Enter: submit · Esc: cancel")));
            }
            DialogState::Submitted { value } => {
                // Show submitted value (matching pi's replaceInputWithSubmittedText)
                lines.push(format!(
                    "  {}",
                    theme.fg_key(ThemeKey::Text, &format!("> {}", value))
                ));
                if self.submitted {
                    lines.push(String::new());
                    lines.push(format!("  {}", theme.success("API key saved.")));
                }
            }
            DialogState::Auth { url, instructions } => {
                // Show URL as clickable link hint
                let linked = format!("\x1b]8;;{url}\x07{url}\x1b]8;;\x07", url = url);
                lines.push(format!("  {}", theme.fg_key(ThemeKey::Accent, &linked)));
                lines.push(format!("  {}", theme.dim("Ctrl+click to open in browser")));
                if let Some(instr) = instructions {
                    lines.push(String::new());
                    lines.push(format!("  {}", theme.fg_key(ThemeKey::Warning, instr)));
                }
                lines.push(String::new());
                lines.push(format!("  {}", theme.dim("Esc: cancel")));
            }
            DialogState::DeviceCode {
                verification_uri,
                user_code,
            } => {
                let linked = format!("\x1b]8;;{uri}\x07{uri}\x1b]8;;\x07", uri = verification_uri);
                lines.push(format!("  {}", theme.fg_key(ThemeKey::Accent, &linked)));
                lines.push(format!("  {}", theme.dim("Ctrl+click to open in browser")));
                lines.push(String::new());
                lines.push(format!(
                    "  {}",
                    theme.fg_key(ThemeKey::Warning, &format!("Enter code: {}", user_code))
                ));
                lines.push(String::new());
                lines.push(format!("  {}", theme.dim("Esc: cancel")));
            }
            DialogState::Waiting { message } => {
                lines.push(format!("  {}", theme.fg_key(ThemeKey::Dim, message)));
                lines.push(String::new());
                lines.push(format!("  {}", theme.dim("Esc: cancel")));
            }
            DialogState::Progress { messages } => {
                for msg in messages {
                    lines.push(format!("  {}", theme.fg_key(ThemeKey::Dim, msg)));
                }
                lines.push(String::new());
                lines.push(format!("  {}", theme.dim("Esc: cancel")));
            }
            DialogState::ManualInput { prompt } => {
                // Don't clear existing lines — show prompt below current content.
                // The prompt is followed by the input field.
                lines.push(format!("  {}", theme.fg_key(ThemeKey::Dim, prompt)));
                lines.push(String::new());

                // Input line (not masked — shows actual URL/code)
                let display = if self.input_buffer.is_empty() {
                    String::new()
                } else {
                    self.input_buffer.clone()
                };
                let cursor = "\u{2588}";
                lines.push(format!(
                    "  {}",
                    theme.fg_key(ThemeKey::Text, &format!("{} {}", display, cursor))
                ));
                lines.push(String::new());
                lines.push(format!("  {}", theme.dim("Enter: submit · Esc: cancel")));
            }
            DialogState::Done => {
                lines.push(format!("  {}", theme.dim("Login complete.")));
            }
        }

        lines.push(String::new());

        // Bottom border
        lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        if self.submitted {
            return false;
        }

        let kb = get_keybindings();

        // Escape cancels (matching pi's onEscape / select.cancel).
        // Returns false so the main loop pops the overlay (same as OAuthSelector).
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            if self.submitted {
                return false;
            }
            self.submitted = true;
            if let Some(cb) = self.on_cancel.take() {
                cb();
            }
            return false;
        }

        // Only handle text input in Prompt or ManualInput states
        let is_input_state = matches!(
            self.state,
            DialogState::Prompt { .. } | DialogState::ManualInput { .. }
        );

        if !is_input_state {
            return false;
        }

        // Enter submits
        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            let value = std::mem::take(&mut self.input_buffer);
            if !value.is_empty() {
                let old_state = std::mem::replace(
                    &mut self.state,
                    DialogState::Submitted {
                        value: value.clone(),
                    },
                );
                self.submitted = true;
                if let Some(cb) = self.on_submit.take()
                    && matches!(
                        old_state,
                        DialogState::Prompt { .. } | DialogState::ManualInput { .. }
                    )
                {
                    cb(value);
                }
            }
            return true;
        }

        // Backspace
        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
            self.input_buffer.pop();
            return true;
        }

        // Printable characters
        if let KeyCode::Char(c) = key.code
            && !c.is_control()
            && !key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
            && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
        {
            self.input_buffer.push(c);
            return true;
        }

        // Ctrl+C cancels
        if key.code == KeyCode::Char('c')
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            self.submitted = true;
            if let Some(cb) = self.on_cancel.take() {
                cb();
            }
            return true;
        }

        false
    }

    fn handle_paste(&mut self, text: &str) {
        // Insert pasted text into buffer when in an input state
        if self.submitted {
            return;
        }
        let is_input_state = matches!(
            self.state,
            DialogState::Prompt { .. } | DialogState::ManualInput { .. }
        );
        if is_input_state {
            self.input_buffer.push_str(text);
        }
    }
}
