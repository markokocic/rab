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
//!
//! For now, only showPrompt is used (API key login). Other methods are
//! ready for future OAuth support.

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
    /// Showing an informational message (no input).
    Info,
    /// Showing a prompt with an input field.
    Prompt {
        message: String,
        placeholder: Option<String>,
    },
    /// Prompt has been submitted, showing submitted value.
    Submitted { value: String },
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
            state: DialogState::Info,
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
    pub fn show_info(&mut self, _lines: &[&str]) {
        self.state = DialogState::Info;
    }

    /// Reset the dialog for reuse.
    pub fn reset(&mut self) {
        self.state = DialogState::Info;
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
            DialogState::Info => {
                lines.push(format!("  {}", theme.dim("Ready.")));
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

        // Escape cancels (matching pi's onEscape / select.cancel)
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            self.submitted = true;
            if let Some(cb) = self.on_cancel.take() {
                cb();
            }
            return true;
        }

        // Only handle input in Prompt state
        match &self.state {
            DialogState::Prompt { .. } => {}
            _ => return false,
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
                    && let DialogState::Prompt {
                        message: _,
                        placeholder: _,
                    } = old_state
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
}
