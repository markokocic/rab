/// Generic undo stack for storing state snapshots.
///
/// Stores clones of state snapshots. Popped snapshots are returned
/// directly since they are already detached.
#[derive(Debug, Clone, Default)]
pub struct UndoStack<S> {
    stack: Vec<S>,
}

impl<S: Clone> UndoStack<S> {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a clone of the given state onto the stack.
    pub fn push(&mut self, state: &S) {
        self.stack.push(state.clone());
    }

    /// Pop and return the most recent snapshot, or None if empty.
    pub fn pop(&mut self) -> Option<S> {
        self.stack.pop()
    }

    /// Remove all snapshots.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_pop() {
        let mut stack = UndoStack::new();
        stack.push(&"hello".to_string());
        stack.push(&"world".to_string());
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.pop(), Some("world".to_string()));
        assert_eq!(stack.pop(), Some("hello".to_string()));
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn test_clear() {
        let mut stack = UndoStack::new();
        stack.push(&1);
        stack.push(&2);
        assert_eq!(stack.len(), 2);
        stack.clear();
        assert!(stack.is_empty());
    }

    #[test]
    fn test_clone_semantics() {
        let mut stack = UndoStack::new();
        let original = "data".to_string();
        stack.push(&original);
        // Modify original - popped value should be unchanged
        drop(original);
        assert_eq!(stack.pop(), Some("data".to_string()));
    }
}
