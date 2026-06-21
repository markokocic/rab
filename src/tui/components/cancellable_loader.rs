use crate::tui::Component;
use crate::tui::components::loader::Loader;
use crate::tui::keybindings::{ACTION_SELECT_CANCEL, get_keybindings};

/// Loader with escape-to-cancel functionality.
/// Port of pi's `packages/tui/src/components/cancellable-loader.ts`.
pub struct CancellableLoader {
    loader: Loader,
    cancelled: bool,
    pub on_abort: Option<Box<dyn FnMut()>>,
}

impl CancellableLoader {
    pub fn new(
        spinner_color_fn: Box<dyn Fn(&str) -> String>,
        message_color_fn: Box<dyn Fn(&str) -> String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            loader: Loader::new(spinner_color_fn, message_color_fn, message),
            cancelled: false,
            on_abort: None,
        }
    }

    pub fn start(&mut self) {
        self.loader.start();
    }

    pub fn stop(&mut self) {
        self.loader.stop();
    }

    /// Stop the animation and clean up. Matches pi's `dispose()`.
    pub fn dispose(&mut self) {
        self.loader.stop();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn tick(&mut self) -> bool {
        self.loader.tick()
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.loader.set_message(message);
    }
}

impl Component for CancellableLoader {
    fn render(&self, width: usize) -> Vec<String> {
        self.loader.render(width)
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        let kb = get_keybindings();
        if kb.matches(key, ACTION_SELECT_CANCEL) {
            self.cancelled = true;
            if let Some(ref mut cb) = self.on_abort {
                cb();
            }
            return true;
        }
        false
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_cancel_on_escape() {
        let mut cl = CancellableLoader::new(
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Working...",
        );
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(!cl.is_cancelled());
        cl.handle_input(&escape);
        assert!(cl.is_cancelled());
    }

    #[test]
    fn test_dispose_stops() {
        let mut cl = CancellableLoader::new(
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Working...",
        );
        cl.start();
        cl.dispose();
        // After dispose, tick should not advance
        assert!(!cl.tick());
    }
}
