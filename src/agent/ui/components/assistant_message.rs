use std::sync::Arc;

use crate::agent::ui::theme::ThemeKey;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::component::RenderCacheKey;
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
    /// Persistent Markdown instance for the text content.
    /// Reused across renders so its internal cache (cached_text + cached_lines)
    /// avoids re-parsing when text hasn't changed (e.g. spinner ticks between deltas).
    text_md: Option<Markdown>,
    /// Persistent Markdown instances for each thinking block.
    thinking_md: Vec<Markdown>,
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
            text_md: None,
            thinking_md: Vec::new(),
        }
    }

    /// Ensure the persistent text Markdown instance matches the current text.
    fn sync_text_md(&mut self) {
        if self.text.is_empty() {
            self.text_md = None;
            return;
        }
        let md_theme = crate::agent::ui::theme::get_markdown_theme();
        let should_recreate = match self.text_md {
            Some(ref mut md) => {
                let needs_update = !md.cached_text_matches(&self.text);
                if needs_update {
                    md.set_text(&self.text);
                }
                false // reuse existing
            }
            None => true, // create new
        };
        if should_recreate {
            self.text_md = Some(Markdown::new(self.text.clone(), 1, 0, md_theme, None));
        }
    }

    /// Ensure the persistent thinking Markdown instances match current thinking blocks.
    fn sync_thinking_md(&mut self) {
        // Remove excess cached instances
        while self.thinking_md.len() > self.thinking.len() {
            self.thinking_md.pop();
        }
        for (i, block) in self.thinking.iter().enumerate() {
            if block.text.trim().is_empty() {
                continue;
            }
            if i >= self.thinking_md.len() {
                // Create new Markdown for this block
                let color_fn: StyleFn = Arc::new(|s: &str| -> String {
                    crate::agent::ui::theme::current_theme().fg(ThemeKey::ThinkingText.as_str(), s)
                });
                let default_style = DefaultTextStyle {
                    color: Some(color_fn),
                    bold: false,
                    italic: true,
                    strikethrough: false,
                    underline: false,
                };
                let mut md = Markdown::new(
                    block.text.clone(),
                    1,
                    0,
                    crate::agent::ui::theme::get_markdown_theme(),
                    Some(default_style),
                );
                // Mark dirty since text may have already changed by the time
                // this instance is created (the block was pushed earlier).
                md.invalidate();
                self.thinking_md.push(md);
            } else {
                let needs_update = !self.thinking_md[i].cached_text_matches(&block.text);
                if needs_update {
                    self.thinking_md[i].set_text(&block.text);
                }
            }
        }
    }

    /// Compute a hash of the current state for cache_key.
    fn state_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.text.hash(&mut hasher);
        self.hide_thinking.hash(&mut hasher);
        for block in &self.thinking {
            block.text.hash(&mut hasher);
            block.level.hash(&mut hasher);
        }
        hasher.finish()
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

        // Sync persistent Markdown instances before rendering so they
        // can leverage their own text-based caching.
        self.sync_text_md();
        if !self.thinking.is_empty() {
            self.sync_thinking_md();
        }

        let mut lines: Vec<String> = Vec::new();

        let has_thinking = self.thinking.iter().any(|b| !b.text.trim().is_empty());
        let has_text = !self.text.trim().is_empty();
        if !has_thinking && !has_text {
            self.cached_lines = Some(Vec::new());
            self.cached_width = width;
            return Vec::new();
        }

        // Leading blank line before any content (matching pi's leading Spacer(1))
        lines.push(String::new());

        let mut think_idx = 0;
        for (block_idx, block) in self.thinking.iter().enumerate() {
            let trimmed = block.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if self.hide_thinking {
                let theme = current_theme();
                let label = theme.italic(&theme.fg(ThemeKey::ThinkingText.as_str(), "Thinking..."));
                let padded = format!(" {} ", label);
                lines.push(padded);
            } else {
                // Use persistent Markdown instance for this thinking block
                if let Some(md) = self.thinking_md.get_mut(think_idx) {
                    lines.extend(md.render(width));
                }
                think_idx += 1;
            }

            // Pi: after each block, add Spacer(1) if there's visible content after
            let has_content_after = self.thinking.iter().skip(block_idx + 1).any(|b| {
                if self.hide_thinking {
                    true
                } else {
                    !b.text.trim().is_empty()
                }
            }) || has_text;
            if has_content_after {
                lines.push(String::new());
            }
        }

        if has_text {
            // Use persistent text Markdown instance
            if let Some(ref mut md) = self.text_md {
                lines.extend(md.render(width));
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

    fn cache_key(&self, width: usize) -> Option<RenderCacheKey> {
        Some(RenderCacheKey {
            width,
            expanded: false,
            state_hash: self.state_hash(),
        })
    }

    fn set_hide_thinking(&mut self, hide: bool) {
        self.hide_thinking = hide;
        self.invalidate();
    }

    fn invalidate(&mut self) {
        self.cached_lines = None;
    }
}
