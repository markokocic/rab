use crate::tui::Component;
use crate::tui::components::Loader;

/// Loader with escape-to-cancel functionality.
/// Shows a cancel hint and supports AbortSignal-like cancellation.
pub struct CancellableLoader {
    loader: Loader,
    cancelled: bool,
    cancel_hint: String,
}

impl CancellableLoader {
    pub fn new() -> Self {
        Self {
            loader: Loader::new(),
            cancelled: false,
            cancel_hint: "Esc to cancel".to_string(),
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.loader.set_message(message);
        self
    }

    pub fn set_cancel_hint(&mut self, hint: impl Into<String>) {
        self.cancel_hint = hint.into();
    }

    pub fn start(&mut self) {
        self.loader.start();
    }

    pub fn stop(&mut self) {
        self.loader.stop();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn tick(&mut self) {
        self.loader.tick();
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.loader.set_message(message);
    }

    pub fn set_frames(&mut self, frames: Vec<String>) {
        self.loader.set_frames(frames);
    }
}

impl Default for CancellableLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for CancellableLoader {
    fn render(&self, width: usize) -> Vec<String> {
        let mut lines = self.loader.render(width);
        if !self.cancelled {
            lines.push(self.cancel_hint.clone());
        }
        lines
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        use crate::tui::keys::{Key, matches_key};
        if matches_key(key, &Key::Escape) {
            self.cancelled = true;
            return true;
        }
        false
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }
}
