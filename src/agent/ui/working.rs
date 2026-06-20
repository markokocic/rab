use crate::agent::ui::theme::RabTheme;
use crate::tui::Component;

/// Spinner shown during agent streaming.
pub struct WorkingIndicator {
    frames: Vec<String>,
    interval_ms: u64,
    frame: usize,
    last_tick: std::time::Instant,
    theme: RabTheme,
    pub active: bool,
}

impl WorkingIndicator {
    pub fn new() -> Self {
        let theme = crate::agent::ui::theme::current_theme().clone();
        Self {
            frames: vec![
                "⠋".into(),
                "⠙".into(),
                "⠹".into(),
                "⠸".into(),
                "⠼".into(),
                "⠴".into(),
                "⠦".into(),
                "⠧".into(),
                "⠇".into(),
                "⠏".into(),
            ],
            interval_ms: 80,
            frame: 0,
            last_tick: std::time::Instant::now(),
            theme,
            active: false,
        }
    }

    pub fn start(&mut self) {
        self.active = true;
        self.last_tick = std::time::Instant::now();
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Returns true if the frame changed (caller should re-render).
    pub fn tick(&mut self) -> bool {
        if !self.active || self.frames.is_empty() {
            return false;
        }
        let elapsed = self.last_tick.elapsed();
        if elapsed.as_millis() >= self.interval_ms as u128 {
            self.frame = (self.frame + 1) % self.frames.len();
            self.last_tick = std::time::Instant::now();
            return true;
        }
        false
    }
}

impl Default for WorkingIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for WorkingIndicator {
    fn render(&self, _width: usize) -> Vec<String> {
        if !self.active || self.frames.is_empty() {
            return vec![];
        }
        let frame = &self.frames[self.frame % self.frames.len()];
        let text = self.theme.accent(frame);
        vec![text]
    }
}
