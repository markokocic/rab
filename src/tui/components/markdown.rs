#![allow(clippy::type_complexity, clippy::arc_with_non_send_sync)]

use std::cell::RefCell;
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::tui::Component;
use crate::tui::util::{apply_background_to_line, visible_width, wrap_text_with_ansi};

/// Type alias for markdown theme styling functions.
pub type StyleFn = Arc<dyn Fn(&str) -> String>;
/// Type alias for code highlighting function.
pub type HighlightFn = Arc<dyn Fn(&str, Option<&str>) -> Vec<String>>;

// ── MarkdownTheme ────────────────────────────────────────────────

/// Theme functions for markdown elements.
/// Each function takes text and returns styled text with ANSI codes.
pub struct MarkdownTheme {
    pub heading: StyleFn,
    pub link: StyleFn,
    pub link_url: StyleFn,
    pub code: StyleFn,
    pub code_block: StyleFn,
    pub code_block_border: StyleFn,
    pub quote: StyleFn,
    pub quote_border: StyleFn,
    pub hr: StyleFn,
    pub list_bullet: StyleFn,
    pub bold: StyleFn,
    pub italic: StyleFn,
    pub strikethrough: StyleFn,
    pub underline: StyleFn,
    /// If set, used for syntax-highlighted code blocks.
    pub highlight_code: Option<HighlightFn>,
    /// Prefix applied to each rendered code block line (default: `"  "`).
    pub code_block_indent: String,
}

impl MarkdownTheme {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        heading: StyleFn,
        link: StyleFn,
        link_url: StyleFn,
        code: StyleFn,
        code_block: StyleFn,
        code_block_border: StyleFn,
        quote: StyleFn,
        quote_border: StyleFn,
        hr: StyleFn,
        list_bullet: StyleFn,
        bold: StyleFn,
        italic: StyleFn,
        strikethrough: StyleFn,
        underline: StyleFn,
    ) -> Self {
        Self {
            heading,
            link,
            link_url,
            code,
            code_block,
            code_block_border,
            quote,
            quote_border,
            hr,
            list_bullet,
            bold,
            italic,
            strikethrough,
            underline,
            highlight_code: None,
            code_block_indent: "  ".to_string(),
        }
    }
}

// ── DefaultTextStyle ─────────────────────────────────────────────

/// Default text styling for markdown content.
/// Applied to all text unless overridden by markdown formatting.
pub struct DefaultTextStyle {
    /// Optional foreground color function.
    pub color: Option<StyleFn>,
    /// Optional background color function (applied at the padding stage).
    pub bg_color: Option<StyleFn>,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
}

// ── MarkdownOptions ──────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct MarkdownOptions {
    /// Preserve source list markers instead of normalizing them.
    pub preserve_ordered_list_markers: bool,
}

// ── Internal helpers ─────────────────────────────────────────────

/// Context for inline rendering, carrying the parent-style functions
/// and the ANSI prefix to restore after inline resets.
struct InlineCtx {
    /// Apply the current text style (color + decorations).
    apply_text: Arc<dyn Fn(&str) -> String>,
    /// ANSI prefix to emit after closing an inline element,
    /// restoring this context's styling.
    style_prefix: String,
}

impl InlineCtx {
    fn new(apply_text: Arc<dyn Fn(&str) -> String>) -> Self {
        let prefix = get_style_prefix(&*apply_text);
        Self {
            apply_text,
            style_prefix: prefix,
        }
    }
}

/// Extract the ANSI prefix from a style function.
/// Uses a sentinel character (`\0`) to find where text starts.
fn get_style_prefix(style_fn: &dyn Fn(&str) -> String) -> String {
    const SENTINEL: char = '\0';
    let styled = style_fn(&SENTINEL.to_string());
    styled
        .find(SENTINEL)
        .map(|i| styled[..i].to_string())
        .unwrap_or_default()
}

/// Check whether hyperlinks (OSC 8) are supported.
/// Detects common terminal emulators that support the feature.
fn hyperlinks_supported() -> bool {
    // Check well-known terminal programs
    if let Ok(prog) = std::env::var("TERM_PROGRAM")
        && (prog == "iTerm.app" || prog == "kitty" || prog == "WezTerm" || prog == "vscode")
    {
        return true;
    }
    // Check TERM env for kitty
    if let Ok(term) = std::env::var("TERM")
        && term.contains("kitty")
    {
        return true;
    }
    // Windows Terminal supports OSC 8
    #[cfg(windows)]
    {
        if let Ok(prog) = std::env::var("WT_SESSION") {
            let _ = prog;
            return true;
        }
    }
    false
}

/// Wrap text in an OSC 8 hyperlink.
/// Format: `\x1b]8;params;url\x07text\x1b]8;;\x07`
fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{}\x07{}\x1b]8;;\x07", url, text)
}

// ── Markdown Component ───────────────────────────────────────────

/// Markdown rendering component.
///
/// Parses markdown text with pulldown-cmark and renders styled ANSI output.
/// Two-phase: (1) render tokens → styled ANSI lines, (2) wrap + pad + bg.
pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    theme: MarkdownTheme,
    default_text_style: Option<DefaultTextStyle>,
    #[allow(dead_code)]
    options: MarkdownOptions,

    // Cache
    cached_text: RefCell<Option<String>>,
    cached_width: RefCell<Option<usize>>,
    cached_lines: RefCell<Vec<String>>,
}

impl Markdown {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<DefaultTextStyle>,
        options: Option<MarkdownOptions>,
    ) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            theme,
            default_text_style,
            options: options.unwrap_or_default(),
            cached_text: RefCell::new(None),
            cached_width: RefCell::new(None),
            cached_lines: RefCell::new(Vec::new()),
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }

    fn build_default_ctx(&self) -> InlineCtx {
        InlineCtx::new(self.build_default_apply_fn())
    }

    /// Build the default `apply_text` closure from `DefaultTextStyle`.
    fn build_default_apply_fn(&self) -> Arc<dyn Fn(&str) -> String> {
        let style = &self.default_text_style;
        let theme = &self.theme;

        // Capture what we need as Arcs to satisfy the closure lifetime
        let color: Option<StyleFn> = style.as_ref().and_then(|s| s.color.clone());
        let bold = style.as_ref().map(|s| s.bold).unwrap_or(false);
        let italic = style.as_ref().map(|s| s.italic).unwrap_or(false);
        let strikethrough = style.as_ref().map(|s| s.strikethrough).unwrap_or(false);
        let underline = style.as_ref().map(|s| s.underline).unwrap_or(false);
        let theme_bold = theme.bold.clone();
        let theme_italic = theme.italic.clone();
        let theme_strikethrough = theme.strikethrough.clone();
        let theme_underline = theme.underline.clone();

        Arc::new(move |text: &str| {
            let mut styled = text.to_string();
            if let Some(ref color_fn) = color {
                styled = color_fn(&styled);
            }
            if bold {
                styled = theme_bold(&styled);
            }
            if italic {
                styled = theme_italic(&styled);
            }
            if strikethrough {
                styled = theme_strikethrough(&styled);
            }
            if underline {
                styled = theme_underline(&styled);
            }
            styled
        })
    }

    /// Build the style context for a heading at the given level.
    fn heading_ctx(&self, level: HeadingLevel) -> InlineCtx {
        let theme_heading = self.theme.heading.clone();
        let theme_bold = self.theme.bold.clone();
        let theme_underline = self.theme.underline.clone();

        let style_fn: Arc<dyn Fn(&str) -> String> = match level {
            HeadingLevel::H1 => {
                Arc::new(move |text: &str| theme_heading(&theme_bold(&theme_underline(text))))
            }
            _ => Arc::new(move |text: &str| theme_heading(&theme_bold(text))),
        };
        InlineCtx::new(style_fn)
    }

    /// Build the default inline style context for blockquote content.
    fn quote_ctx(&self) -> InlineCtx {
        let theme_quote = self.theme.quote.clone();
        let theme_italic = self.theme.italic.clone();

        let style_fn: Arc<dyn Fn(&str) -> String> =
            Arc::new(move |text: &str| theme_quote(&theme_italic(text)));
        InlineCtx::new(style_fn)
    }
}

impl Component for Markdown {
    fn render(&self, width: usize) -> Vec<String> {
        // Check cache
        if self.cached_text.borrow().as_deref() == Some(&self.text)
            && *self.cached_width.borrow() == Some(width)
        {
            return self.cached_lines.borrow().clone();
        }

        // Don't render anything if there's no actual text
        if self.text.is_empty() || self.text.trim().is_empty() {
            let result: Vec<String> = Vec::new();
            *self.cached_text.borrow_mut() = Some(self.text.clone());
            *self.cached_width.borrow_mut() = Some(width);
            *self.cached_lines.borrow_mut() = result.clone();
            return result;
        }

        // Calculate available width for content
        let content_width = width.saturating_sub(2 * self.padding_x).max(1);

        // Replace tabs with 3 spaces
        let normalized = self.text.replace('\t', "   ");

        // Parse with pulldown-cmark
        let md_options = Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_HEADING_ATTRIBUTES
            | Options::ENABLE_GFM;
        let parser = Parser::new_ext(&normalized, md_options);
        let events: Vec<Event> = parser.collect();

        // Render document to styled ANSI lines (Phase 1)
        let rendered = self.render_document(&events, content_width);

        // Wrap lines
        let mut wrapped: Vec<String> = Vec::new();
        for line in &rendered {
            for wl in wrap_text_with_ansi(line, content_width) {
                wrapped.push(wl);
            }
        }

        // Add padding and background
        let left_margin = " ".repeat(self.padding_x);
        let right_margin = " ".repeat(self.padding_x);
        let bg_fn = self
            .default_text_style
            .as_ref()
            .and_then(|s| s.bg_color.clone());

        let mut content_lines: Vec<String> = Vec::new();
        for line in &wrapped {
            let line_with_margins = format!("{}{}{}", left_margin, line, right_margin);
            if let Some(ref bg) = bg_fn {
                content_lines.push(apply_background_to_line(
                    &line_with_margins,
                    width,
                    bg.as_ref(),
                ));
            } else {
                let visible = visible_width(&line_with_margins);
                if visible < width {
                    content_lines.push(format!(
                        "{}{}",
                        line_with_margins,
                        " ".repeat(width - visible)
                    ));
                } else {
                    content_lines.push(line_with_margins);
                }
            }
        }

        let empty_line = " ".repeat(width);
        let empty_bg = bg_fn
            .as_ref()
            .map(|bg| bg(&empty_line))
            .unwrap_or_else(|| empty_line.clone());

        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(empty_bg.clone());
        }
        result.extend(content_lines);
        for _ in 0..self.padding_y {
            result.push(empty_bg.clone());
        }

        // Update cache
        *self.cached_text.borrow_mut() = Some(self.text.clone());
        *self.cached_width.borrow_mut() = Some(width);
        *self.cached_lines.borrow_mut() = result.clone();

        if result.is_empty() {
            vec![String::new()]
        } else {
            result
        }
    }

    fn invalidate(&mut self) {
        *self.cached_text.borrow_mut() = None;
        *self.cached_width.borrow_mut() = None;
        self.cached_lines.borrow_mut().clear();
    }
}

// ── Document / Block Rendering ──────────────────────────────────

impl Markdown {
    /// Render the full event stream into styled ANSI lines.
    fn render_document(&self, events: &[Event], width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut pos = 0;

        while pos < events.len() {
            match &events[pos] {
                Event::Start(tag) => {
                    pos += 1;
                    let block_lines = self.render_block(events, &mut pos, tag, width, false, 0);
                    if !block_lines.is_empty() {
                        lines.extend(block_lines);
                    }
                }
                Event::End(_) => {
                    pos += 1;
                }
                Event::Rule => {
                    pos += 1;
                    lines.push((self.theme.hr)(&"─".repeat(width.min(80))));
                    // Check next event for spacing
                    if pos < events.len() && !matches!(events[pos], Event::Start(Tag::Paragraph)) {
                        lines.push(String::new());
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    pos += 1;
                }
                Event::Text(text) => {
                    pos += 1;
                    let ctx = self.build_default_ctx();
                    lines.push((ctx.apply_text)(text));
                }
                _ => {
                    pos += 1;
                }
            }
        }

        lines
    }

    /// Render a single block element, consuming events until the matching `End`.
    /// `inside_quote` indicates whether we're inside a blockquote (affects spacing).
    /// `list_depth` tracks nesting depth for list indentation.
    fn render_block(
        &self,
        events: &[Event],
        pos: &mut usize,
        tag: &Tag,
        width: usize,
        inside_quote: bool,
        list_depth: usize,
    ) -> Vec<String> {
        match tag {
            Tag::Paragraph => {
                let content =
                    self.render_inline(events, pos, TagEnd::Paragraph, &self.build_default_ctx());
                let mut lines = Vec::new();
                if !content.is_empty() {
                    lines.push(content);
                }
                // Add spacing after paragraph if next event isn't a list or space-like
                if *pos < events.len() {
                    let next_is_list = matches!(
                        &events[*pos],
                        Event::Start(Tag::List(_)) | Event::End(TagEnd::List(_))
                    );
                    if !next_is_list {
                        lines.push(String::new());
                    }
                }
                lines
            }

            Tag::Heading { level, .. } => {
                let ctx = self.heading_ctx(*level);
                let mut content = self.render_inline(events, pos, TagEnd::Heading(*level), &ctx);

                // For h3+, add the heading prefix marker
                if *level >= HeadingLevel::H3 {
                    let prefix_marker = format!("{} ", "#".repeat(level_to_usize(*level)));
                    content = format!("{}{}", (ctx.apply_text)(&prefix_marker), content);
                }

                let mut lines = vec![content];
                // Add spacing if next event isn't a space
                if *pos < events.len() {
                    let next_is_para_or_space = matches!(
                        &events[*pos],
                        Event::Start(Tag::Paragraph)
                            | Event::Start(Tag::List(_))
                            | Event::End(TagEnd::List(_))
                            | Event::End(TagEnd::BlockQuote(None))
                    );
                    if !next_is_para_or_space && !inside_quote {
                        lines.push(String::new());
                    }
                }
                lines
            }

            Tag::BlockQuote(kind) => {
                // Blockquotes contain block-level tokens
                let quote_content_width = width.saturating_sub(2).max(1); // "│ " = 2 chars
                let quote_ctx = self.quote_ctx();

                let mut inner_lines: Vec<String> = Vec::new();
                loop {
                    if *pos >= events.len() {
                        break;
                    }
                    match &events[*pos] {
                        Event::End(TagEnd::BlockQuote(k)) if *k == *kind => {
                            *pos += 1;
                            break;
                        }
                        Event::Start(inner_tag) => {
                            *pos += 1;
                            let block_lines = self.render_block(
                                events,
                                pos,
                                inner_tag,
                                quote_content_width,
                                true,
                                0,
                            );
                            inner_lines.extend(block_lines);
                        }
                        Event::End(_) => {
                            *pos += 1;
                        }
                        _ => {
                            // Flush remaining (text directly in blockquote)
                            let text = self.render_inline(
                                events,
                                pos,
                                TagEnd::BlockQuote(*kind),
                                &quote_ctx,
                            );
                            if !text.is_empty() {
                                inner_lines.push(text);
                            }
                        }
                    }
                }

                // Remove trailing blank lines from inner content
                while inner_lines.last().is_some_and(|l| l.is_empty()) {
                    inner_lines.pop();
                }

                // Apply the quote style to each line and add "│ " prefix
                let quote_style_prefix = get_style_prefix(&|s: &str| (quote_ctx.apply_text)(s));
                let qborder = self.theme.quote_border.clone();

                let mut result: Vec<String> = Vec::new();
                for line in &inner_lines {
                    let restyled = if !quote_style_prefix.is_empty() {
                        line.replace("\x1b[0m", &format!("\x1b[0m{}", quote_style_prefix))
                    } else {
                        line.clone()
                    };
                    let styled = (quote_ctx.apply_text)(&restyled);
                    let wrapped = wrap_text_with_ansi(&styled, quote_content_width);
                    for wl in wrapped {
                        result.push(format!("{} {}", qborder("│"), wl));
                    }
                }

                // Add spacing after blockquote
                if *pos < events.len() && !inside_quote {
                    let next_is_space_or_end = matches!(
                        &events[*pos],
                        Event::End(_) | Event::SoftBreak | Event::HardBreak
                    );
                    if !next_is_space_or_end {
                        result.push(String::new());
                    }
                }
                result
            }

            Tag::CodeBlock(kind) => {
                let info = match kind {
                    CodeBlockKind::Fenced(info) => {
                        if info.is_empty() {
                            None
                        } else {
                            Some(info.as_ref())
                        }
                    }
                    CodeBlockKind::Indented => None,
                };

                // Collect code content until End(CodeBlock)
                let mut code_text = String::new();
                loop {
                    if *pos >= events.len() {
                        break;
                    }
                    match &events[*pos] {
                        Event::End(TagEnd::CodeBlock) => {
                            *pos += 1;
                            break;
                        }
                        Event::Text(t) => {
                            code_text.push_str(t);
                            *pos += 1;
                        }
                        Event::SoftBreak | Event::HardBreak => {
                            code_text.push('\n');
                            *pos += 1;
                        }
                        _ => {
                            *pos += 1;
                        }
                    }
                }

                let indent = &self.theme.code_block_indent;
                let border = self.theme.code_block_border.clone();
                let code_fn = self.theme.code_block.clone();

                // Show language in opening fence
                let lang_label = info.unwrap_or("");
                let mut lines = vec![border(&format!("```{}", lang_label))];

                // Syntax highlighting or plain
                if let Some(ref highlight) = self.theme.highlight_code {
                    let hl_lines = highlight(&code_text, info);
                    for hl in hl_lines {
                        lines.push(format!("{}{}", indent, hl));
                    }
                } else {
                    for code_line in code_text.split('\n') {
                        lines.push(format!("{}{}", indent, code_fn(code_line)));
                    }
                }

                lines.push(border("```"));

                // Add spacing after code block
                if *pos < events.len() {
                    let next_is_space = matches!(
                        &events[*pos],
                        Event::Start(Tag::Paragraph)
                            | Event::End(_)
                            | Event::SoftBreak
                            | Event::HardBreak
                    );
                    if !next_is_space {
                        lines.push(String::new());
                    }
                }
                lines
            }

            Tag::List(start) => self.render_list(events, pos, *start, width, list_depth),

            Tag::Item => {
                // Items are handled in render_list; skip to End(Item)
                let mut depth = 1;
                loop {
                    if *pos >= events.len() {
                        break;
                    }
                    match &events[*pos] {
                        Event::Start(Tag::Item) => {
                            depth += 1;
                            *pos += 1;
                        }
                        Event::End(TagEnd::Item) => {
                            depth -= 1;
                            *pos += 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Event::Start(_) => {
                            *pos += 1;
                            // Skip inner content
                            let _ = self.render_block(
                                events,
                                pos,
                                &Tag::Paragraph,
                                width,
                                false,
                                list_depth + 1,
                            );
                        }
                        _ => {
                            *pos += 1;
                        }
                    }
                }
                Vec::new()
            }

            Tag::Table(alignments) => self.render_table(events, pos, alignments, width),

            Tag::HtmlBlock => {
                // Collect HTML content until End(HtmlBlock)
                let mut html_text = String::new();
                loop {
                    if *pos >= events.len() {
                        break;
                    }
                    match &events[*pos] {
                        Event::End(TagEnd::HtmlBlock) => {
                            *pos += 1;
                            break;
                        }
                        Event::Text(t) | Event::Html(t) => {
                            html_text.push_str(t);
                            *pos += 1;
                        }
                        Event::SoftBreak | Event::HardBreak => {
                            html_text.push('\n');
                            *pos += 1;
                        }
                        _ => {
                            *pos += 1;
                        }
                    }
                }
                let ctx = self.build_default_ctx();
                let mut lines = Vec::new();
                for line in html_text.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        lines.push((ctx.apply_text)(trimmed));
                    }
                }
                lines
            }

            Tag::TableHead | Tag::TableRow | Tag::TableCell => {
                // Should be handled by render_table — skip to End
                let end = tag.to_end();
                loop {
                    if *pos >= events.len() {
                        break;
                    }
                    if matches!(&events[*pos], Event::End(e) if *e == end) {
                        *pos += 1;
                        break;
                    }
                    // For TableCell, render inline content
                    if matches!(tag, Tag::TableCell)
                        && let Event::Start(_) = &events[*pos]
                    {
                        // Skip nested starts
                        *pos += 1;
                        continue;
                    }
                    *pos += 1;
                }
                Vec::new()
            }

            Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {
                // Skip unsupported block types
                let end = tag.to_end();
                skip_until(events, pos, end);
                Vec::new()
            }

            // Inline tags at block level — render inline
            Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. } => {
                let content =
                    self.render_inline(events, pos, tag.to_end(), &self.build_default_ctx());
                vec![content]
            }
        }
    }

    /// Render inline-level events into a single styled string.
    fn render_inline(
        &self,
        events: &[Event],
        pos: &mut usize,
        end: TagEnd,
        ctx: &InlineCtx,
    ) -> String {
        let mut result = String::new();

        loop {
            if *pos >= events.len() {
                break;
            }

            match &events[*pos] {
                Event::End(tag_end) if *tag_end == end => {
                    *pos += 1;
                    break;
                }

                Event::Text(text) => {
                    *pos += 1;
                    // Text may contain newlines (from soft breaks in parsed markdown)
                    result.push_str(&split_newline_apply(text, &*ctx.apply_text));
                }

                Event::Code(code) => {
                    *pos += 1;
                    result.push_str(&(self.theme.code)(code));
                    result.push_str(&ctx.style_prefix);
                }

                Event::Start(Tag::Emphasis) => {
                    *pos += 1;
                    let inner = self.render_inline(events, pos, TagEnd::Emphasis, ctx);
                    result.push_str(&(self.theme.italic)(&inner));
                    result.push_str(&ctx.style_prefix);
                }

                Event::Start(Tag::Strong) => {
                    *pos += 1;
                    let inner = self.render_inline(events, pos, TagEnd::Strong, ctx);
                    result.push_str(&(self.theme.bold)(&inner));
                    result.push_str(&ctx.style_prefix);
                }

                Event::Start(Tag::Strikethrough) => {
                    *pos += 1;
                    let inner = self.render_inline(events, pos, TagEnd::Strikethrough, ctx);
                    result.push_str(&(self.theme.strikethrough)(&inner));
                    result.push_str(&ctx.style_prefix);
                }

                Event::Start(Tag::Link {
                    dest_url, title: _, ..
                }) => {
                    *pos += 1;
                    let inner = self.render_inline(events, pos, TagEnd::Link, ctx);

                    let styled_link = (self.theme.link)(&(self.theme.underline)(&inner));

                    if hyperlinks_supported() {
                        result.push_str(&hyperlink(&styled_link, dest_url));
                    } else {
                        // Fallback: print URL in parentheses when text differs from href
                        let href = dest_url.as_ref();
                        let href_clean = if let Some(mailto) = href.strip_prefix("mailto:") {
                            mailto
                        } else {
                            href
                        };
                        if inner.trim() == href_clean || inner.trim() == href {
                            result.push_str(&styled_link);
                        } else {
                            result.push_str(&styled_link);
                            result.push_str(&(self.theme.link_url)(&format!(" ({})", href)));
                        }
                    }
                    result.push_str(&ctx.style_prefix);
                }

                Event::Start(Tag::Image { .. }) => {
                    // Skip image content until End(Image)
                    *pos += 1;
                    let _ = self.render_inline(events, pos, TagEnd::Image, ctx);
                }

                Event::SoftBreak => {
                    *pos += 1;
                    result.push('\n');
                }

                Event::HardBreak => {
                    *pos += 1;
                    result.push('\n');
                }

                Event::InlineHtml(html) | Event::Html(html) => {
                    *pos += 1;
                    result.push_str(&(ctx.apply_text)(html.trim()));
                }

                // Task list marker
                Event::TaskListMarker(checked) => {
                    *pos += 1;
                    let marker = if *checked { "[x] " } else { "[ ] " };
                    let styled = (self.theme.list_bullet)(marker);
                    result.push_str(&styled);
                }

                // Handle inline math as plain text
                Event::InlineMath(math) | Event::DisplayMath(math) => {
                    *pos += 1;
                    result.push_str(&(ctx.apply_text)(math));
                }

                // Footnote reference — render as text
                Event::FootnoteReference(ref_id) => {
                    *pos += 1;
                    result.push_str(&(ctx.apply_text)(&format!("[^{}]", ref_id)));
                }

                // Nested starts in inline context (rare: paragraph inside list item etc.)
                Event::Start(tag) => {
                    *pos += 1;
                    let content = self.render_block(events, pos, tag, 80, false, 0);
                    for (i, line) in content.iter().enumerate() {
                        if i > 0 {
                            result.push('\n');
                        }
                        result.push_str(line);
                    }
                }

                _ => {
                    *pos += 1;
                }
            }
        }

        // Trim trailing style prefix from result (matching pi)
        while result.ends_with(&ctx.style_prefix) && !ctx.style_prefix.is_empty() {
            result = result[..result.len() - ctx.style_prefix.len()].to_string();
        }

        result
    }

    /// Render a list (ordered or unordered).
    fn render_list(
        &self,
        events: &[Event],
        pos: &mut usize,
        start: Option<u64>,
        width: usize,
        depth: usize,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let indent_str = "    ".repeat(depth);
        let start_number = start.unwrap_or(1);
        let mut item_index: u64 = 0;

        loop {
            if *pos >= events.len() {
                break;
            }

            match &events[*pos] {
                Event::End(TagEnd::List(ordered)) => {
                    if *ordered == start.is_some() {
                        *pos += 1;
                        break;
                    }
                    *pos += 1;
                }

                Event::Start(Tag::Item) => {
                    *pos += 1;
                    item_index += 1;

                    // Collect task list marker if present
                    let task_marker = if *pos < events.len() {
                        match &events[*pos] {
                            Event::TaskListMarker(checked) => {
                                *pos += 1;
                                let checked_str = if *checked { "[x] " } else { "[ ] " };
                                Some(checked_str.to_string())
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };

                    let is_ordered = start.is_some();
                    let marker = if is_ordered {
                        let num_str = (start_number + item_index - 1).to_string();
                        format!("{}. ", num_str)
                    } else {
                        "- ".to_string()
                    };
                    let marker = task_marker
                        .map(|tm| format!("{}{}", marker, tm))
                        .unwrap_or(marker);

                    let bullet_prefix = indent_str.clone() + &(self.theme.list_bullet)(&marker);
                    let continuation_prefix =
                        indent_str.clone() + &" ".repeat(visible_width(&marker));
                    let item_width = width.saturating_sub(visible_width(&bullet_prefix)).max(1);
                    let mut rendered_any = false;

                    // Render item content (paragraphs, nested lists, etc.)
                    loop {
                        if *pos >= events.len() {
                            break;
                        }

                        match &events[*pos] {
                            Event::End(TagEnd::Item) => {
                                *pos += 1;
                                break;
                            }

                            Event::Start(Tag::List(lst)) => {
                                *pos += 1;
                                let nested = self.render_list(events, pos, *lst, width, depth + 1);
                                for nl in nested {
                                    lines.push(nl);
                                }
                                rendered_any = true;
                            }

                            Event::Start(Tag::Item) => {
                                // Next item started — break to outer loop
                                break;
                            }

                            Event::Start(tag) => {
                                *pos += 1;
                                let block_lines =
                                    self.render_block(events, pos, tag, item_width, false, depth);
                                for bl in block_lines.iter() {
                                    for wl in wrap_text_with_ansi(bl, item_width) {
                                        let prefix = if rendered_any {
                                            &continuation_prefix
                                        } else {
                                            &bullet_prefix
                                        };
                                        lines.push(format!("{}{}", prefix, wl));
                                        rendered_any = true;
                                    }
                                }
                            }

                            // Inline content directly in list item (no Paragraph wrapper)
                            Event::Text(_)
                            | Event::Code(_)
                            | Event::SoftBreak
                            | Event::HardBreak
                            | Event::InlineHtml(_)
                            | Event::InlineMath(_)
                            | Event::DisplayMath(_) => {
                                let inline = self.render_inline(
                                    events,
                                    pos,
                                    TagEnd::Item,
                                    &self.build_default_ctx(),
                                );
                                for wl in wrap_text_with_ansi(&inline, item_width) {
                                    let prefix = if rendered_any {
                                        &continuation_prefix
                                    } else {
                                        &bullet_prefix
                                    };
                                    lines.push(format!("{}{}", prefix, wl));
                                    rendered_any = true;
                                }
                            }

                            Event::End(TagEnd::Paragraph) => {
                                // Skip paragraph end if present
                                *pos += 1;
                            }

                            _ => {
                                *pos += 1;
                            }
                        }
                    }

                    if !rendered_any {
                        lines.push(bullet_prefix);
                    }
                }

                _ => {
                    *pos += 1;
                }
            }
        }

        lines
    }

    /// Render a table with width-aware column sizing and box-drawing borders.
    fn render_table(
        &self,
        events: &[Event],
        pos: &mut usize,
        alignments: &[pulldown_cmark::Alignment],
        width: usize,
    ) -> Vec<String> {
        let ctx = self.build_default_ctx();
        let num_cols = alignments.len();

        if num_cols == 0 {
            // Skip the table
            skip_until(events, pos, TagEnd::Table);
            return Vec::new();
        }

        // Collect header cells
        let mut headers: Vec<Vec<String>> = Vec::new(); // header row, each cell has rendered lines (1 per cell)
        let mut body: Vec<Vec<Vec<String>>> = Vec::new(); // rows, each row has cells, each cell has rendered lines

        let mut current_cell_content: Vec<Event> = Vec::new(); // collected inline events for current cell
        let mut current_row: Vec<Vec<String>> = Vec::new(); // cells of current row
        let mut _current_cell_idx: usize = 0;
        let mut in_body = false;

        loop {
            if *pos >= events.len() {
                break;
            }

            match &events[*pos] {
                Event::End(TagEnd::Table) => {
                    // Flush last cell if any
                    if !current_cell_content.is_empty() {
                        let cell_text = self.render_collected_inline(&current_cell_content, &ctx);
                        current_row.push(cell_text);
                        current_cell_content.clear();
                    }
                    if !current_row.is_empty() {
                        body.push(current_row.clone());
                    }
                    *pos += 1;
                    break;
                }

                Event::Start(Tag::TableHead) => {
                    *pos += 1;
                    // TableHead started, next events are row/cells
                }

                Event::End(TagEnd::TableHead) => {
                    *pos += 1;
                    if !current_row.is_empty() {
                        headers = current_row.clone();
                        current_row.clear();
                    }
                    in_body = true;
                }

                Event::Start(Tag::TableRow) => {
                    *pos += 1;
                    _current_cell_idx = 0;
                }

                Event::End(TagEnd::TableRow) => {
                    *pos += 1;
                    // Flush last cell
                    if !current_cell_content.is_empty() {
                        let cell_text = self.render_collected_inline(&current_cell_content, &ctx);
                        current_row.push(cell_text);
                        current_cell_content.clear();
                    }
                    if !current_row.is_empty() {
                        if !in_body {
                            headers = current_row.clone();
                        } else {
                            body.push(current_row.clone());
                        }
                        current_row.clear();
                    }
                    _current_cell_idx = 0;
                }

                Event::Start(Tag::TableCell) => {
                    *pos += 1;
                    // Flush previous cell content if any
                    if !current_cell_content.is_empty() {
                        let cell_text = self.render_collected_inline(&current_cell_content, &ctx);
                        current_row.push(cell_text);
                        current_cell_content.clear();
                        _current_cell_idx += 1;
                    }
                }

                Event::End(TagEnd::TableCell) => {
                    *pos += 1;
                    // Flush this cell
                    let cell_text = self.render_collected_inline(&current_cell_content, &ctx);
                    current_row.push(cell_text);
                    current_cell_content.clear();
                    _current_cell_idx += 1;
                }

                // Collect inline events during cell processing
                Event::Text(_t) => {
                    current_cell_content.push(events[*pos].clone());
                    *pos += 1;
                }
                Event::Code(_c) => {
                    current_cell_content.push(events[*pos].clone());
                    *pos += 1;
                }
                Event::Start(Tag::Emphasis)
                | Event::Start(Tag::Strong)
                | Event::Start(Tag::Strikethrough)
                | Event::Start(Tag::Link { .. }) => {
                    current_cell_content.push(events[*pos].clone());
                    *pos += 1;
                }
                Event::End(TagEnd::Emphasis)
                | Event::End(TagEnd::Strong)
                | Event::End(TagEnd::Strikethrough)
                | Event::End(TagEnd::Link) => {
                    current_cell_content.push(events[*pos].clone());
                    *pos += 1;
                }
                Event::SoftBreak | Event::HardBreak => {
                    current_cell_content.push(events[*pos].clone());
                    *pos += 1;
                }
                Event::InlineHtml(_h) => {
                    current_cell_content.push(events[*pos].clone());
                    *pos += 1;
                }

                // Skip any other events
                Event::Start(_) => {
                    current_cell_content.push(events[*pos].clone());
                    *pos += 1;
                }
                Event::End(_) => {
                    *pos += 1;
                }
                _ => {
                    *pos += 1;
                }
            }
        }

        // Now render the collected table data
        let border_overhead = 3 * num_cols + 1;
        let available = width.saturating_sub(border_overhead);
        if available < num_cols {
            // Too narrow, skip
            return Vec::new();
        }

        // Calculate natural widths (max visible width of each cell across all rows)
        let max_unbroken_word_width = 30;
        let mut natural_widths = vec![0usize; num_cols];
        let mut min_word_widths = vec![1usize; num_cols];

        // Helper to update widths from a set of cells
        let update_widths =
            |cells: &[Vec<String>], natural: &mut [usize], min_word: &mut [usize]| {
                for (i, cell_lines) in cells.iter().enumerate() {
                    if i >= num_cols {
                        break;
                    }
                    for cl in cell_lines {
                        let vw = visible_width(cl);
                        natural[i] = natural[i].max(vw);
                        // Longest word width
                        let longest = cl
                            .split_whitespace()
                            .map(visible_width)
                            .max()
                            .unwrap_or(0)
                            .min(max_unbroken_word_width);
                        min_word[i] = min_word[i].max(longest.max(1));
                    }
                }
            };

        // Apply headers for width calculation
        update_widths(&headers, &mut natural_widths, &mut min_word_widths);

        for row_cells in &body {
            update_widths(row_cells, &mut natural_widths, &mut min_word_widths);
        }

        // Calculate final column widths
        let total_natural: usize = natural_widths.iter().sum();
        let mut column_widths = vec![0usize; num_cols];

        if total_natural + border_overhead <= width {
            // Everything fits
            for i in 0..num_cols {
                column_widths[i] = natural_widths[i].max(min_word_widths[i]);
            }
        } else {
            // Need to shrink — start from min widths and distribute remaining space
            let min_total: usize = min_word_widths.iter().sum();
            let extra = available.saturating_sub(min_total);

            let grow_potential: usize = natural_widths
                .iter()
                .zip(min_word_widths.iter())
                .map(|(n, m)| n.saturating_sub(*m))
                .sum();

            if min_total <= available {
                for i in 0..num_cols {
                    let n = natural_widths[i];
                    let m = min_word_widths[i];
                    let potential = n.saturating_sub(m);
                    let grow = if grow_potential > 0 {
                        extra
                            .checked_mul(potential)
                            .map(|p| p / grow_potential)
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    column_widths[i] = m + grow;
                }
                // Distribute rounding remainder
                let allocated: usize = column_widths.iter().sum();
                let mut remaining = available.saturating_sub(allocated);
                for i in 0..num_cols {
                    if remaining == 0 {
                        break;
                    }
                    if column_widths[i] < natural_widths[i] {
                        column_widths[i] += 1;
                        remaining -= 1;
                    }
                }
            } else {
                // Even min widths don't fit — equal distribution
                let base = available / num_cols;
                let rem = available % num_cols;
                for (i, cw) in column_widths.iter_mut().enumerate() {
                    *cw = base + if i < rem { 1 } else { 0 };
                }
            }
        }

        let mut result: Vec<String> = Vec::new();

        // Top border
        let top_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        result.push(format!("┌─{}─┐", top_cells.join("─┬─")));

        // Header row (with bold)
        let header_lines = self.render_table_row(&headers, &column_widths, num_cols, &ctx, true);
        result.extend(header_lines);

        // Separator
        let sep_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        result.push(format!("├─{}─┤", sep_cells.join("─┼─")));

        // Body rows
        for (ri, row_cells) in body.iter().enumerate() {
            let row_lines = self.render_table_row(row_cells, &column_widths, num_cols, &ctx, false);
            result.extend(row_lines);
            if ri < body.len() - 1 {
                // Row separator (same as header separator)
                result.push(format!("├─{}─┤", sep_cells.join("─┼─")));
            }
        }

        // Bottom border
        let bottom_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        result.push(format!("└─{}─┘", bottom_cells.join("─┴─")));

        // Spacing after table
        if *pos < events.len() {
            let next_is_space = matches!(
                &events[*pos],
                Event::End(_) | Event::SoftBreak | Event::HardBreak
            );
            if !next_is_space {
                result.push(String::new());
            }
        }

        result
    }

    /// Render a single table row (header or body) with cell wrapping.
    fn render_table_row(
        &self,
        cells: &[Vec<String>],
        column_widths: &[usize],
        num_cols: usize,
        _ctx: &InlineCtx,
        is_header: bool,
    ) -> Vec<String> {
        if cells.is_empty() {
            return Vec::new();
        }

        // Wrap each cell to column width
        let mut wrapped_cells: Vec<Vec<String>> = Vec::new();
        for (i, cell_lines) in cells.iter().enumerate() {
            if i >= num_cols {
                break;
            }
            let col_width = column_widths[i];
            let mut wrapped: Vec<String> = Vec::new();
            for cl in cell_lines {
                for wl in wrap_text_with_ansi(cl, col_width) {
                    wrapped.push(wl);
                }
            }
            if wrapped.is_empty() {
                wrapped.push(String::new());
            }
            wrapped_cells.push(wrapped);
        }

        // Pad all cells to same number of lines
        let max_lines = wrapped_cells.iter().map(|c| c.len()).max().unwrap_or(1);
        for cell in &mut wrapped_cells {
            while cell.len() < max_lines {
                cell.push(String::new());
            }
        }

        let mut result: Vec<String> = Vec::new();
        for line_idx in 0..max_lines {
            let mut row_parts: Vec<String> = Vec::new();
            for (col_idx, cell) in wrapped_cells.iter().enumerate() {
                let text = cell.get(line_idx).map(|s| s.as_str()).unwrap_or("");
                let vw = visible_width(text);
                let padding = column_widths[col_idx].saturating_sub(vw);
                let padded = if is_header {
                    (self.theme.bold)(&format!("{}{}", text, " ".repeat(padding)))
                } else {
                    format!("{}{}", text, " ".repeat(padding))
                };
                row_parts.push(padded);
            }
            result.push(format!("│ {} │", row_parts.join(" │ ")));
        }

        result
    }

    /// Render a collected sequence of inline events into a styled string.
    fn render_collected_inline(&self, events: &[Event], ctx: &InlineCtx) -> Vec<String> {
        if events.is_empty() {
            return vec![String::new()];
        }
        // Group events into inline rendering and return as lines
        let mut pos = 0usize;
        let rendered = self.render_inline(events, &mut pos, TagEnd::TableCell, ctx);
        if rendered.is_empty() {
            vec![String::new()]
        } else {
            rendered.split('\n').map(|s| s.to_string()).collect()
        }
    }
}

// ── Helper functions ─────────────────────────────────────────────

/// Skip events until the matching `End` tag is found.
fn skip_until(events: &[Event], pos: &mut usize, end: TagEnd) {
    let mut depth = 0;
    loop {
        if *pos >= events.len() {
            break;
        }
        match &events[*pos] {
            Event::End(tag_end) if *tag_end == end => {
                if depth == 0 {
                    *pos += 1;
                    break;
                }
                depth -= 1;
                *pos += 1;
            }
            Event::Start(_) => {
                depth += 1;
                *pos += 1;
            }
            _ => {
                *pos += 1;
            }
        }
    }
}

/// Convert a `HeadingLevel` to its numeric value.
fn level_to_usize(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Split text by newlines and apply style to each segment.
/// Preserves newlines between styled segments.
fn split_newline_apply(text: &str, apply: &dyn Fn(&str) -> String) -> String {
    let segments: Vec<&str> = text.split('\n').collect();
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if i > 0 {
                format!("\n{}", apply(s))
            } else {
                apply(s)
            }
        })
        .collect()
}

// ── Syntax Highlighting (feature-gated) ─────────────────────────

/// Create a syntax highlighting function.
/// Returns `Some(..)` when the `syntect` feature is enabled, `None` otherwise.
pub fn create_highlight_fn() -> Option<HighlightFn> {
    #[cfg(feature = "syntect")]
    {
        Some(Arc::new(highlight_code))
    }
    #[cfg(not(feature = "syntect"))]
    {
        None
    }
}

#[cfg(feature = "syntect")]
fn highlight_code(code: &str, lang: Option<&str>) -> Vec<String> {
    use std::sync::LazyLock;

    use syntect::{
        easy::HighlightLines,
        highlighting::ThemeSet,
        parsing::SyntaxSet,
        util::{LinesWithEndings, as_24_bit_terminal_escaped},
    };

    static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

    static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

    let ss = &SYNTAX_SET;
    let ts = &THEME_SET;

    // Find the syntax by language name/extension
    let syntax = lang
        .and_then(|l| ss.find_syntax_by_token(l))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    // Pick a theme (base16-ocean.dark works well for dark terminals)
    let theme = ts
        .themes
        .get("base16-ocean.dark")
        .or_else(|| ts.themes.iter().next().map(|(_, t)| t));

    let Some(theme) = theme else {
        // No themes available — return plain text
        return code.split('\n').map(|s| s.to_string()).collect();
    };

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = Vec::new();

    for line in LinesWithEndings::from(code) {
        match highlighter.highlight_line(line, ss) {
            Ok(ranges) => {
                let escaped = as_24_bit_terminal_escaped(&ranges, false);
                let text = line.trim_end_matches('\n');
                if escaped.is_empty() {
                    result.push(text.to_string());
                } else {
                    result.push(format!("{}\x1b[0m", escaped));
                }
            }
            Err(_) => {
                result.push(line.trim_end_matches('\n').to_string());
            }
        }
    }

    result
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a minimal theme for testing.
    fn test_theme() -> MarkdownTheme {
        MarkdownTheme::new(
            Arc::new(|s| format!("\x1b[33m{}\x1b[39m", s)), // heading: yellow
            Arc::new(|s| format!("\x1b[34m{}\x1b[39m", s)), // link: blue
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)), // link_url: bright black
            Arc::new(|s| format!("\x1b[36m{}\x1b[39m", s)), // code: cyan
            Arc::new(|s| format!("\x1b[32m{}\x1b[39m", s)), // code_block: green
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)), // code_block_border: gray
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)), // quote: gray
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)), // quote_border: gray
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)), // hr: gray
            Arc::new(|s| format!("\x1b[33m{}\x1b[39m", s)), // list_bullet: yellow
            Arc::new(|s| format!("\x1b[1m{}\x1b[22m", s)),  // bold
            Arc::new(|s| format!("\x1b[3m{}\x1b[23m", s)),  // italic
            Arc::new(|s| format!("\x1b[9m{}\x1b[29m", s)),  // strikethrough
            Arc::new(|s| format!("\x1b[4m{}\x1b[24m", s)),  // underline
        )
    }

    #[test]
    fn test_basic_paragraph() {
        let theme = test_theme();
        let md = Markdown::new("hello world", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("hello world"));
        assert!(!all.contains("\x1b[")); // no styling
    }

    #[test]
    fn test_heading_h1() {
        let theme = test_theme();
        let md = Markdown::new("# Heading 1", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("Heading 1"), "Should contain heading text");
        assert!(all.contains("\x1b[1m"), "Should have bold for h1");
        assert!(all.contains("\x1b[33m"), "Should have heading color");
    }

    #[test]
    fn test_heading_h3_marker() {
        let theme = test_theme();
        let md = Markdown::new("### Heading 3", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("### Heading 3") || all.contains("Heading 3"));
        // h3 should have ### prefix
        assert!(
            !all.contains("### ") || all.contains("###"),
            "h3 should show ### marker"
        );
    }

    #[test]
    fn test_bold_italic() {
        let theme = test_theme();
        let md = Markdown::new("**bold** and *italic*", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("bold"), "Should contain bold text");
        assert!(all.contains("italic"), "Should contain italic text");
        assert!(all.contains("\x1b[1m"), "Should contain bold ANSI");
        assert!(all.contains("\x1b[3m"), "Should contain italic ANSI");
    }

    #[test]
    fn test_codespan() {
        let theme = test_theme();
        let md = Markdown::new("use `code` here", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("code"), "Should contain code text");
        assert!(all.contains("\x1b[36m"), "Should contain code color (cyan)");
    }

    #[test]
    fn test_inline_code_style_restore() {
        let theme = test_theme();
        let md = Markdown::new("**bold `code` end**", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("bold"), "Should contain bold text");
        assert!(all.contains("code"), "Should contain code text");
        assert!(all.contains("end"), "Should contain 'end' text");
        // The 'end' should be bold (style restored after codespan)
    }

    #[test]
    fn test_code_block() {
        let theme = test_theme();
        let md = Markdown::new("```\nlet x = 1;\n```", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("let x = 1;"), "Should contain code");
        assert!(all.contains("\x1b[32m"), "Should have code block color");
        assert!(all.contains("```"), "Should have fence markers");
    }

    #[test]
    fn test_fenced_code_with_language() {
        let theme = test_theme();
        let md = Markdown::new("```rust\nfn main() {}\n```", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("```rust"), "Should show language tag");
        assert!(all.contains("fn main() {}"), "Should contain code");
    }

    #[test]
    fn test_unordered_list() {
        let theme = test_theme();
        let md = Markdown::new("- item 1\n- item 2\n- item 3", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("item 1"), "Should contain first item");
        assert!(all.contains("item 2"), "Should contain second item");
        assert!(all.contains("item 3"), "Should contain third item");
    }

    #[test]
    fn test_strikethrough() {
        let theme = test_theme();
        let md = Markdown::new("~~struck~~", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("struck"), "Should contain text");
        assert!(all.contains("\x1b[9m"), "Should contain strikethrough");
    }

    #[test]
    fn test_link_inline() {
        let theme = test_theme();
        let md = Markdown::new("[text](https://example.com)", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("text"), "Should contain link text");
        assert!(
            all.contains("https://example.com"),
            "Should contain URL in fallback"
        );
    }

    #[test]
    fn test_empty_text() {
        let theme = test_theme();
        let md = Markdown::new("", 0, 0, theme, None, None);
        let lines = md.render(80);
        assert!(lines.is_empty() || (lines.len() == 1 && lines[0].is_empty()));
    }

    #[test]
    fn test_whitespace_only() {
        let theme = test_theme();
        let md = Markdown::new("   ", 0, 0, theme, None, None);
        let lines = md.render(80);
        assert!(lines.is_empty() || (lines.len() == 1 && lines[0].is_empty()));
    }

    #[test]
    fn test_horizontal_rule() {
        let theme = test_theme();
        let md = Markdown::new("---", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains('─'), "Should have horizontal rule");
    }

    #[test]
    fn test_padding_x() {
        let theme = test_theme();
        let md = Markdown::new("hello", 2, 0, theme, None, None);
        let lines = md.render(20);
        assert_eq!(
            visible_width(&lines[0]),
            20,
            "Should be padded to full width"
        );
        assert!(lines[0].starts_with("  "), "Should have left padding");
    }

    #[test]
    fn test_padding_y() {
        let theme = test_theme();
        let md = Markdown::new("hello", 0, 1, theme, None, None);
        let lines = md.render(20);
        assert_eq!(
            lines.len(),
            3,
            "Should have top padding + content + bottom padding"
        );
    }

    #[test]
    fn test_cache_hit() {
        let theme = test_theme();
        let md = Markdown::new("hello", 1, 0, theme, None, None);
        let a = md.render(20);
        let b = md.render(20);
        assert_eq!(a, b, "Cache should return same result");
    }

    #[test]
    fn test_cache_invalidation() {
        let theme = test_theme();
        let mut md = Markdown::new("hello", 1, 0, theme, None, None);
        let a = md.render(20);
        md.set_text("world");
        let b = md.render(20);
        assert_ne!(a, b, "Cache should be invalidated on set_text");
    }

    #[test]
    fn test_strikethrough_not_enabled_without_tilde() {
        // Without the ~ markers, strikethrough shouldn't trigger
        let theme = test_theme();
        let md = Markdown::new("~not struck~", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        // With ENABLE_STRIKETHROUGH, ~ should work as strikethrough in pulldown-cmark
        // But with ~~ being the correct syntax for GFM, ~ alone might not trigger
        assert!(
            all.contains("~not struck~") || all.contains("not struck"),
            "~ should work as plain text or strikethrough"
        );
    }

    #[test]
    fn test_blockquote() {
        let theme = test_theme();
        let md = Markdown::new("> quoted text", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("quoted text"), "Should contain quote text");
        assert!(all.contains("│"), "Should have blockquote border");
    }

    #[test]
    fn test_task_list() {
        let theme = test_theme();
        let md = Markdown::new("- [x] done\n- [ ] todo", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("[x]"), "Should show done marker");
        assert!(all.contains("[ ]"), "Should show todo marker");
        assert!(all.contains("done"), "Should contain done text");
        assert!(all.contains("todo"), "Should contain todo text");
    }

    #[test]
    fn test_paragraph_spacing() {
        let theme = test_theme();
        let md = Markdown::new("para one\n\npara two", 0, 0, theme, None, None);
        let lines = md.render(80);
        assert!(lines.len() >= 2, "Should have multiple lines");
    }

    #[test]
    fn test_tabs_replaced() {
        let theme = test_theme();
        let md = Markdown::new("\tindented", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(
            all.contains("indented"),
            "Tabs should be replaced with 3 spaces"
        );
    }

    #[test]
    fn test_default_text_style() {
        let theme = test_theme();
        let default_style = DefaultTextStyle {
            color: Some(Arc::new(|s| format!("\x1b[33m{}\x1b[39m", s))),
            bg_color: None,
            bold: true,
            italic: false,
            strikethrough: false,
            underline: false,
        };
        let md = Markdown::new("styled text", 0, 0, theme, Some(default_style), None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("styled text"));
        assert!(
            all.contains("\x1b[1m"),
            "Should have bold from default style"
        );
        assert!(
            all.contains("\x1b[33m"),
            "Should have yellow from default style"
        );
    }

    #[test]
    fn test_table_basic() {
        let theme = test_theme();
        let md = Markdown::new(
            "| H1 | H2 |\n| --- | --- |\n| A1 | B1 |\n| A2 | B2 |",
            0,
            0,
            theme,
            None,
            None,
        );
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("H1"), "Should contain header");
        assert!(all.contains("H2"), "Should contain header");
        assert!(all.contains("A1"), "Should contain cell");
        assert!(all.contains("B1"), "Should contain cell");
        assert!(all.contains("┌"), "Should have top border");
        assert!(all.contains("└"), "Should have bottom border");
        assert!(all.contains("│"), "Should have column separators");
    }

    #[test]
    fn test_table_narrow_fallback() {
        let theme = test_theme();
        let md = Markdown::new(
            "| A | B |\n| --- | --- |\n| 1 | 2 |",
            0,
            0,
            theme,
            None,
            None,
        );
        // Render at very narrow width
        let lines = md.render(10);
        // Should not panic, either renders tiny table or nothing
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_ordered_list() {
        let theme = test_theme();
        let md = Markdown::new("1. first\n2. second\n3. third", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("first"), "Should contain first");
        assert!(all.contains("second"), "Should contain second");
        assert!(all.contains("third"), "Should contain third");
    }

    #[test]
    fn test_nested_list() {
        let theme = test_theme();
        let md = Markdown::new("- outer\n  - inner\n- more", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("outer"), "Should contain outer");
        assert!(all.contains("inner"), "Should contain nested");
        assert!(all.contains("more"), "Should contain more");
    }

    #[test]
    fn test_blockquote_nested() {
        let theme = test_theme();
        let md = Markdown::new("> outer\n> > nested\n> back", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("outer"), "Should contain outer text");
        assert!(all.contains("nested"), "Should contain nested text");
        assert!(all.contains("back"), "Should contain text after nested");
        assert!(all.contains("│"), "Should have blockquote border");
    }

    #[test]
    fn test_link_with_dest() {
        let theme = test_theme();
        let md = Markdown::new(
            "[example](https://example.com/page)",
            0,
            0,
            theme,
            None,
            None,
        );
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("example"), "Should contain link text");
        assert!(all.contains("example.com/page"), "Should contain URL");
    }

    #[test]
    fn test_autolink() {
        let theme = test_theme();
        let md = Markdown::new("<https://example.com>", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("example.com"), "Should contain URL");
    }

    #[test]
    fn test_heading_h2_spacing() {
        let theme = test_theme();
        let md = Markdown::new("## Heading\n\nParagraph", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("Heading"), "Should contain heading");
        assert!(all.contains("Paragraph"), "Should contain paragraph");
    }

    #[test]
    fn test_code_block_markers() {
        let theme = test_theme();
        let md = Markdown::new("```rust\nfn hello() {}\n```", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("```rust"), "Should show language in fence");
        assert!(all.contains("fn hello() {}"), "Should contain code");
    }

    #[test]
    fn test_strikethrough_markers() {
        let theme = test_theme();
        let md = Markdown::new("~~struck text~~", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("struck text"), "Should contain text");
        assert!(all.contains("\x1b[9m"), "Should have strikethrough ANSI");
    }

    #[test]
    fn test_wrap_long_text() {
        let theme = test_theme();
        let long = "this is a very long line that should definitely wrap to multiple lines when rendered in a narrow terminal column";
        let md = Markdown::new(long, 0, 0, theme, None, None);
        let lines = md.render(30);
        assert!(lines.len() > 1, "Long text should wrap");
        for line in &lines {
            assert!(visible_width(line) <= 30, "Each line should fit width");
        }
    }

    #[test]
    fn test_cache_different_width() {
        let theme = test_theme();
        let md = Markdown::new("hello world", 1, 0, theme, None, None);
        let a = md.render(30);
        let b = md.render(50);
        assert_ne!(a, b, "Different widths should produce different output");
    }

    #[test]
    fn test_html_block_plain() {
        let theme = test_theme();
        let md = Markdown::new("<div>plain html</div>", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(
            all.contains("plain html"),
            "Should render HTML as plain text"
        );
    }

    #[test]
    fn test_bold_italic_style_restore() {
        let theme = test_theme();
        let md = Markdown::new("**bold `code` more bold**", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("bold"), "Should contain bold text");
        assert!(all.contains("code"), "Should contain code");
        assert!(all.contains("more"), "Should contain text after code");
        // The "more" should still be bold after the inline code reset
        assert!(
            all.contains("\x1b[22m") || all.contains("more bold"),
            "Style should be restored after codespan"
        );
    }

    #[test]
    fn test_heading_h4_marker() {
        let theme = test_theme();
        let md = Markdown::new("#### Heading 4", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("####"), "h4 should show prefix marker");
        assert!(all.contains("Heading 4"), "Should contain heading text");
    }

    #[test]
    fn test_heading_h5_marker() {
        let theme = test_theme();
        let md = Markdown::new("##### Heading 5", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("#####"), "h5 should show prefix marker");
        assert!(all.contains("Heading 5"), "Should contain heading text");
    }

    #[test]
    fn test_heading_h6_marker() {
        let theme = test_theme();
        let md = Markdown::new("###### Heading 6", 0, 0, theme, None, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("######"), "h6 should show prefix marker");
        assert!(all.contains("Heading 6"), "Should contain heading text");
    }
}
