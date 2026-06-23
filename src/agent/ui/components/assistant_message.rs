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
            // it's a full accumulation - replace instead of append.
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
    // as independent concerns - tool expansion must not affect thinking visibility.

    fn render(&mut self, width: usize) -> Vec<String> {
        if self.cached_width == width
            && let Some(ref lines) = self.cached_lines
        {
            return lines.clone();
        }

        let mut lines: Vec<String> = Vec::new();

        // Collect visible content blocks (thinking + text) matching pi's message.content array.
        // Pi iterates over message.content[type="thinking"|type="text"] and adds a leading
        // Spacer(1) if any visible content exists, then per-block conditional spacers.
        struct ContentItem {
            is_thinking: bool,
            text: String,
        }

        let mut items: Vec<ContentItem> = Vec::new();
        for block in &self.thinking {
            let trimmed = block.text.trim().to_string();
            if !trimmed.is_empty() {
                items.push(ContentItem {
                    is_thinking: true,
                    text: trimmed,
                });
            }
        }
        let text_trimmed = self.text.trim().to_string();
        if !text_trimmed.is_empty() {
            items.push(ContentItem {
                is_thinking: false,
                text: text_trimmed,
            });
        }

        if items.is_empty() {
            self.cached_lines = Some(Vec::new());
            self.cached_width = width;
            return Vec::new();
        }

        // Leading blank line before any content (matching pi's leading Spacer(1))
        lines.push(String::new());

        for (i, item) in items.iter().enumerate() {
            if item.is_thinking {
                if self.hide_thinking {
                    // Match pi: Text with padding_x=1, italic, thinkingText color (no bg)
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
                        item.text.clone(),
                        1,
                        0,
                        crate::agent::ui::theme::get_markdown_theme(),
                        Some(default_style),
                    );
                    lines.extend(md.render(width));
                }
            } else {
                // Render main text content through Markdown (matching pi)
                let md_theme = crate::agent::ui::theme::get_markdown_theme();
                let mut md = Markdown::new(item.text.clone(), 1, 0, md_theme, None);
                lines.extend(md.render(width));
            }

            // Pi: after each block, add Spacer(1) if there's visible content after this block
            let has_content_after = items[i + 1..].iter().any(|next| {
                if next.is_thinking && self.hide_thinking {
                    // Hidden thinking is always visible as a label
                    true
                } else {
                    !next.text.is_empty()
                }
            });
            if has_content_after {
                lines.push(String::new());
            }
        }

        // Add OSC133 zones around the entire component (matching pi)
        if !lines.is_empty() {
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
