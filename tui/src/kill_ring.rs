/// Ring buffer for Emacs-style kill/yank operations.
///
/// Tracks killed (deleted) text entries. Consecutive kills can accumulate
/// into a single entry. Supports yank (paste most recent) and yank-pop
/// (cycle through older entries).
#[derive(Debug, Clone, Default)]
pub struct KillRing {
    ring: Vec<String>,
}

impl KillRing {
    pub fn new() -> Self {
        Self { ring: Vec::new() }
    }

    /// Add text to the kill ring.
    ///
    /// If `accumulate` is true, merges with the most recent entry.
    /// If `prepend` is true, the new text goes before the existing entry.
    pub fn push(&mut self, text: &str, prepend: bool, accumulate: bool) {
        if text.is_empty() {
            return;
        }

        if accumulate && let Some(last) = self.ring.last_mut() {
            if prepend {
                let new_entry = format!("{}{}", text, last);
                *last = new_entry;
            } else {
                last.push_str(text);
            }
            return;
        }

        self.ring.push(text.to_string());
    }

    /// Get the most recent entry without modifying the ring.
    pub fn peek(&self) -> Option<&str> {
        self.ring.last().map(|s| s.as_str())
    }

    /// Move the last entry to the front (for yank-pop cycling).
    pub fn rotate(&mut self) {
        if self.ring.len() > 1
            && let Some(last) = self.ring.pop()
        {
            self.ring.insert(0, last);
        }
    }

    /// Number of entries in the ring.
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_peek() {
        let mut kr = KillRing::new();
        kr.push("hello", false, false);
        assert_eq!(kr.peek(), Some("hello"));
        assert_eq!(kr.len(), 1);
    }

    #[test]
    fn test_push_empty_ignored() {
        let mut kr = KillRing::new();
        kr.push("", false, false);
        assert!(kr.is_empty());
    }

    #[test]
    fn test_accumulate_append() {
        let mut kr = KillRing::new();
        kr.push("hello", false, false);
        kr.push(" world", false, true);
        assert_eq!(kr.peek(), Some("hello world"));
        assert_eq!(kr.len(), 1);
    }

    #[test]
    fn test_accumulate_prepend() {
        let mut kr = KillRing::new();
        kr.push("world", false, false);
        kr.push("hello ", true, true);
        assert_eq!(kr.peek(), Some("hello world"));
    }

    #[test]
    fn test_accumulate_without_entries() {
        let mut kr = KillRing::new();
        kr.push("hello", false, true);
        assert_eq!(kr.peek(), Some("hello"));
    }

    #[test]
    fn test_rotate_single_entry() {
        let mut kr = KillRing::new();
        kr.push("only", false, false);
        kr.rotate();
        assert_eq!(kr.peek(), Some("only"));
    }

    #[test]
    fn test_rotate_multiple() {
        let mut kr = KillRing::new();
        kr.push("first", false, false);
        kr.push("second", false, false);
        kr.push("third", false, false);
        // ring = [first, second, third]
        assert_eq!(kr.peek(), Some("third"));
        kr.rotate(); // ring = [third, first, second]
        assert_eq!(kr.peek(), Some("second"));
        kr.rotate(); // ring = [second, third, first]
        assert_eq!(kr.peek(), Some("first"));
        kr.rotate(); // ring = [first, second, third]
        assert_eq!(kr.peek(), Some("third"));
    }
}
