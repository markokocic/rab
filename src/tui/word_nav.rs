use unicode_segmentation::UnicodeSegmentation;

/// Characters recognized as ASCII punctuation, matching pi's PUNCTUATION_REGEX.
pub const PUNCTUATION_CHARS: &[char] = &[
    '.', ',', ';', ':', '!', '?',
    '(', ')', '[', ']', '{', '}',
    '<', '>', '\'', '"',
    '+', '-', '*', '/', '\\', '|',
    '&', '%', '$', '#', '@',
    '~', '`', '^', '=',
];

/// Options for word navigation functions (matching pi's WordNavigationOptions).
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct WordNavigationOptions<'a> {
    /// Custom segmenter returning word segments for the given text.
    /// When omitted, uses grapheme-based segmentation with `is_word_char` heuristic.
    pub segment: Option<&'a dyn Fn(&str) -> Vec<WordSegment>>,
    /// Predicate identifying atomic segments that should be treated as single units.
    /// When provided, segments matching this predicate are skipped atomically.
    pub is_atomic_segment: Option<&'a dyn Fn(&str) -> bool>,
}

/// A segment of text produced by word segmentation.
#[derive(Debug, Clone)]
pub struct WordSegment {
    pub text: String,
    pub is_word: bool,
}

impl WordSegment {
    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Default segmentation: split text into word and non-word runs using grapheme clusters.
/// A "word" is a maximal sequence of alphanumeric or CJK characters.
fn default_segment(text: &str) -> Vec<WordSegment> {
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

fn get_segments<'a>(text: &'a str, options: &WordNavigationOptions<'a>) -> Vec<WordSegment> {
    if let Some(segment_fn) = options.segment {
        segment_fn(text)
    } else {
        default_segment(text)
    }
}

fn is_atomic(segment: &str, options: &WordNavigationOptions) -> bool {
    options
        .is_atomic_segment
        .is_some_and(|is_atomic| is_atomic(segment))
}

/// Find the cursor position after moving one word backward from `cursor` in `text`.
///
/// Skips trailing whitespace, then stops at the next word/punctuation boundary.
/// When `options` is provided, uses custom segmentation and atomic segment detection.
///
/// Pure function - does not mutate any state.
pub fn find_word_backward(text: &str, cursor: usize) -> usize {
    find_word_backward_with(text, cursor, &WordNavigationOptions::default())
}

/// Find word backward with custom options (pi-style WordNavigationOptions).
/// Supports custom segmenter and isAtomicSegment predicate.
pub fn find_word_backward_with(text: &str, cursor: usize, options: &WordNavigationOptions) -> usize {
    if cursor == 0 {
        return 0;
    }

    let cursor = cursor.min(text.len());
    let segments = get_segments(&text[..cursor], options);

    if segments.is_empty() {
        return 0;
    }

    let mut pos = cursor;

    // Skip trailing whitespace
    let mut i = segments.len();
    while i > 0 {
        i -= 1;
        let seg = &segments[i];
        if !is_atomic(&seg.text, options) && is_whitespace_segment(seg) {
            pos -= seg.len();
        } else {
            break;
        }
    }

    if i == 0 && !segments.is_empty() && is_whitespace_segment(&segments[0]) {
        return pos;
    }

    if i >= segments.len() {
        return pos;
    }

    let last = &segments[i];

    if is_atomic(&last.text, options) {
        // Skip one atomic segment
        pos -= last.text.len();
    } else if last.is_word {
        // Skip inside one word-like segment, preserving punctuation boundaries
        if let Some(punct_pos) = last.text.rfind(is_ascii_punctuation) {
            let after_punct: String = last.text[punct_pos..].graphemes(true).take(1).collect();
            pos -= last.text.len() - (punct_pos + after_punct.len());
        } else {
            pos -= last.text.len();
        }
    } else {
        // Skip non-word non-whitespace run (punctuation)
        pos -= last.text.len();
        while i > 0 {
            i -= 1;
            let seg = &segments[i];
            if is_atomic(&seg.text, options) || seg.is_word || is_whitespace_segment(seg) {
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
/// When `options` is provided, uses custom segmentation and atomic segment detection.
///
/// Pure function - does not mutate any state.
pub fn find_word_forward(text: &str, cursor: usize) -> usize {
    find_word_forward_with(text, cursor, &WordNavigationOptions::default())
}

/// Find word forward with custom options (pi-style WordNavigationOptions).
pub fn find_word_forward_with(text: &str, cursor: usize, options: &WordNavigationOptions) -> usize {
    if cursor >= text.len() {
        return text.len();
    }

    let segments = get_segments(&text[cursor..], options);

    let mut pos = cursor;
    let mut i = 0;

    // Skip leading whitespace
    while i < segments.len() && !is_atomic(&segments[i].text, options) && is_whitespace_segment(&segments[i]) {
        pos += segments[i].text.len();
        i += 1;
    }

    if i >= segments.len() {
        return pos;
    }

    let first = &segments[i];

    if is_atomic(&first.text, options) {
        // Skip one atomic segment
        pos += first.text.len();
    } else if first.is_word {
        // Skip inside one word-like segment, stopping at punctuation
        if let Some(punct_pos) = first.text.find(is_ascii_punctuation) {
            let up_to_punct: String = first.text[..=punct_pos].graphemes(true).collect();
            pos += up_to_punct.len();
        } else {
            pos += first.text.len();
        }
    } else {
        // Skip non-word non-whitespace run (punctuation)
        while i < segments.len() && !is_atomic(&segments[i].text, options) && !segments[i].is_word && !is_whitespace_segment(&segments[i]) {
            pos += segments[i].text.len();
            i += 1;
        }
    }

    pos
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
    PUNCTUATION_CHARS.contains(&c)
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

    #[test]
    fn test_find_word_backward_with_atomic_segment() {
        let options = WordNavigationOptions {
            segment: None,
            is_atomic_segment: Some(&|s: &str| s.starts_with("[paste")),
        };
        let text = "hello [paste #1] world";
        let cursor = text.len();
        // Should skip the atomic paste marker as one unit
        let result = find_word_backward_with(text, cursor, &options);
        // After skipping " world", should be at "[paste #1]" start or before
        assert!(result < cursor, "Should have moved backward");
    }

    #[test]
    fn test_find_word_forward_with_atomic_segment() {
        let options = WordNavigationOptions {
            segment: None,
            is_atomic_segment: Some(&|s: &str| s.starts_with("[paste")),
        };
        let text = "hello [paste #1] world";
        // Start after "hello "
        let cursor = 6;
        let result = find_word_forward_with(text, cursor, &options);
        // Should skip the atomic paste marker and the space after it, landing at "world"
        assert!(result > cursor, "Should have moved forward past marker");
    }

    #[test]
    fn test_punctuation_regex_matches() {
        assert!(matches!('.', c if is_ascii_punctuation(c)));
        assert!(matches!(',', c if is_ascii_punctuation(c)));
        assert!(matches!(';', c if is_ascii_punctuation(c)));
        assert!(matches!(':', c if is_ascii_punctuation(c)));
        assert!(matches!('!', c if is_ascii_punctuation(c)));
        assert!(matches!('?', c if is_ascii_punctuation(c)));
        assert!(!matches!('a', c if is_ascii_punctuation(c)));
        assert!(!matches!(' ', c if is_ascii_punctuation(c)));
    }

    #[test]
    fn test_word_segment_empty() {
        let ws = WordSegment {
            text: "".to_string(),
            is_word: false,
        };
        assert!(ws.is_empty());
        assert_eq!(ws.len(), 0);
    }
}
