use unicode_segmentation::UnicodeSegmentation;

/// Find the cursor position after moving one word backward from `cursor` in `text`.
///
/// Skips trailing whitespace, then stops at the next word/punctuation boundary.
/// Pure function - does not mutate any state.
pub fn find_word_backward(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }

    let cursor = cursor.min(text.len());
    let segments = segment_words(&text[..cursor]);

    if segments.is_empty() {
        return 0;
    }

    let mut pos = cursor;

    // Skip trailing whitespace
    let mut i = segments.len();
    while i > 0 {
        i -= 1;
        let seg = &segments[i];
        if is_whitespace_segment(seg) {
            pos -= seg.len();
        } else {
            break;
        }
    }

    if i == 0 && !segments.is_empty() && is_whitespace_segment(&segments[0]) {
        return pos;
    }

    // i now points to the last non-whitespace segment
    if i >= segments.len() {
        return pos;
    }

    let last = &segments[i];

    if last.is_word {
        // Skip inside one word-like segment, preserving punctuation boundaries
        if let Some(punct_pos) = last.text.rfind(is_ascii_punctuation) {
            // Stop after the last punctuation char (inclusive)
            // Find grapheme boundary after the punctuation
            let after_punct: String = last.text[punct_pos..].graphemes(true).take(1).collect();
            pos -= last.text.len() - (punct_pos + after_punct.len());
        } else {
            pos -= last.text.len();
        }
    } else {
        // Skip non-word non-whitespace run (punctuation), including current segment
        pos -= last.text.len();
        while i > 0 {
            i -= 1;
            let seg = &segments[i];
            if seg.is_word || is_whitespace_segment(seg) {
                break;
            }
            pos -= seg.text.len();
        }
    }

    pos
}

/// Find the cursor position after moving one word forward from `cursor` in `text`.
///
/// Skips leading whitespace, then stops at the next word/punctuation boundary.
/// Pure function - does not mutate any state.
pub fn find_word_forward(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }

    let segments = segment_words(&text[cursor..]);

    let mut pos = cursor;
    let mut i = 0;

    // Skip leading whitespace
    while i < segments.len() && is_whitespace_segment(&segments[i]) {
        pos += segments[i].text.len();
        i += 1;
    }

    if i >= segments.len() {
        return pos;
    }

    let first = &segments[i];

    if first.is_word {
        // Skip inside one word-like segment, stopping at punctuation
        if let Some(punct_pos) = first.text.find(is_ascii_punctuation) {
            // Include up to and including the first punctuation boundary
            let up_to_punct: String = first.text[..=punct_pos].graphemes(true).collect();
            pos += up_to_punct.len();
        } else {
            pos += first.text.len();
        }
    } else {
        // Skip non-word non-whitespace run (punctuation)
        while i < segments.len() && !segments[i].is_word && !is_whitespace_segment(&segments[i]) {
            pos += segments[i].text.len();
            i += 1;
        }
    }

    pos
}

#[derive(Debug, Clone)]
struct WordSegment {
    text: String,
    is_word: bool,
}

impl WordSegment {
    fn len(&self) -> usize {
        self.text.len()
    }
}

/// Segment text into word and non-word runs.
/// A "word" is a maximal sequence of alphanumeric or CJK characters.
/// Everything else (spaces, punctuation, symbols) forms non-word segments.
fn segment_words(text: &str) -> Vec<WordSegment> {
    let mut segments: Vec<WordSegment> = Vec::new();

    for grapheme in text.graphemes(true) {
        let is_word_char = is_word_char(grapheme);

        if let Some(last) = segments.last_mut()
            && last.is_word == is_word_char
            && !is_single_punctuation(grapheme)
        {
            last.text.push_str(grapheme);
            continue;
        }

        segments.push(WordSegment {
            text: grapheme.to_string(),
            is_word: is_word_char,
        });
    }

    // Merge adjacent segments of the same type
    let mut merged: Vec<WordSegment> = Vec::new();
    for seg in segments {
        if let Some(last) = merged.last_mut()
            && last.is_word == seg.is_word
        {
            last.text.push_str(&seg.text);
            continue;
        }
        merged.push(seg);
    }

    merged
}

fn is_whitespace_segment(seg: &WordSegment) -> bool {
    !seg.is_word && seg.text.trim().is_empty()
}

fn is_word_char(grapheme: &str) -> bool {
    grapheme.chars().any(|c| c.is_alphanumeric() || is_cjk(c))
}

fn is_cjk(c: char) -> bool {
    let block = c as u32;
    (0x4E00..=0x9FFF).contains(&block)
        || (0x3040..=0x309F).contains(&block)
        || (0x30A0..=0x30FF).contains(&block)
        || (0xAC00..=0xD7AF).contains(&block)
}

fn is_ascii_punctuation(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '<'
            | '>'
            | '\''
            | '"'
            | '+'
            | '-'
            | '*'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '%'
            | '$'
            | '#'
            | '@'
            | '~'
            | '`'
            | '^'
            | '='
    )
}

fn is_single_punctuation(grapheme: &str) -> bool {
    grapheme.len() == 1 && grapheme.chars().next().is_some_and(is_ascii_punctuation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_word_backward_basic() {
        let text = "hello world";
        assert_eq!(find_word_backward(text, 11), 6);
        assert_eq!(find_word_backward(text, 6), 0);
    }

    #[test]
    fn test_find_word_backward_dotted() {
        let text = "foo.bar";
        assert_eq!(find_word_backward(text, 7), 4);
        assert_eq!(find_word_backward(text, 4), 3);
        assert_eq!(find_word_backward(text, 3), 0);
    }

    #[test]
    fn test_find_word_backward_cursor_at_zero() {
        assert_eq!(find_word_backward("hello", 0), 0);
    }

    #[test]
    fn test_find_word_backward_punctuation_run() {
        let text = "foo...bar";
        assert_eq!(find_word_backward(text, 9), 6);
        assert_eq!(find_word_backward(text, 6), 3);
        assert_eq!(find_word_backward(text, 3), 0);
    }

    #[test]
    fn test_find_word_forward_basic() {
        let text = "hello world";
        assert_eq!(find_word_forward(text, 0), 5);
        assert_eq!(find_word_forward(text, 5), 11);
    }

    #[test]
    fn test_find_word_forward_dotted() {
        let text = "foo.bar";
        assert_eq!(find_word_forward(text, 0), 3);
        assert_eq!(find_word_forward(text, 3), 4);
        assert_eq!(find_word_forward(text, 4), 7);
    }

    #[test]
    fn test_find_word_forward_cursor_at_end() {
        assert_eq!(find_word_forward("hello", 5), 5);
    }

    #[test]
    fn test_find_word_forward_punctuation_run() {
        let text = "foo...bar";
        assert_eq!(find_word_forward(text, 0), 3);
        assert_eq!(find_word_forward(text, 3), 6);
        assert_eq!(find_word_forward(text, 6), 9);
    }
}
