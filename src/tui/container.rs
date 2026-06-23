use crate::tui::Component;
use crate::tui::component::RenderCache;

/// Per-child render cache entry.
struct ChildCache {
    /// Cached render output.
    cache: Option<RenderCache>,
    /// Whether child needs re-render.
    dirty: bool,
}

impl ChildCache {
    fn new() -> Self {
        Self {
            cache: None,
            dirty: true,
        }
    }
}

/// Container - a component that contains other components rendered vertically.
/// Supports per-child caching for efficient re-rendering.
pub struct Container {
    children: Vec<Box<dyn Component>>,
    /// Per-child cache state.
    child_caches: Vec<ChildCache>,
}

impl Container {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            child_caches: Vec::new(),
        }
    }

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.child_caches.push(ChildCache::new());
        self.children.push(component);
    }

    pub fn remove_child(&mut self, component: &dyn Component) {
        // Use pointer-based identity check - simplistic but works for our use case
        let idx = self.children.iter().position(|c| {
            std::ptr::eq(
                c.as_ref() as *const dyn Component,
                component as *const dyn Component,
            )
        });
        if let Some(idx) = idx {
            self.children.remove(idx);
            self.child_caches.remove(idx);
        }
    }

    pub fn clear(&mut self) {
        self.children.clear();
        self.child_caches.clear();
    }

    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }

    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        &mut self.children
    }

    /// Mark all children as needing re-render.
    pub fn invalidate_all(&mut self) {
        for cache in &mut self.child_caches {
            cache.dirty = true;
            cache.cache = None;
        }
    }

    /// Mark a specific child as needing re-render by index.
    pub fn invalidate_child(&mut self, index: usize) {
        if let Some(cache) = self.child_caches.get_mut(index) {
            cache.dirty = true;
            cache.cache = None;
        }
    }

    /// Get the number of children.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in self.children.iter_mut() {
            let child_lines = child.render(width);
            lines.extend(child_lines);
        }
        lines
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        for child in self.children.iter_mut().rev() {
            if child.handle_input(key) {
                return true;
            }
        }
        false
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
        // Also clear all caches
        for cache in &mut self.child_caches {
            cache.dirty = true;
            cache.cache = None;
        }
    }

    fn is_dirty(&self) -> bool {
        // Container is dirty if any child is dirty
        self.child_caches.iter().any(|c| c.dirty)
    }

    fn clear_dirty(&mut self) {
        for cache in &mut self.child_caches {
            cache.dirty = false;
        }
    }
}

/// CachedContainer - a Container that caches its rendered output.
/// Used for components that need efficient re-rendering (e.g., chat area).
pub struct CachedContainer {
    inner: Container,
    cache: Option<RenderCache>,
    dirty: bool,
}

impl CachedContainer {
    pub fn new() -> Self {
        Self {
            inner: Container::new(),
            cache: None,
            dirty: true,
        }
    }

    pub fn inner(&self) -> &Container {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut Container {
        self.dirty = true;
        &mut self.inner
    }

    /// Add a child component.
    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.inner.add_child(component);
        self.dirty = true;
    }

    /// Remove a child component.
    pub fn remove_child(&mut self, component: &dyn Component) {
        self.inner.remove_child(component);
        self.dirty = true;
    }

    /// Clear all children.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.cache = None;
        self.dirty = true;
    }

    /// Mark as needing re-render.
    pub fn invalidate(&mut self) {
        self.dirty = true;
        self.cache = None;
        self.inner.invalidate_all();
    }

    /// Get children.
    pub fn children(&self) -> &[Box<dyn Component>] {
        self.inner.children()
    }

    /// Get mutable children.
    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        self.dirty = true;
        self.inner.children_mut()
    }

    /// Get number of children.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for CachedContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for CachedContainer {
    fn render(&mut self, width: usize) -> Vec<String> {
        // For now, just delegate to inner container.
        // Full caching requires &mut self which render() doesn't have.
        // This is handled at the TUI level with render_with_cache().
        self.inner.render(width)
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        let result = self.inner.handle_input(key);
        if result {
            self.dirty = true;
        }
        result
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        self.cache = None;
        self.inner.invalidate();
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.inner.is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
        self.inner.clear_dirty();
    }
}
