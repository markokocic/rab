use crate::tui::Component;
use crate::tui::component::RenderCacheKey;

/// A component that wraps a `Vec<String>` buffer that can be updated dynamically.
/// Used for sections whose content changes each frame (pending text, status, etc.).
pub struct DynamicLines {
    lines: Vec<String>,
    /// Last cache key hash, used to avoid unnecessary re-renders.
    last_hash: u64,
}

impl DynamicLines {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            last_hash: 0,
        }
    }

    /// Set the lines for this component.
    pub fn set_lines(&mut self, new_lines: Vec<String>) {
        self.lines = new_lines;
        // Invalidate cache so the Container re-renders this component.
        self.last_hash = 0;
    }

    /// Clear the lines.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Compute a hash of the current lines for cache_key.
    fn compute_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.lines.len().hash(&mut hasher);
        for line in &self.lines {
            line.hash(&mut hasher);
        }
        hasher.finish()
    }
}

impl Default for DynamicLines {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for DynamicLines {
    fn render(&mut self, _width: usize) -> Vec<String> {
        self.last_hash = self.compute_hash();
        self.lines.clone()
    }

    fn invalidate(&mut self) {
        // No-op: render always returns current buffer
    }

    fn cache_key(&self, width: usize) -> Option<RenderCacheKey> {
        Some(RenderCacheKey {
            width,
            expanded: false,
            state_hash: self.last_hash,
        })
    }
}
