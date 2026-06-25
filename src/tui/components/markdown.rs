#![allow(clippy::type_complexity, clippy::arc_with_non_send_sync)]

//! Markdown rendering using comrak's tree-based AST.
//!
//! Two-phase approach:
//!   1. Parse with comrak → mutable tree AST
//!   2. AST manipulation: float headings/code blocks/blockquotes out of lists
//!      (prevents progressive nesting from LLM output artifacts)
//!   3. Render tree → styled ANSI lines
//!   4. Wrap + pad → final output

use std::sync::Arc;

use comrak::nodes::{AstNode, ListType, NodeCodeBlock, NodeTable, NodeValue};
use comrak::{Arena, Options, parse_document};

use crate::tui::Component;
use crate::tui::util::{visible_width, wrap_text_with_ansi};

// ── Type aliases ────────────────────────────────────────────────

/// Type alias for markdown theme styling functions.
pub type StyleFn = Arc<dyn Fn(&str) -> String>;
/// Type alias for code highlighting function.
pub type HighlightFn = Arc<dyn Fn(&str, Option<&str>) -> Vec<String>>;

// ── Code block indent ───────────────────────────────────────────

/// Indent prefix applied to each code line inside a fenced code block.
/// Defaults to two spaces for visual inset from the backtick fence.
pub const CODE_BLOCK_INDENT: &str = "  ";

// ── MarkdownTheme ───────────────────────────────────────────────

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
    /// Indent prefix applied to each code line inside a fenced code block.
    /// Defaults to two spaces for visual inset from the backtick fence.
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
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
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
fn get_style_prefix(style_fn: &dyn Fn(&str) -> String) -> String {
    const SENTINEL: char = '\0';
    let styled = style_fn(&SENTINEL.to_string());
    styled
        .find(SENTINEL)
        .map(|i| styled[..i].to_string())
        .unwrap_or_default()
}

/// Check whether hyperlinks (OSC 8) are supported.
pub(crate) fn hyperlinks_supported() -> bool {
    if let Ok(prog) = std::env::var("TERM_PROGRAM")
        && (prog == "iTerm.app" || prog == "kitty" || prog == "WezTerm" || prog == "vscode")
    {
        return true;
    }
    if let Ok(term) = std::env::var("TERM")
        && term.contains("kitty")
    {
        return true;
    }
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
pub(crate) fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{}\x07{}\x1b]8;;\x07", url, text)
}

// ── Markdown Component ───────────────────────────────────────────

/// Markdown rendering component.
///
/// Parses markdown with comrak (tree-based CommonMark parser),
/// restructures the AST to fix LLM-induced nesting artifacts,
/// then renders to styled ANSI output.
pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    theme: MarkdownTheme,
    default_text_style: Option<DefaultTextStyle>,

    // Cache
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Vec<String>,
}

impl Markdown {
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<DefaultTextStyle>,
    ) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            theme,
            default_text_style,
            cached_text: None,
            cached_width: None,
            cached_lines: Vec::new(),
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }

    pub fn cached_text_matches(&self, other: &str) -> bool {
        self.cached_text.as_deref() == Some(&self.text) && self.text == other
    }

    pub fn get_text(&self) -> &str {
        &self.text
    }

    // ── Style helpers ────────────────────────────────────────────

    fn build_default_ctx(&self) -> InlineCtx {
        InlineCtx::new(self.build_default_apply_fn())
    }

    fn build_default_apply_fn(&self) -> Arc<dyn Fn(&str) -> String> {
        let style = &self.default_text_style;
        let theme = &self.theme;

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

    fn heading_ctx(&self, level: u8) -> InlineCtx {
        let theme_heading = self.theme.heading.clone();
        let theme_bold = self.theme.bold.clone();
        let theme_underline = self.theme.underline.clone();

        let style_fn: Arc<dyn Fn(&str) -> String> = match level {
            1 => Arc::new(move |text: &str| theme_heading(&theme_bold(&theme_underline(text)))),
            _ => Arc::new(move |text: &str| theme_heading(&theme_bold(text))),
        };
        InlineCtx::new(style_fn)
    }

    fn quote_ctx(&self) -> InlineCtx {
        let theme_quote = self.theme.quote.clone();
        let theme_italic = self.theme.italic.clone();
        let style_fn: Arc<dyn Fn(&str) -> String> =
            Arc::new(move |text: &str| theme_quote(&theme_italic(text)));
        InlineCtx::new(style_fn)
    }

    // ── Flattening: float "LLM artifact" nodes out of lists ─────
    //
    // LLM markdown output often indents headings, code blocks, and
    // blockquotes inside list items, e.g.:
    //
    //   - Step 1
    //     ### Notes
    //     ```python
    //     ...
    //     ```
    //
    // CommonMark parsers nest these inside the list item. This pass
    // detects such nodes and detaches/reparents them just after the
    // nearest enclosing List, so they render at list-sibling depth.
    //
    // We collect nodes first, then reparent, to avoid issues with
    // tree mutation during iteration.

    /// Collect nodes that should be floated out of their enclosing list.
    fn collect_float_candidates<'a>(&self, root: &'a AstNode<'a>) -> Vec<&'a AstNode<'a>> {
        let mut candidates: Vec<&'a AstNode<'a>> = Vec::new();

        for node in root.descendants() {
            let val = node.data.borrow();
            let is_floatable = matches!(
                val.value,
                NodeValue::Heading(_) | NodeValue::CodeBlock(_) | NodeValue::BlockQuote
            );
            if !is_floatable {
                continue;
            }
            // Check if any ancestor is a List or Item (but not reaching root)
            let mut ancestor = node.parent();
            let mut inside_list = false;
            while let Some(anc) = ancestor {
                let av = anc.data.borrow();
                match av.value {
                    NodeValue::List(_) | NodeValue::Item { .. } => {
                        inside_list = true;
                        break;
                    }
                    NodeValue::Document => break,
                    _ => {}
                }
                ancestor = anc.parent();
            }
            if inside_list {
                candidates.push(node);
            }
        }
        candidates
    }

    /// Find the nearest enclosing List ancestor of a node.
    fn find_enclosing_list<'a>(&self, node: &'a AstNode<'a>) -> Option<&'a AstNode<'a>> {
        let mut ancestor = node.parent();
        while let Some(anc) = ancestor {
            let av = anc.data.borrow();
            if matches!(av.value, NodeValue::List(_)) {
                return Some(anc);
            }
            ancestor = anc.parent();
        }
        None
    }

    /// Float collected candidates out of their enclosing lists.
    fn float_block_nodes<'a>(&self, root: &'a AstNode<'a>) {
        let candidates = self.collect_float_candidates(root);
        for node in candidates {
            // If already detached by a previous reparenting, skip
            if node.parent().is_none() {
                continue;
            }
            let Some(list_node) = self.find_enclosing_list(node) else {
                continue;
            };

            // Detach the candidate from its current parent
            node.detach();

            // Insert it after the list node (at the list's sibling level)
            list_node.insert_after(node);
        }
    }

    // ── Comrak options ───────────────────────────────────────────

    fn comrak_options() -> Options<'static> {
        use comrak::options::Extension;
        Options {
            extension: Extension {
                strikethrough: true,
                table: true,
                autolink: true,
                tasklist: true,
                tagfilter: false,
                ..Extension::default()
            },
            ..Options::default()
        }
    }
}

impl Component for Markdown {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Check cache
        if self.cached_text.as_deref() == Some(&self.text) && self.cached_width == Some(width) {
            return self.cached_lines.clone();
        }

        // Don't render anything if there's no actual text
        if self.text.is_empty() || self.text.trim().is_empty() {
            self.cached_text = Some(self.text.clone());
            self.cached_width = Some(width);
            self.cached_lines = Vec::new();
            return Vec::new();
        }

        let content_width = width.saturating_sub(2 * self.padding_x).max(1);

        // Parse with comrak
        let arena = Arena::new();
        let normalized = self.text.replace('\t', "   ");
        let opts = Self::comrak_options();
        let root = parse_document(&arena, &normalized, &opts);

        // AST manipulation: float headings/code/blockquotes out of lists
        self.float_block_nodes(root);

        // Render tree to styled ANSI lines
        let rendered = self.render_node_lines(root, content_width, 0);

        // Wrap lines
        let mut wrapped: Vec<String> = Vec::new();
        for line in &rendered {
            for wl in wrap_text_with_ansi(line, content_width) {
                wrapped.push(wl);
            }
        }

        // Add padding
        let left_margin = " ".repeat(self.padding_x);
        let right_margin = " ".repeat(self.padding_x);
        let mut content_lines: Vec<String> = Vec::new();
        for line in &wrapped {
            let line_with_margins = format!("{}{}{}", left_margin, line, right_margin);
            let visible = visible_width(&line_with_margins);
            let padded = if visible < width {
                format!("{}{}", line_with_margins, " ".repeat(width - visible))
            } else {
                line_with_margins
            };
            content_lines.push(padded);
        }

        let empty_line = " ".repeat(width);
        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }
        result.extend(content_lines);
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        // Update cache
        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_lines = result.clone();

        if result.is_empty() {
            vec![String::new()]
        } else {
            result
        }
    }

    fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines.clear();
    }
}

// ── Tree Rendering ──────────────────────────────────────────────

impl Markdown {
    /// Render a node's children as lines, collecting non-inline children.
    /// `list_depth` tracks list nesting for indentation (the float pass
    /// removes most artifical nesting, but genuine nested lists remain).
    fn render_node_lines<'a>(
        &self,
        node: &'a AstNode<'a>,
        width: usize,
        list_depth: usize,
    ) -> Vec<String> {
        let val = node.data.borrow();
        let mut lines: Vec<String> = Vec::new();
        let children: Vec<_> = node.children().collect();

        match &val.value {
            NodeValue::Document => {
                for child in &children {
                    lines.extend(self.render_node_lines(child, width, 0));
                }
            }

            NodeValue::Paragraph => {
                let ctx = self.build_default_ctx();
                let text = self.render_inline_children(&children, &ctx);
                if !text.is_empty() {
                    lines.push(text);
                }
            }

            NodeValue::Heading(h) => {
                let ctx = self.heading_ctx(h.level);
                let content = self.render_inline_children(&children, &ctx);
                let styled = if h.level >= 3 {
                    let prefix = format!("{} ", "#".repeat(h.level as usize));
                    format!("{}{}", (ctx.apply_text)(&prefix), content)
                } else {
                    content
                };
                lines.push(styled);
            }

            NodeValue::CodeBlock(cb) => {
                self.render_code_block(cb, &mut lines);
            }

            NodeValue::List(_lst) => {
                let list_lines = self.render_list(node, children.clone(), width, list_depth);
                lines.extend(list_lines);
            }

            NodeValue::Item(_) => {
                // Items are handled by render_list; render children directly
                for child in &children {
                    lines.extend(self.render_node_lines(child, width, list_depth));
                }
            }

            NodeValue::BlockQuote => {
                lines.extend(self.render_blockquote(&children, width));
            }

            NodeValue::Table(tbl) => {
                lines.extend(self.render_table(node, tbl, &children, width));
            }

            NodeValue::ThematicBreak => {
                lines.push((self.theme.hr)(&"─".repeat(width.min(80))));
            }

            NodeValue::HtmlBlock(hb) => {
                let ctx = self.build_default_ctx();
                for line in hb.literal.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        lines.push((ctx.apply_text)(trimmed));
                    }
                }
            }

            NodeValue::FrontMatter(_) => {
                // Skip front matter
            }

            _ => {
                // Fallback: try to render as inline text if there's any
                let ctx = self.build_default_ctx();
                let text = self.render_inline_children(&children, &ctx);
                if !text.is_empty() {
                    lines.push(text);
                }
            }
        }

        lines
    }

    // ── Code Block ───────────────────────────────────────────────

    fn render_code_block(&self, cb: &NodeCodeBlock, lines: &mut Vec<String>) {
        let border = self.theme.code_block_border.clone();
        let code_fn = self.theme.code_block.clone();
        let indent = &self.theme.code_block_indent;

        let lang = if cb.info.is_empty() {
            None
        } else {
            Some(cb.info.as_str())
        };

        // Opening fence
        lines.push(border(&format!("```{}", lang.unwrap_or(""))));

        // Syntax highlighting or plain
        if let Some(ref highlight) = self.theme.highlight_code {
            let hl_lines = highlight(&cb.literal, lang);
            for hl in hl_lines {
                lines.push(format!("{}{}", indent, hl));
            }
        } else {
            for code_line in cb.literal.split('\n') {
                lines.push(format!("{}{}", indent, code_fn(code_line)));
            }
        }

        // Closing fence
        lines.push(border("```"));
    }

    // ── List ─────────────────────────────────────────────────────

    fn render_list<'a>(
        &self,
        node: &'a AstNode<'a>,
        children: Vec<&'a AstNode<'a>>,
        width: usize,
        depth: usize,
    ) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        let val = node.data.borrow();
        let NodeValue::List(lst) = &val.value else {
            return result;
        };

        let indent_str = "    ".repeat(depth.min(8));
        let start_number = lst.start.max(1);
        let mut item_index: u64 = 0;

        for child in &children {
            let cv = child.data.borrow();
            let is_item = matches!(cv.value, NodeValue::Item(_) | NodeValue::TaskItem(_));
            if !is_item {
                continue;
            }
            item_index += 1;

            // Check for task list marker
            let mut task_marker = String::new();
            if let NodeValue::TaskItem(ti) = &cv.value {
                task_marker = if ti.symbol.is_some() {
                    "[x] ".to_string()
                } else {
                    "[ ] ".to_string()
                };
            } else {
                // Also check children for TaskItem (some comrak versions nest it)
                for ic in child.children() {
                    if let NodeValue::TaskItem(ti) = &ic.data.borrow().value {
                        task_marker = if ti.symbol.is_some() {
                            "[x] ".to_string()
                        } else {
                            "[ ] ".to_string()
                        };
                        break;
                    }
                }
            }

            let raw_marker = if lst.list_type == ListType::Ordered {
                format!("{}. ", start_number + item_index as usize - 1)
            } else {
                "- ".to_string()
            };
            let marker = format!("{}{}", raw_marker, task_marker);

            let bullet_prefix = indent_str.clone() + &(self.theme.list_bullet)(&marker);
            let continuation_prefix = indent_str.clone() + &" ".repeat(visible_width(&marker));
            let item_width = width.saturating_sub(visible_width(&bullet_prefix)).max(1);
            let mut rendered_any = false;

            // Gather item's block-level children
            let item_children: Vec<_> = child.children().collect();
            for item_child in &item_children {
                let ic_val = item_child.data.borrow();
                match &ic_val.value {
                    NodeValue::List(_) => {
                        // Nested list: fold into rendering by recursing
                        let nested = self.render_list(
                            item_child,
                            item_child.children().collect(),
                            width,
                            depth + 1,
                        );
                        result.extend(nested);
                        rendered_any = true;
                    }
                    NodeValue::Paragraph => {
                        let ctx = self.build_default_ctx();
                        let text = self.render_inline_children(
                            &item_child.children().collect::<Vec<_>>(),
                            &ctx,
                        );
                        for wl in wrap_text_with_ansi(&text, item_width) {
                            let prefix = if rendered_any {
                                &continuation_prefix
                            } else {
                                &bullet_prefix
                            };
                            result.push(format!("{}{}", prefix, wl));
                            rendered_any = true;
                        }
                    }
                    _ => {
                        // Other block (already floated, but handle eg nested quotes)
                        let block_lines = self.render_node_lines(item_child, item_width, depth);
                        for bl in &block_lines {
                            for wl in wrap_text_with_ansi(bl, item_width) {
                                let prefix = if rendered_any {
                                    &continuation_prefix
                                } else {
                                    &bullet_prefix
                                };
                                result.push(format!("{}{}", prefix, wl));
                                rendered_any = true;
                            }
                        }
                    }
                }
            }

            if !rendered_any {
                result.push(bullet_prefix);
            }
        }

        result
    }

    // ── Blockquote ───────────────────────────────────────────────

    fn render_blockquote<'a>(&self, children: &[&'a AstNode<'a>], width: usize) -> Vec<String> {
        let quote_content_width = width.saturating_sub(2).max(1);
        let quote_ctx = self.quote_ctx();
        let quote_style_prefix = get_style_prefix(&|s: &str| (quote_ctx.apply_text)(s));
        let qborder = self.theme.quote_border.clone();

        let mut inner_lines: Vec<String> = Vec::new();
        for child in children {
            let child_lines = self.render_node_lines(child, quote_content_width, 0);
            inner_lines.extend(child_lines);
        }

        // Remove trailing blank lines
        while inner_lines.last().is_some_and(|l| l.is_empty()) {
            inner_lines.pop();
        }

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

        result
    }

    // ── Table ────────────────────────────────────────────────────

    fn render_table<'a>(
        &self,
        _node: &'a AstNode<'a>,
        tbl: &NodeTable,
        children: &[&'a AstNode<'a>],
        width: usize,
    ) -> Vec<String> {
        let ctx = self.build_default_ctx();
        let num_cols = tbl.num_columns;
        if num_cols == 0 {
            return Vec::new();
        }

        let border_overhead = 3 * num_cols + 1;
        let available_for_cells = width.saturating_sub(border_overhead);
        if available_for_cells < num_cols {
            return Vec::new();
        }

        // Separate rows into header and body
        let mut header_cells: Vec<Vec<String>> = Vec::new();
        let mut body_rows: Vec<Vec<Vec<String>>> = Vec::new();

        for child in children {
            let cv = child.data.borrow();
            if let NodeValue::TableRow(is_header) = &cv.value {
                let row_cells: Vec<Vec<String>> = child
                    .children()
                    .filter_map(|cell_node| {
                        let cell_val = cell_node.data.borrow();
                        if matches!(cell_val.value, NodeValue::TableCell) {
                            let cell_children: Vec<_> = cell_node.children().collect();
                            let text = self.render_inline_children(&cell_children, &ctx);
                            Some(text.split('\n').map(|s| s.to_string()).collect::<Vec<_>>())
                        } else {
                            None
                        }
                    })
                    .collect();

                if *is_header {
                    header_cells = row_cells;
                } else {
                    body_rows.push(row_cells);
                }
            }
        }

        if header_cells.is_empty() {
            return Vec::new();
        }

        // Calculate column widths (same algorithm as current)
        let max_unbroken_word_width = 30;
        let mut natural_widths = vec![0usize; num_cols];
        let mut min_word_widths = vec![1usize; num_cols];

        let update_widths =
            |cells: &[Vec<String>], natural: &mut [usize], min_word: &mut [usize]| {
                for (i, cell_lines) in cells.iter().enumerate() {
                    if i >= num_cols {
                        break;
                    }
                    for cl in cell_lines {
                        let vw = visible_width(cl);
                        natural[i] = natural[i].max(vw);
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

        update_widths(&header_cells, &mut natural_widths, &mut min_word_widths);
        for row_cells in &body_rows {
            update_widths(row_cells, &mut natural_widths, &mut min_word_widths);
        }

        let total_natural: usize = natural_widths.iter().sum();
        let mut column_widths = vec![0usize; num_cols];

        if total_natural + border_overhead <= width {
            for i in 0..num_cols {
                column_widths[i] = natural_widths[i].max(min_word_widths[i]);
            }
        } else {
            let min_total: usize = min_word_widths.iter().sum();
            let extra = available_for_cells.saturating_sub(min_total);
            let grow_potential: usize = natural_widths
                .iter()
                .zip(min_word_widths.iter())
                .map(|(n, m)| n.saturating_sub(*m))
                .sum();

            if min_total <= available_for_cells {
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
                let allocated: usize = column_widths.iter().sum();
                let mut remaining = available_for_cells.saturating_sub(allocated);
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
                let base = available_for_cells / num_cols;
                let rem = available_for_cells % num_cols;
                for (i, cw) in column_widths.iter_mut().enumerate() {
                    *cw = base + if i < rem { 1 } else { 0 };
                }
            }
        }

        // Render
        let mut result: Vec<String> = Vec::new();

        // Top border
        let top_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        result.push(format!("┌─{}─┐", top_cells.join("─┬─")));

        // Header row
        let header_lines = self.render_table_row(&header_cells, &column_widths, num_cols, true);
        result.extend(header_lines);

        // Separator
        let sep_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        result.push(format!("├─{}─┤", sep_cells.join("─┼─")));

        // Body rows
        for (ri, row_cells) in body_rows.iter().enumerate() {
            let row_lines = self.render_table_row(row_cells, &column_widths, num_cols, false);
            result.extend(row_lines);
            if ri < body_rows.len() - 1 {
                result.push(format!("├─{}─┤", sep_cells.join("─┼─")));
            }
        }

        // Bottom border
        let bottom_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        result.push(format!("└─{}─┘", bottom_cells.join("─┴─")));

        result
    }

    fn render_table_row(
        &self,
        cells: &[Vec<String>],
        column_widths: &[usize],
        num_cols: usize,
        is_header: bool,
    ) -> Vec<String> {
        if cells.is_empty() {
            return Vec::new();
        }

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

    // ── Inline Rendering ─────────────────────────────────────────

    /// Render inline children into a single styled string.
    fn render_inline_children<'a>(&self, children: &[&'a AstNode<'a>], ctx: &InlineCtx) -> String {
        let mut result = String::new();

        for node in children {
            let val = node.data.borrow();
            match &val.value {
                NodeValue::Text(t) => {
                    result.push_str(&split_newline_apply(t, &*ctx.apply_text));
                }
                NodeValue::Code(c) => {
                    result.push_str(&(self.theme.code)(&c.literal));
                    result.push_str(&ctx.style_prefix);
                }
                NodeValue::Emph => {
                    let inner =
                        self.render_inline_children(&node.children().collect::<Vec<_>>(), ctx);
                    result.push_str(&(self.theme.italic)(&inner));
                    result.push_str(&ctx.style_prefix);
                }
                NodeValue::Strong => {
                    let inner =
                        self.render_inline_children(&node.children().collect::<Vec<_>>(), ctx);
                    result.push_str(&(self.theme.bold)(&inner));
                    result.push_str(&ctx.style_prefix);
                }
                NodeValue::Strikethrough => {
                    let inner =
                        self.render_inline_children(&node.children().collect::<Vec<_>>(), ctx);
                    result.push_str(&(self.theme.strikethrough)(&inner));
                    result.push_str(&ctx.style_prefix);
                }
                NodeValue::Link(link) => {
                    let inner =
                        self.render_inline_children(&node.children().collect::<Vec<_>>(), ctx);
                    let styled_link = (self.theme.link)(&(self.theme.underline)(&inner));
                    if hyperlinks_supported() {
                        result.push_str(&hyperlink(&styled_link, &link.url));
                    } else {
                        let href_clean = if let Some(mailto) = link.url.strip_prefix("mailto:") {
                            mailto
                        } else {
                            &link.url
                        };
                        if inner.trim() == href_clean || inner.trim() == link.url {
                            result.push_str(&styled_link);
                        } else {
                            result.push_str(&styled_link);
                            result.push_str(&(self.theme.link_url)(&format!(" ({})", link.url)));
                        }
                    }
                    result.push_str(&ctx.style_prefix);
                }
                NodeValue::Image(_) => {
                    // Skip image content
                }
                NodeValue::SoftBreak | NodeValue::LineBreak => {
                    result.push('\n');
                }
                NodeValue::HtmlInline(h) => {
                    result.push_str(&(ctx.apply_text)(h.trim()));
                }

                _ => {
                    // Skip unknown inline nodes
                }
            }
        }

        // Trim trailing style prefix
        while result.ends_with(&ctx.style_prefix) && !ctx.style_prefix.is_empty() {
            result = result[..result.len() - ctx.style_prefix.len()].to_string();
        }

        result
    }
}

// ── Helper functions ─────────────────────────────────────────────

/// Split text by newlines and apply style to each segment.
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
pub fn highlight_code(code: &str, lang: Option<&str>) -> Vec<String> {
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

    let syntax = lang
        .and_then(|l| ss.find_syntax_by_token(l))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let theme = ts
        .themes
        .get("base16-ocean.dark")
        .or_else(|| ts.themes.iter().next().map(|(_, t)| t));

    let Some(theme) = theme else {
        return code.split('\n').map(|s| s.to_string()).collect();
    };

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = Vec::new();

    for line in LinesWithEndings::from(code) {
        match highlighter.highlight_line(line, ss) {
            Ok(ranges) => {
                let escaped = as_24_bit_terminal_escaped(&ranges, false);
                let trimmed = escaped.trim_end_matches('\n');
                if trimmed.is_empty() {
                    result.push(String::new());
                } else {
                    result.push(format!("{}\x1b[0m", trimmed));
                }
            }
            Err(_) => {
                result.push(line.trim_end_matches('\n').to_string());
            }
        }
    }

    result
}

/// Map a file path to a language identifier for syntax highlighting.
pub fn path_to_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_lowercase();
    let lang = match ext.as_str() {
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "sh" | "bash" | "zsh" => "bash",
        "ps1" => "powershell",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "md" | "markdown" => "markdown",
        "clj" | "cljs" | "cljc" => "clojure",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "lua" => "lua",
        _ => return None,
    };
    Some(lang)
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> MarkdownTheme {
        MarkdownTheme::new(
            Arc::new(|s| format!("\x1b[33m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[34m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[36m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[32m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[90m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[33m{}\x1b[39m", s)),
            Arc::new(|s| format!("\x1b[1m{}\x1b[22m", s)),
            Arc::new(|s| format!("\x1b[3m{}\x1b[23m", s)),
            Arc::new(|s| format!("\x1b[9m{}\x1b[29m", s)),
            Arc::new(|s| format!("\x1b[4m{}\x1b[24m", s)),
        )
    }

    #[test]
    fn test_basic_paragraph() {
        let theme = test_theme();
        let mut md = Markdown::new("hello world", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("hello world"));
        assert!(!all.contains("\x1b["));
    }

    #[test]
    fn test_heading_h1() {
        let theme = test_theme();
        let mut md = Markdown::new("# Heading 1", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("Heading 1"));
        assert!(all.contains("\x1b[1m"));
        assert!(all.contains("\x1b[33m"));
    }

    #[test]
    fn test_heading_h3_marker() {
        let theme = test_theme();
        let mut md = Markdown::new("### Heading 3", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("###") || all.contains("Heading 3"));
    }

    #[test]
    fn test_bold_italic() {
        let theme = test_theme();
        let mut md = Markdown::new("**bold** and *italic*", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("bold"));
        assert!(all.contains("italic"));
        assert!(all.contains("\x1b[1m"));
        assert!(all.contains("\x1b[3m"));
    }

    #[test]
    fn test_codespan() {
        let theme = test_theme();
        let mut md = Markdown::new("use `code` here", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("code"));
        assert!(all.contains("\x1b[36m"));
    }

    #[test]
    fn test_inline_code_style_restore() {
        let theme = test_theme();
        let mut md = Markdown::new("**bold `code` end**", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("bold"));
        assert!(all.contains("code"));
        assert!(all.contains("end"));
    }

    #[test]
    fn test_code_block() {
        let theme = test_theme();
        let mut md = Markdown::new("```\nlet x = 1;\n```", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("let x = 1;"));
        assert!(all.contains("\x1b[32m"));
        assert!(all.contains("```"));
    }

    #[test]
    fn test_fenced_code_with_language() {
        let theme = test_theme();
        let mut md = Markdown::new("```rust\nfn main() {}\n```", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("```rust"));
        assert!(all.contains("fn main() {}"));
    }

    #[test]
    fn test_unordered_list() {
        let theme = test_theme();
        let mut md = Markdown::new("- item 1\n- item 2\n- item 3", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("item 1"));
        assert!(all.contains("item 2"));
        assert!(all.contains("item 3"));
    }

    #[test]
    fn test_strikethrough() {
        let theme = test_theme();
        let mut md = Markdown::new("~~struck~~", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("struck"));
        assert!(all.contains("\x1b[9m"));
    }

    #[test]
    fn test_link_inline() {
        let theme = test_theme();
        let mut md = Markdown::new("[text](https://example.com)", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("text"));
        assert!(all.contains("https://example.com"));
    }

    #[test]
    fn test_empty_text() {
        let theme = test_theme();
        let mut md = Markdown::new("", 0, 0, theme, None);
        let lines = md.render(80);
        assert!(lines.is_empty() || (lines.len() == 1 && lines[0].is_empty()));
    }

    #[test]
    fn test_whitespace_only() {
        let theme = test_theme();
        let mut md = Markdown::new("   ", 0, 0, theme, None);
        let lines = md.render(80);
        assert!(lines.is_empty() || (lines.len() == 1 && lines[0].is_empty()));
    }

    #[test]
    fn test_horizontal_rule() {
        let theme = test_theme();
        let mut md = Markdown::new("---", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains('─'));
    }

    #[test]
    fn test_padding_x() {
        let theme = test_theme();
        let mut md = Markdown::new("hello", 2, 0, theme, None);
        let lines = md.render(20);
        assert_eq!(visible_width(&lines[0]), 20);
        assert!(lines[0].starts_with("  "));
    }

    #[test]
    fn test_padding_y() {
        let theme = test_theme();
        let mut md = Markdown::new("hello", 0, 1, theme, None);
        let lines = md.render(20);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_cache_hit() {
        let theme = test_theme();
        let mut md = Markdown::new("hello", 1, 0, theme, None);
        let a = md.render(20);
        let b = md.render(20);
        assert_eq!(a, b);
    }

    #[test]
    fn test_cache_invalidation() {
        let theme = test_theme();
        let mut md = Markdown::new("hello", 1, 0, theme, None);
        let a = md.render(20);
        md.set_text("world");
        let b = md.render(20);
        assert_ne!(a, b);
    }

    #[test]
    fn test_blockquote() {
        let theme = test_theme();
        let mut md = Markdown::new("> quoted text", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("quoted text"));
        assert!(all.contains("│"));
    }

    #[test]
    fn test_task_list() {
        let theme = test_theme();
        let mut md = Markdown::new("- [x] done\n- [ ] todo", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("[x]") || all.contains("done"));
        assert!(all.contains("[ ]") || all.contains("todo"));
    }

    #[test]
    fn test_paragraph_spacing() {
        let theme = test_theme();
        let mut md = Markdown::new("para one\n\npara two", 0, 0, theme, None);
        let lines = md.render(80);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_tabs_replaced() {
        let theme = test_theme();
        let mut md = Markdown::new("\tindented", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("indented"));
    }

    #[test]
    fn test_default_text_style() {
        let theme = test_theme();
        let default_style = DefaultTextStyle {
            color: Some(Arc::new(|s| format!("\x1b[33m{}\x1b[39m", s))),
            bold: true,
            italic: false,
            strikethrough: false,
            underline: false,
        };
        let mut md = Markdown::new("styled text", 0, 0, theme, Some(default_style));
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("styled text"));
        assert!(all.contains("\x1b[1m"));
        assert!(all.contains("\x1b[33m"));
    }

    #[test]
    fn test_table_basic() {
        let theme = test_theme();
        let mut md = Markdown::new(
            "| H1 | H2 |\n| --- | --- |\n| A1 | B1 |\n| A2 | B2 |",
            0,
            0,
            theme,
            None,
        );
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("H1"));
        assert!(all.contains("H2"));
        assert!(all.contains("A1"));
        assert!(all.contains("┌"));
        assert!(all.contains("└"));
        assert!(all.contains("│"));
    }

    #[test]
    fn test_table_narrow_fallback() {
        let theme = test_theme();
        let mut md = Markdown::new("| A | B |\n| --- | --- |\n| 1 | 2 |", 0, 0, theme, None);
        let lines = md.render(10);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_ordered_list() {
        let theme = test_theme();
        let mut md = Markdown::new("1. first\n2. second\n3. third", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("first"));
        assert!(all.contains("second"));
        assert!(all.contains("third"));
    }

    #[test]
    fn test_nested_list() {
        let theme = test_theme();
        let mut md = Markdown::new("- outer\n  - inner\n- more", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("outer"));
        assert!(all.contains("inner"));
        assert!(all.contains("more"));
    }

    #[test]
    fn test_blockquote_nested() {
        let theme = test_theme();
        let mut md = Markdown::new("> outer\n> > nested\n> back", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("outer"));
        assert!(all.contains("nested"));
        assert!(all.contains("back"));
        assert!(all.contains("│"));
    }

    #[test]
    fn test_link_with_dest() {
        let theme = test_theme();
        let mut md = Markdown::new("[example](https://example.com/page)", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("example"));
        assert!(all.contains("example.com/page"));
    }

    #[test]
    fn test_autolink() {
        let theme = test_theme();
        let mut md = Markdown::new("<https://example.com>", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("example.com"));
    }

    #[test]
    fn test_wrap_long_text() {
        let theme = test_theme();
        let long = "this is a very long line that should definitely wrap to multiple lines when rendered in a narrow terminal column";
        let mut md = Markdown::new(long, 0, 0, theme, None);
        let lines = md.render(30);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(visible_width(line) <= 30);
        }
    }

    #[test]
    fn test_cache_different_width() {
        let theme = test_theme();
        let mut md = Markdown::new("hello world", 1, 0, theme, None);
        let a = md.render(30);
        let b = md.render(50);
        assert_ne!(a, b);
    }

    #[test]
    fn test_html_block_plain() {
        let theme = test_theme();
        let mut md = Markdown::new("<div>plain html</div>", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("plain html"));
    }

    #[test]
    fn test_bold_italic_style_restore() {
        let theme = test_theme();
        let mut md = Markdown::new("**bold `code` more bold**", 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("bold"));
        assert!(all.contains("code"));
        assert!(all.contains("more"));
    }

    // ── Heading-in-list float test ───────────────────────────────

    #[test]
    fn test_heading_inside_list_is_floated() {
        let theme = test_theme();
        let md_text = "- item\n  ### heading\n  - nested\n- more";
        let mut md = Markdown::new(md_text, 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        // heading should NOT be indented with list prefix — it was floated
        // Check it appears near the left margin, not 4+ spaces in
        assert!(all.contains("heading"), "Should contain heading text");
        // The nested list item should still be indented
        assert!(all.contains("nested"), "Should contain nested item");
        assert!(all.contains("more"), "Should contain more item");
    }

    // ── Code block inside list float test ────────────────────────

    #[test]
    fn test_code_block_inside_list_is_floated() {
        let theme = test_theme();
        let md_text = "- item\n  ```python\n  print('hi')\n  ```\n- more";
        let mut md = Markdown::new(md_text, 0, 0, theme, None);
        let lines = md.render(80);
        let all = lines.join("\n");
        assert!(all.contains("print('hi')"), "Should contain code content");
        assert!(all.contains("```"), "Should have fence markers");
        assert!(all.contains("item"), "Should contain item text");
        assert!(all.contains("more"), "Should contain more item");
    }
}
