use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Regex pattern matching CJK characters for word-wrapping breaks.
/// Matches pi's `cjkBreakRegex` script extension pattern.
pub const CJK_BREAK_REGEX: &str = r"[\p{Script_Extensions=Han}\p{Script_Extensions=Hiragana}\p{Script_Extensions=Katakana}\p{Script_Extensions=Hangul}\p{Script_Extensions=Bopomofo}]";

/// Calculate the visible width of a string in terminal columns.
/// Strips ANSI escape codes and counts grapheme cluster widths.
/// Uses a thread-local LRU cache for non-ASCII strings (matching pi).
pub fn visible_width(str: &str) -> usize {
    if str.is_empty() {
        return 0;
    }

    // Fast path: pure ASCII printable
    if is_printable_ascii(str) {
        return str.len();
    }

    // Use cache for non-ASCII
    WIDTH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(&w) = cache.get(str) {
            return w;
        }
        let w = compute_visible_width_inner(str);
        if cache.len() >= WIDTH_CACHE_SIZE {
            cache.clear();
        }
        cache.insert(str.to_string(), w);
        w
    })
}

/// Check if a string consists entirely of printable ASCII characters (0x20-0x7E).
fn is_printable_ascii(str: &str) -> bool {
    str.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Calculate the terminal width of a single grapheme cluster.
fn grapheme_width(grapheme: &str) -> usize {
    if grapheme == "\t" {
        return 3;
    }

    // Check for zero-width and combining characters
    let first_char = grapheme.chars().next();
    if let Some(c) = first_char {
        // Zero-width characters
        if is_zero_width_char(c) {
            return 0;
        }

        // Emoji width (most emoji are width 2)
        if could_be_emoji(grapheme) {
            return 2;
        }

        // Regional indicator symbols (U+1F1E6..U+1F1FF) are often wide
        let _cp = c as u32;
        if (0x1f1e6..=0x1f1ff).contains(&(c as u32)) {
            return 2;
        }

        // Use unicode-width for standard characters
        if let Some(w) = c.width()
            && w > 0
        {
            return w;
        }

        // Check trailing characters for halfwidth/fullwidth forms
        let mut w = 0;
        for ch in grapheme.chars() {
            if (0xff00..=0xffef).contains(&(ch as u32)) {
                w += 2;
            } else if ch as u32 == 0x0e33 || ch as u32 == 0x0eb3 {
                w += 1;
            }
        }
        if w > 0 {
            return w;
        }

        return 2; // Default wide for unknown
    }
    0
}

/// Fast heuristic to check if a grapheme could be emoji.
fn could_be_emoji(grapheme: &str) -> bool {
    let first_cp = grapheme.chars().next().map(|c| c as u32).unwrap_or(0);
    ((0x1f000..=0x1fbff).contains(&first_cp))
        || ((0x2300..=0x23ff).contains(&first_cp))
        || ((0x2600..=0x27bf).contains(&first_cp))
        || ((0x2b50..=0x2b55).contains(&first_cp))
        || grapheme.contains('\u{FE0F}') // VS16 emoji presentation selector
        || grapheme.chars().count() > 2 // ZWJ sequences, skin tones
}

/// Check if a character is zero-width (combining marks, control chars, etc.).
fn is_zero_width_char(c: char) -> bool {
    let _cp = c as u32;
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | // Zero-width space, etc.
        '\u{2028}'..='\u{2029}' | // Line/paragraph separator
        '\u{202A}'..='\u{202E}' | // Bidi control
        '\u{2060}'..='\u{2064}' | // Word joiner, etc.
        '\u{FEFF}'                 // BOM / ZWNBS
    ) || c.is_control()
        || (unicode_width::UnicodeWidthChar::width(c) == Some(0))
}

/// Extract an ANSI escape sequence from a string at the given byte position.
/// Returns the code string and its byte length, or None if not an ANSI sequence.
fn extract_ansi_code_at(str: &str, pos: usize) -> Option<&str> {
    let bytes = str.as_bytes();
    if pos >= bytes.len() || bytes[pos] != 0x1b {
        return None;
    }

    let next = bytes.get(pos + 1).copied();

    // CSI sequence: ESC [ ... (0x40-0x7E)
    if next == Some(b'[') {
        let mut j = pos + 2;
        while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
            j += 1;
        }
        if j < bytes.len() {
            return Some(&str[pos..=j]);
        }
        return None;
    }

    // OSC sequence: ESC ] ... BEL or ESC ] ... ST (ESC \)
    if next == Some(b']') {
        let mut j = pos + 2;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                return Some(&str[pos..=j]);
            }
            if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                return Some(&str[pos..=j + 1]);
            }
            j += 1;
        }
        return None;
    }

    // APC sequence: ESC _ ... BEL or ESC _ ... ST (ESC \)
    if next == Some(b'_') {
        let mut j = pos + 2;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                return Some(&str[pos..=j]);
            }
            if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                return Some(&str[pos..=j + 1]);
            }
            j += 1;
        }
        return None;
    }

    None
}

/// Truncate text to fit within a maximum visible width, adding ellipsis if needed.
/// Optionally pad with spaces to reach exactly max_width.
///
/// Properly handles ANSI escape codes (they don't count toward width).
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }

    let text_width = visible_width(text);
    let ellipsis_width = visible_width(ellipsis);

    // Text already fits
    if text_width <= max_width {
        return if pad {
            let mut result = text.to_string();
            result.push_str(&" ".repeat(max_width - text_width));
            result
        } else {
            text.to_string()
        };
    }

    // Ellipsis is wider than available space
    if ellipsis_width >= max_width {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }

    let target_width = max_width - ellipsis_width;

    // Simple ASCII fast path
    if is_printable_ascii(text) {
        let prefix = &text[..target_width.min(text.len())];
        let mut result = String::with_capacity(max_width + 20);
        result.push_str(prefix);
        result.push_str("\x1b[0m");
        result.push_str(ellipsis);
        result.push_str("\x1b[0m");
        if pad {
            let visible = target_width.min(text.len()) + ellipsis_width;
            if visible < max_width {
                result.push_str(&" ".repeat(max_width - visible));
            }
        }
        return result;
    }

    // General: grapheme-by-grapheme truncation
    let mut kept = String::new();
    let mut kept_width: usize = 0;
    let mut pending_ansi = String::new();
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < bytes.len() {
        if bytes[i] == 0x1b
            && let Some(ansi) = extract_ansi_code_at(text, i)
        {
            pending_ansi.push_str(ansi);
            i += ansi.len();
            continue;
        }

        // Get the grapheme at this position
        let rest = &text[i..];
        let mut _grapheme_end = i;
        for g in rest.graphemes(true) {
            _grapheme_end += g.len();
            let g_width = grapheme_width(g);

            if kept_width + g_width <= target_width {
                if !pending_ansi.is_empty() {
                    kept.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                kept.push_str(g);
                kept_width += g_width;
            } else {
                // Overflow - stop
                break;
            }
        }
        break;
    }

    let mut result = String::new();
    result.push_str(&kept);
    result.push_str("\x1b[0m");
    result.push_str(ellipsis);
    result.push_str("\x1b[0m");
    if pad {
        let visible = kept_width + ellipsis_width;
        if visible < max_width {
            result.push_str(&" ".repeat(max_width - visible));
        }
    }
    result
}

/// Word-wrap text preserving ANSI escape codes.
/// Returns lines where each line is <= width visible chars.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    // Handle newlines by processing each line separately
    let mut result: Vec<String> = Vec::new();
    let mut active_codes = String::new();

    for (line_idx, input_line) in text.split('\n').enumerate() {
        let prefix = if line_idx > 0 {
            active_codes.clone()
        } else {
            String::new()
        };
        let wrapped = wrap_single_line(&format!("{}{}", prefix, input_line), width);
        for line in wrapped {
            result.push(line);
        }
        // Update active codes for next line
        update_tracker_from_text(input_line, &mut active_codes);
    }

    if result.is_empty() {
        vec![String::new()]
    } else {
        result
    }
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let visible = visible_width(line);
    if visible <= width {
        return vec![line.to_string()];
    }

    // Split line into tokens (words separated by spaces, plus CJK breaks)
    let tokens = split_into_tokens(line);
    let mut wrapped: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut current_width: usize = 0;
    let mut tracker = AnsiState::new();

    for token in &tokens {
        let token_width = visible_width(token);
        let is_space = token.trim().is_empty();

        // Token is wider than available width - break it character by character
        if token_width > width && !is_space {
            if !current_line.is_empty() {
                let line_end = tracker.line_end_reset();
                if !line_end.is_empty() {
                    current_line.push_str(&line_end);
                }
                wrapped.push(current_line);
                current_line = String::new();
                current_width = 0;
            }

            let broken = break_long_word(token, width, &mut tracker);
            let last = broken.len().saturating_sub(1);
            for (i, line) in broken.iter().enumerate() {
                if i < last {
                    wrapped.push(line.clone());
                } else {
                    current_line = line.clone();
                    current_width = visible_width(line);
                }
            }
            continue;
        }

        let total = current_width + token_width;
        if total > width && current_width > 0 {
            // Don't trim trailing spaces: they are valid content (user-typed spaces)
            // and the line is already within width (current_width <= width).
            let mut line_to_wrap = current_line.clone();
            let line_end = tracker.line_end_reset();
            if !line_end.is_empty() {
                line_to_wrap.push_str(&line_end);
            }
            wrapped.push(line_to_wrap);
            if is_space {
                // Place the whitespace at the start of the next visual line
                // so it's not lost (space typed at wrap boundary).
                let codes = tracker.active_codes();
                current_line = format!("{}{}", codes, token);
                current_width = token_width;
            } else {
                let codes = tracker.active_codes();
                current_line = format!("{}{}", codes, token);
                current_width = token_width;
            }
        } else {
            current_line.push_str(token);
            current_width += token_width;
        }

        tracker.update(token);
    }

    if !current_line.is_empty() {
        // No trim: trailing spaces are valid user-typed content and invisible
        // in the editor's padding anyway.
        wrapped.push(current_line);
    }

    if wrapped.is_empty() {
        vec![String::new()]
    } else {
        wrapped
    }
}

/// Split text into tokens for word wrapping.
/// Keeps ANSI codes attached to adjacent visible content.
fn split_into_tokens(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut current_is_space: Option<bool> = None;
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < bytes.len() {
        if bytes[i] == 0x1b
            && let Some(ansi) = extract_ansi_code_at(text, i)
        {
            pending_ansi.push_str(ansi);
            i += ansi.len();
            continue;
        }

        // Find end of non-ANSI run
        let mut end = i;
        while end < bytes.len() && bytes[end] != 0x1b {
            end += 1;
        }

        let segment_str = &text[i..end];
        let mut seg_pos = 0;
        while seg_pos < segment_str.len() {
            // Check for paste marker start - treat as single atomic token
            if segment_str[seg_pos..].starts_with("[paste #") {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                    current_is_space = None;
                }
                if let Some(end) = segment_str[seg_pos..].find(']') {
                    let marker = &segment_str[seg_pos..=seg_pos + end];
                    let token = format!("{}{}", pending_ansi, marker);
                    pending_ansi.clear();
                    tokens.push(token);
                    seg_pos += end + 1;
                    continue;
                }
            }

            // Get the next grapheme
            let grapheme = if let Some(g) = segment_str[seg_pos..].graphemes(true).next() {
                g
            } else {
                break;
            };
            let g_len = grapheme.len();
            let is_space = grapheme == " ";

            // CJK characters get their own token
            if !is_space && is_cjk_break(grapheme) {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                    current_is_space = None;
                }
                let token = format!("{}{}", pending_ansi, grapheme);
                pending_ansi.clear();
                tokens.push(token);
                seg_pos += g_len;
                continue;
            }

            let segment_is_space = is_space;
            if current_is_space.is_some_and(|s| s != segment_is_space) && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }

            if !pending_ansi.is_empty() {
                current.push_str(&pending_ansi);
                pending_ansi.clear();
            }

            current_is_space = Some(segment_is_space);
            current.push_str(grapheme);
            seg_pos += g_len;
        }

        i = end;
    }

    // Attach any remaining pending ANSI
    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = pending_ansi;
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Break a long word (wider than available width) into multiple lines.
fn break_long_word(word: &str, width: usize, tracker: &mut AnsiState) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = tracker.active_codes();
    let mut current_width: usize = 0;
    let mut i = 0;
    let bytes = word.as_bytes();

    while i < bytes.len() {
        if bytes[i] == 0x1b
            && let Some(ansi) = extract_ansi_code_at(word, i)
        {
            current_line.push_str(ansi);
            tracker.update(ansi);
            i += ansi.len();
            continue;
        }

        let rest = &word[i..];
        let mut grapheme_end = i;
        for g in rest.graphemes(true) {
            grapheme_end += g.len();
            let g_width = grapheme_width(g);

            if current_width + g_width > width && current_width > 0 {
                let line_end = tracker.line_end_reset();
                if !line_end.is_empty() {
                    current_line.push_str(&line_end);
                }
                lines.push(std::mem::take(&mut current_line));
                current_line = tracker.active_codes();
                current_width = 0;
            }

            current_line.push_str(g);
            current_width += g_width;
        }
        i = grapheme_end;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

/// Extract a range of visible columns from a line. Handles ANSI codes and wide chars.
pub fn slice_by_column(line: &str, start_col: usize, length: usize) -> String {
    if length == 0 {
        return String::new();
    }

    let end_col = start_col + length;
    let mut result = String::new();
    let mut current_col: usize = 0;
    let mut pending_ansi = String::new();
    let mut i = 0;
    let bytes = line.as_bytes();

    while i < bytes.len() {
        if bytes[i] == 0x1b
            && let Some(ansi) = extract_ansi_code_at(line, i)
        {
            if current_col >= start_col && current_col < end_col {
                result.push_str(ansi);
            } else if current_col < start_col {
                pending_ansi.push_str(ansi);
            }
            i += ansi.len();
            continue;
        }

        // Find end of non-ANSI run
        let mut text_end = i;
        while text_end < bytes.len() && bytes[text_end] != 0x1b {
            text_end += 1;
        }

        let segment_str = &line[i..text_end];
        for grapheme in segment_str.graphemes(true) {
            let w = grapheme_width(grapheme);
            let in_range = current_col >= start_col && current_col < end_col;

            if in_range && current_col + w <= end_col {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push_str(grapheme);
            }

            current_col += w;
            if current_col >= end_col {
                return result;
            }
        }
        i = text_end;
        if current_col >= end_col {
            return result;
        }
    }

    result
}

/// Convert a visual column position to a byte offset in the given text.
/// Handles ANSI escape codes and wide characters correctly.
pub fn visual_col_to_byte_offset(text: &str, visual_col: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut vis_so_far: usize = 0;
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < bytes.len() {
        if bytes[i] == 0x1b
            && let Some(ansi) = extract_ansi_code_at(text, i)
        {
            i += ansi.len();
            continue;
        }

        let rest = &text[i..];
        if let Some(g) = rest.graphemes(true).next() {
            let gw = grapheme_width(g);
            if vis_so_far + gw > visual_col {
                return i;
            }
            vis_so_far += gw;
            i += g.len();
            continue;
        }
        break;
    }

    text.len()
}

/// Simple ANSI state tracker for wrap_text_with_ansi.
struct AnsiState {
    bold: bool,
    underline: bool,
    fg_color: Option<String>,
    bg_color: Option<String>,
}

impl AnsiState {
    fn new() -> Self {
        Self {
            bold: false,
            underline: false,
            fg_color: None,
            bg_color: None,
        }
    }

    fn update(&mut self, text: &str) {
        let mut i = 0;
        let bytes = text.as_bytes();
        while i < bytes.len() {
            if bytes[i] == 0x1b
                && let Some(ansi) = extract_ansi_code_at(text, i)
            {
                self.process_ansi(ansi);
                i += ansi.len();
                continue;
            }
            i += 1;
        }
    }

    fn process_ansi(&mut self, code: &str) {
        let code_bytes = code.as_bytes();
        // Check for SGR codes: ESC [ ... m
        if code_bytes.len() < 4 || code_bytes[code_bytes.len() - 1] != b'm' {
            return;
        }

        let inner = &code[2..code.len() - 1]; // Strip ESC[ and m
        if inner.is_empty() || inner == "0" {
            self.bold = false;
            self.underline = false;
            self.fg_color = None;
            self.bg_color = None;
            return;
        }

        let params: Vec<&str> = inner.split(';').collect();
        let mut i = 0;
        while i < params.len() {
            let Ok(parsed) = params[i].parse::<u8>() else {
                i += 1;
                continue;
            };
            match parsed {
                0 => {
                    self.bold = false;
                    self.underline = false;
                    self.fg_color = None;
                    self.bg_color = None;
                }
                1 => self.bold = true,
                4 => self.underline = true,
                22 => self.bold = false,
                24 => self.underline = false,
                30..=37 | 90..=97 => {
                    self.fg_color = Some(parsed.to_string());
                }
                40..=47 | 100..=107 => {
                    self.bg_color = Some(parsed.to_string());
                }
                38 => {
                    // Extended foreground color: 38;5;N or 38;2;R;G;B
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            "5" if i + 2 < params.len() => {
                                self.fg_color = Some(params[i..=i + 2].join(";"));
                                i += 2;
                            }
                            "2" if i + 4 < params.len() => {
                                self.fg_color = Some(params[i..=i + 4].join(";"));
                                i += 4;
                            }
                            _ => {}
                        }
                    }
                }
                48 => {
                    // Extended background color: 48;5;N or 48;2;R;G;B
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            "5" if i + 2 < params.len() => {
                                self.bg_color = Some(params[i..=i + 2].join(";"));
                                i += 2;
                            }
                            "2" if i + 4 < params.len() => {
                                self.bg_color = Some(params[i..=i + 4].join(";"));
                                i += 4;
                            }
                            _ => {}
                        }
                    }
                }
                39 => self.fg_color = None,
                49 => self.bg_color = None,
                _ => {}
            }
            i += 1;
        }
    }

    fn active_codes(&self) -> String {
        let mut codes: Vec<String> = Vec::new();
        if self.bold {
            codes.push("1".to_string());
        }
        if self.underline {
            codes.push("4".to_string());
        }
        if let Some(ref fg) = self.fg_color {
            codes.push(fg.clone());
        }
        if let Some(ref bg) = self.bg_color {
            codes.push(bg.clone());
        }
        if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        }
    }

    /// Get reset for underline only (preserves background at line end).
    fn line_end_reset(&self) -> String {
        if self.underline {
            "\x1b[24m".to_string()
        } else {
            String::new()
        }
    }
}

/// Normalize a terminal output line by appending a reset + hyperlink-close sequence.
/// This ensures any open ANSI/OSC styles are cleanly terminated.
/// Matches pi's normalizeTerminalOutput.
pub fn normalize_terminal_output(line: &str) -> String {
    format!("{}\x1b[0m\x1b]8;;\x07", line)
}

/// Check if a grapheme cluster is whitespace.
/// Single-char check matching pi's isWhitespaceChar.
pub fn is_whitespace_char(grapheme: &str) -> bool {
    grapheme == " " || grapheme == "\t"
}

/// Extract segments from a line for overlay compositing.
/// Returns (before_text, before_width, after_text, after_width).
/// The "before" segment is columns [0, before_end).
/// The "after" segment is columns [after_start, total_width).
/// Matches pi's extractSegments.
pub fn extract_segments(
    line: &str,
    before_end: usize,
    after_start: usize,
    after_len: usize,
    strict: bool,
) -> (String, usize, String, usize) {
    let before = slice_by_column(line, 0, before_end);
    let before_width = visible_width(&before);
    let after = slice_by_column(line, after_start, after_len);
    let after_width = visible_width(&after);

    if strict {
        // If before_text is wider than expected, use empty before
        if before_width > before_end {
            return (String::new(), 0, after, after_width);
        }
    }

    (before, before_width, after, after_width)
}

/// Slice text by visible columns, returning both the extracted text and its width.
/// Like `slice_by_column` but also returns the actual visible width of the result.
/// Matches pi's `sliceWithWidth`.
pub fn slice_with_width(line: &str, start_col: usize, length: usize) -> (String, usize) {
    let text = slice_by_column(line, start_col, length);
    let width = visible_width(&text);
    (text, width)
}

// Width cache for non-ASCII strings (matching pi's WIDTH_CACHE_SIZE = 512)
use std::cell::RefCell;
use std::collections::HashMap;

const WIDTH_CACHE_SIZE: usize = 512;

thread_local! {
    static WIDTH_CACHE: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

/// Compute visible width without cache (used by `visible_width` for cache misses).
fn compute_visible_width_inner(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    // Normalize: tabs to 3 spaces, strip ANSI escape codes
    let mut clean = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'\t' {
            clean.push_str("   ");
            i += 1;
            continue;
        }
        if bytes[i] == 0x1b
            && let Some(ansi) = extract_ansi_code_at(s, i)
        {
            i += ansi.len();
            continue;
        }
        if let Some(ch) = s[i..].chars().next() {
            clean.push(ch);
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }

    let mut width = 0;
    for grapheme in clean.graphemes(true) {
        width += grapheme_width(grapheme);
    }
    width
}

/// Check if a grapheme cluster is CJK (needs its own token for wrapping).
pub fn is_cjk_break(grapheme: &str) -> bool {
    if let Some(c) = grapheme.chars().next() {
        let block = c as u32;
        // CJK Unified, Hiragana, Katakana, Hangul, Bopomofo
        (0x4E00..=0x9FFF).contains(&block)
            || (0x3040..=0x309F).contains(&block)
            || (0x30A0..=0x30FF).contains(&block)
            || (0xAC00..=0xD7AF).contains(&block)
            || (0x3100..=0x312F).contains(&block)
    } else {
        false
    }
}

fn update_tracker_from_text(text: &str, active_codes: &mut String) {
    // Simple: just re-evaluate ANSI state from scratch for the text
    let mut tracker = AnsiState::new();
    tracker.update(text);
    *active_codes = tracker.active_codes();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visible_width_ascii() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn test_visible_width_with_ansi() {
        assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
        assert_eq!(visible_width("\t\x1b[31m界\x1b[0m"), 5); // tab=3 + CJK=2
    }

    #[test]
    fn test_visible_width_cjk() {
        assert_eq!(visible_width("世界"), 4);
        assert_eq!(visible_width("hello世界"), 9);
    }

    #[test]
    fn test_visible_width_emoji() {
        assert_eq!(visible_width("🙂"), 2);
        assert_eq!(visible_width("👋"), 2);
    }

    #[test]
    fn test_truncate_to_width_no_truncation() {
        let result = truncate_to_width("hello", 10, "...", false);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_to_width_with_ellipsis() {
        let result = truncate_to_width("hello world", 8, "...", false);
        assert!(visible_width(&result) <= 8);
        assert!(result.contains("..."));
    }

    #[test]
    fn test_truncate_to_width_with_pad() {
        let result = truncate_to_width("hi", 8, "...", true);
        assert_eq!(visible_width(&result), 8);
    }

    #[test]
    fn test_truncate_to_width_empty() {
        assert_eq!(truncate_to_width("", 5, "...", false), "");
        assert_eq!(truncate_to_width("", 5, "...", true), " ".repeat(5));
    }

    #[test]
    fn test_truncate_to_width_max_zero() {
        assert_eq!(truncate_to_width("hello", 0, "...", false), "");
    }

    #[test]
    fn test_wrap_basic() {
        let text = "hello world this is a test";
        let wrapped = wrap_text_with_ansi(text, 10);
        assert!(wrapped.len() > 1);
        for line in &wrapped {
            assert!(visible_width(line) <= 10);
        }
    }

    #[test]
    fn test_wrap_no_wrap_needed() {
        let text = "hello";
        let wrapped = wrap_text_with_ansi(text, 10);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0], "hello");
    }

    #[test]
    fn test_wrap_preserves_ansi() {
        let text = "\x1b[31mhello world this is red\x1b[0m";
        let wrapped = wrap_text_with_ansi(text, 10);
        // Each continuation line should start with red code
        for line in wrapped.iter().skip(1) {
            assert!(line.starts_with("\x1b[31m"));
        }
    }

    #[test]
    fn test_slice_by_column_basic() {
        let line = "hello world";
        assert_eq!(slice_by_column(line, 0, 5), "hello");
        assert_eq!(slice_by_column(line, 6, 5), "world");
        assert_eq!(slice_by_column(line, 3, 4), "lo w");
    }

    #[test]
    fn test_slice_by_column_empty() {
        assert_eq!(slice_by_column("test", 0, 0), "");
    }

    #[test]
    fn test_normalize_terminal_output() {
        let result = normalize_terminal_output("hello");
        assert_eq!(result, "hello\x1b[0m\x1b]8;;\x07");
    }

    #[test]
    fn test_is_whitespace_char() {
        assert!(is_whitespace_char(" "));
        assert!(is_whitespace_char("\t"));
        assert!(!is_whitespace_char("a"));
        assert!(!is_whitespace_char(""));
    }

    #[test]
    fn test_extract_segments_basic() {
        let line = "hello beautiful world";
        // before_end=5 → cols [0,5) = "hello"
        // after_start=15, len=5 → cols [15,20) = " worl" (space + first 4 chars of "world")
        let (before, bw, after, aw) = extract_segments(line, 5, 15, 5, true);
        assert_eq!(before, "hello");
        assert_eq!(bw, 5);
        assert_eq!(after, " worl");
        assert_eq!(aw, 5);
    }

    #[test]
    fn test_extract_segments_overflow() {
        let line = "short";
        // before_end=10 exceeds line width 5, strict mode doesn't trigger
        // (before_width=5 <= before_end=10) so returns full line as before
        let (before, bw, after, _aw) = extract_segments(line, 10, 15, 5, true);
        assert_eq!(before, "short");
        assert_eq!(bw, 5);
        assert!(after.is_empty());
    }
}

#[test]
fn test_wrap_multiline_preserves_line_count() {
    // Joint: multiline text where lines both fit and need wrapping
    let text = "hello world this is a test\nshort\nanother long line here yes";
    let wrapped = wrap_text_with_ansi(text, 10);
    // "hello world this is a test" → how many wrapped lines?
    // "short" → 1
    // "another long line here yes" → how many wrapped lines?
    let total_wrapped = wrapped.len();
    let expected_min = 3; // at least 3 visual lines
    assert!(
        total_wrapped >= expected_min,
        "Expected at least {} lines, got {}",
        expected_min,
        total_wrapped
    );
    // Verify all lines fit within width
    for (i, line) in wrapped.iter().enumerate() {
        let w = visible_width(line);
        assert!(
            w <= 10,
            "Line {}: '{}' has visible_width {} > 10",
            i,
            line,
            w
        );
    }
}

#[test]
fn test_wrap_text_with_ansi_no_duplicate_lines() {
    // Check that wrapping a multiline string produces exactly
    // the sum of wrapped lines for each logical line, with no duplicates.
    let text = "abc def ghi\njk lm no pq rs";
    let result = wrap_text_with_ansi(text, 5);
    // "abc def ghi" → ["abc", "def", "ghi"] (3 lines)
    // "jk lm no pq rs" → ["jk lm", "no pq", "rs"] (3 lines)
    // Total expected: 6
    assert_eq!(
        result.len(),
        6,
        "Expected 6 wrapped lines (3+3), got {}: {:?}",
        result.len(),
        result
    );

    // Verify no duplicate lines
    let mut seen = std::collections::HashSet::new();
    for line in &result {
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() && !seen.insert(trimmed.clone()) {
            panic!("Duplicate line found: '{}'", trimmed);
        }
    }
}

#[test]
fn test_wrap_user_text_does_not_introduce_duplicates() {
    let t1 = "ghhh jjj jkkk  jrjrnr jrnr rkr rrkr rmrrkrr k   ghhh jjj jkkk  jrjrnr jrnr rkr rrkr rmrrkrr k";

    // The original input has the same 45-char substring twice separated by triple space.
    // This is NOT a wrapping bug - the input legitimately has the duplicate.
    // This test verifies that wrap_text_with_ansi does not INTRODUCE extra duplicates
    // beyond what the input already contains.

    // Count occurrences of each substring in the original
    fn count_occurrences(text: &str, pattern: &str) -> usize {
        text.matches(pattern).count()
    }

    let pattern = "ghhh jjj jkkk  jrjrnr jrnr rkr rrkr rmrrkrr k";
    let original_count = count_occurrences(t1, pattern);
    assert_eq!(
        original_count, 2,
        "Input should have 2 occurrences of pattern"
    );

    for width in [40, 50, 60, 80, 100] {
        let wrapped = wrap_text_with_ansi(t1, width);
        // Count how many times the pattern appears in the wrapped output
        let wrapped_count: usize = wrapped
            .iter()
            .map(|line| count_occurrences(line, pattern))
            .sum();
        // The wrapped output should have at most the same number of occurrences as the input
        assert!(
            wrapped_count <= original_count,
            "Width {}: wrapped has {} occurrences, input has {}",
            width,
            wrapped_count,
            original_count
        );
    }
}
