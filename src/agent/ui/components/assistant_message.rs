use std::sync::Arc;

use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::components::markdown::{DefaultTextStyle, Markdown, StyleFn};
const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// Assistant message component - matches pi's AssistantMessageComponent.
/// Renders text content with Markdown, optional thinking blocks.
pub struct AssistantMessageComponent {
    text: String,
    thinking: Vec<ThinkingBlock>,
    hide_thinking: bool,
    cached_lines: Option<Vec<String>>,
    cached_width: usize,
}

pub struct ThinkingBlock {
    pub text: String,
    pub level: Option<String>,
}

impl AssistantMessageComponent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            thinking: Vec::new(),
            hide_thinking: false,
            cached_lines: None,
            cached_width: 0,
        }
    }

    pub fn add_thinking(&mut self, text: impl Into<String>, level: Option<String>) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.thinking.last_mut() {
            // Skip exact duplicates (same content sent again by the provider).
            if text == last.text {
                return;
            }
            // Some providers (e.g. Ollama) send FULL accumulated content in each
            // chunk instead of a delta. Detect this: if the new text is longer,
            // and its trimmed version starts with the trimmed existing text,
            // it's a full accumulation — replace instead of append.
            if text.len() > last.text.len() {
                let t_trimmed = text.trim_start();
                let l_trimmed = last.text.trim_start();
                if t_trimmed.starts_with(l_trimmed) {
                    last.text = text;
                    self.invalidate();
                    return;
                }
            }
            // Default: treat as delta and append
            last.text.push_str(&text);
        } else {
            self.thinking.push(ThinkingBlock { text, level });
        }
        self.invalidate();
    }

    pub fn append_text(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        // Some providers (e.g. Ollama) send FULL accumulated content in each chunk
        // instead of a delta. Detect this: if delta is longer than existing text,
        // and its trimmed version starts with the trimmed existing text,
        // replace instead of append.
        if delta.len() > self.text.len() {
            let d_trimmed = delta.trim_start();
            let s_trimmed = self.text.trim_start();
            if delta == self.text {
                return; // Skip exact duplicate
            }
            if d_trimmed.starts_with(s_trimmed) {
                self.text = delta.to_string();
                self.invalidate();
                return;
            }
        } else if delta == self.text {
            return; // Skip exact duplicate
        }
        self.text.push_str(delta);
        self.invalidate();
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }
}

impl Component for AssistantMessageComponent {
    // No-op: expand/collapse is controlled separately via set_hide_thinking.
    // Pi keeps app.thinking.toggle (Ctrl+T) and app.tools.expand (Ctrl+O)
    // as independent concerns — tool expansion must not affect thinking visibility.

    fn render(&mut self, width: usize) -> Vec<String> {
        if self.cached_width == width
            && let Some(ref lines) = self.cached_lines
        {
            return lines.clone();
        }

        let mut lines: Vec<String> = Vec::new();
        let md_theme = crate::agent::ui::theme::get_markdown_theme();

        let has_thinking =
            !self.thinking.is_empty() && self.thinking.iter().any(|b| !b.text.trim().is_empty());
        let has_text = !self.text.trim().is_empty();
        let has_any_content = has_thinking || has_text;

        // Leading blank line before any content (matching pi's leading Spacer(1))
        if has_any_content {
            lines.push(String::new());
        }

        // Render thinking blocks through Markdown (matching pi's approach)
        for block in &self.thinking {
            if block.text.trim().is_empty() {
                continue;
            }
            if self.hide_thinking {
                // Match pi: Text with padding_x=1, italic, thinkingText color
                let theme = current_theme();
                let label = theme.italic(&theme.fg_key(ThemeKey::ThinkingText, "Thinking..."));
                let padded = format!(" {} ", label);
                lines.push(padded);
            } else {
                // Pi always uses thinkingText color for ALL thinking blocks (not per-level)
                let color_fn: StyleFn = Arc::new(|s: &str| -> String {
                    crate::agent::ui::theme::current_theme().fg_key(ThemeKey::ThinkingText, s)
                });
                let default_style = DefaultTextStyle {
                    color: Some(color_fn),
                    bold: false,
                    italic: true,
                    strikethrough: false,
                    underline: false,
                };
                // Match pi: padding_x=1, padding_y=0, thinkingText + italic
                let mut md = Markdown::new(
                    block.text.trim().to_string(),
                    1,
                    0,
                    crate::agent::ui::theme::get_markdown_theme(),
                    Some(default_style),
                    None,
                );
                lines.extend(md.render(width));
            }
        }

        // Blank line between thinking and text (matching pi's Spacer(1) after thinking)
        if has_thinking && has_text {
            lines.push(String::new());
        }

        // Render main text content through Markdown (matching pi)
        if has_text {
            let mut md = Markdown::new(self.text.trim().to_string(), 1, 0, md_theme, None, None);
            lines.extend(md.render(width));
        }

        // Add OSC133 zones around the entire component (matching pi)
        if has_any_content && !lines.is_empty() {
            lines[0] = format!("{}{}", OSC133_ZONE_START, &lines[0]);
            if let Some(last) = lines.last_mut() {
                last.push_str(OSC133_ZONE_END);
                last.push_str(OSC133_ZONE_FINAL);
            }
        }

        let result = lines.clone();
        self.cached_lines = Some(lines);
        self.cached_width = width;
        result
    }

    fn set_hide_thinking(&mut self, hide: bool) {
        self.hide_thinking = hide;
        self.invalidate();
    }

    fn invalidate(&mut self) {
        self.cached_lines = None;
    }
}
