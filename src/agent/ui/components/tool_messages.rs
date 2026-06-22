use std::cell::RefCell;
use std::rc::Rc;

use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::components::Text;
use crate::tui::components::r#box::TuiBox;

/// Maximum preview lines when collapsed (matching pi's collapsible tool result).
const PREVIEW_LINES: usize = 10;

/// Combined tool execution component — matches pi's `ToolExecutionComponent`.
/// Renders tool call + result as ONE component with background transitions:
/// - Pending (call only, no result) → `toolPendingBg`
/// - Success (call + result, !is_error) → `toolSuccessBg`
/// - Error (call + result, is_error) → `toolErrorBg`
pub struct ToolExecComponent {
    #[allow(dead_code)]
    name: String,
    header_styled: String,
    output: Option<String>,
    is_error: bool,
    is_complete: bool,
    expanded: bool,
    /// Optional file path for syntax highlighting (read tool).
    file_path: Option<String>,
}

impl ToolExecComponent {
    pub fn new(name: impl Into<String>, header_styled: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            header_styled: header_styled.into(),
            output: None,
            is_error: false,
            is_complete: false,
            expanded: false,
            file_path: None,
        }
    }

    /// Set the file path (for syntax highlighting on read results).
    pub fn set_file_path(&mut self, path: impl Into<String>) {
        self.file_path = Some(path.into());
    }

    /// Builder-style: set file path and return self.
    pub fn with_file_path(mut self, path: String) -> Self {
        self.file_path = Some(path);
        self
    }

    /// Set the result output. Called when ToolResult event arrives.
    pub fn set_result(&mut self, output: impl Into<String>, is_error: bool) {
        self.output = Some(output.into());
        self.is_error = is_error;
        self.is_complete = true;
    }
}

impl Component for ToolExecComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    fn render(&self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let bg_key = if !self.is_complete {
            "toolPendingBg"
        } else if self.is_error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        };
        let bg_ansi = theme.bg_ansi(bg_key).to_string();
        drop(theme);

        let mut msg_box = TuiBox::new(
            1,
            1,
            Some(std::boxed::Box::new(move |s: &str| -> String {
                format!("{}{}\x1b[49m", bg_ansi, s)
            })),
        );

        // Header line: styled tool name + args
        let header_text = Text::new(self.header_styled.clone(), 0, 0, None);
        msg_box.add_child(std::boxed::Box::new(header_text));

        // Result output (if complete)
        if let Some(ref output) = self.output {
            let theme = crate::agent::ui::theme::current_theme();
            let fg_key = if self.is_error { "error" } else { "toolOutput" };
            let fg_ansi = theme.fg_ansi(fg_key).to_string();
            drop(theme);

            // Truncate preview when collapsed
            let display_text = if self.expanded {
                output.clone()
            } else {
                let lines: Vec<&str> = output.lines().collect();
                if lines.len() > PREVIEW_LINES {
                    let preview = lines[..PREVIEW_LINES].join("\n");
                    format!(
                        "{}\n... ({} more lines)",
                        preview,
                        lines.len() - PREVIEW_LINES
                    )
                } else {
                    output.clone()
                }
            };

            // Apply syntax highlighting for read results
            let styled_lines: Vec<String> = if self.name == "read" && !self.is_error {
                if let Some(ref path) = self.file_path {
                    let lang = crate::tui::components::path_to_language(path);
                    #[cfg(feature = "syntect")]
                    if lang.is_some() {
                        let hl = crate::tui::components::highlight_code(&display_text, lang);
                        if !hl.is_empty() {
                            hl
                        } else {
                            display_text
                                .lines()
                                .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                                .collect()
                        }
                    } else {
                        display_text
                            .lines()
                            .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                            .collect()
                    }
                } else {
                    display_text
                        .lines()
                        .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                        .collect()
                }
            } else {
                display_text
                    .lines()
                    .map(|line| format!("{}{}\x1b[39m", fg_ansi, line))
                    .collect()
            };

            let result_text = Text::new(styled_lines.join("\n"), 0, 0, None);
            msg_box.add_child(std::boxed::Box::new(result_text));
        }

        msg_box.render(width)
    }

    fn invalidate(&mut self) {}
}

/// A Component wrapper around `Rc<RefCell<ToolExecComponent>>` for shared ownership.
/// Allows App to hold `Weak<RefCell<ToolExecComponent>>` for result updates.
pub struct RcToolExec(pub Rc<RefCell<ToolExecComponent>>);

impl Clone for RcToolExec {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Component for RcToolExec {
    fn render(&self, width: usize) -> Vec<String> {
        self.0.borrow().render(width)
    }

    fn set_expanded(&mut self, expanded: bool) {
        self.0.borrow_mut().set_expanded(expanded);
    }

    fn invalidate(&mut self) {
        self.0.borrow_mut().invalidate();
    }
}

// ── Keep old types for backward compatibility ──

/// Old tool call component — kept for compatibility.
pub struct ToolCallComponent {
    name: String,
    args: String,
    expanded: bool,
}

impl ToolCallComponent {
    pub fn new(name: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: args.into(),
            expanded: false,
        }
    }
}

impl Component for ToolCallComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
    fn render(&self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let bg_ansi = theme.bg_ansi("toolPendingBg").to_string();
        drop(theme);

        let mut styled = String::new();
        styled.push_str("\x1b[1m");
        styled.push_str(current_theme().fg_ansi("toolTitle"));
        styled.push_str(&self.name);
        styled.push_str("\x1b[22m");

        if !self.args.is_empty() && self.args != "{}" {
            styled.push_str("  ");
            styled.push_str(current_theme().fg_ansi("muted"));
            styled.push_str(&self.args);
        }
        styled.push_str("\x1b[39m");

        let mut msg_box = TuiBox::new(
            1,
            1,
            Some(std::boxed::Box::new(move |s: &str| -> String {
                format!("{}{}\x1b[49m", bg_ansi, s)
            })),
        );
        msg_box.add_child(std::boxed::Box::new(Text::new(styled, 0, 0, None)));
        msg_box.render(width)
    }
    fn invalidate(&mut self) {}
}

/// Old tool result component — kept for compatibility.
pub struct ToolResultComponent {
    content: String,
    is_error: bool,
    expanded: bool,
}

impl ToolResultComponent {
    pub fn new(content: impl Into<String>, is_error: bool) -> Self {
        Self {
            content: content.into(),
            is_error,
            expanded: false,
        }
    }
}

impl Component for ToolResultComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
    fn render(&self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let bg_key = if self.is_error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        };
        let fg_key = if self.is_error { "error" } else { "toolOutput" };
        let bg_ansi = theme.bg_ansi(bg_key).to_string();
        let styled = theme.fg(fg_key, &self.content);
        drop(theme);

        let mut msg_box = TuiBox::new(
            1,
            0,
            Some(std::boxed::Box::new(move |s: &str| -> String {
                format!("{}{}\x1b[49m", bg_ansi, s)
            })),
        );
        msg_box.add_child(std::boxed::Box::new(Text::new(styled, 0, 0, None)));
        msg_box.render(width)
    }
    fn invalidate(&mut self) {}
}
