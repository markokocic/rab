use crate::tui::Component;

/// Animated spinner component.
/// Configurable frames, interval, and message text.
pub struct Loader {
    frames: Vec<String>,
    interval_ms: u64,
    message: String,
    current_frame: usize,
    started: bool,
    last_render: std::time::Instant,
}

impl Loader {
    /// Create a new Loader with default spinner frames.
    pub fn new() -> Self {
        Self {
            frames: vec![
                "⠋".to_string(),
                "⠙".to_string(),
                "⠹".to_string(),
                "⠸".to_string(),
                "⠼".to_string(),
                "⠴".to_string(),
                "⠦".to_string(),
                "⠧".to_string(),
                "⠇".to_string(),
                "⠏".to_string(),
            ],
            interval_ms: 80,
            message: String::new(),
            current_frame: 0,
            started: false,
            last_render: std::time::Instant::now(),
        }
    }

    /// Set custom spinner frames.
    pub fn set_frames(&mut self, frames: Vec<String>) {
        self.frames = frames;
        if self.current_frame >= self.frames.len() {
            self.current_frame = 0;
        }
    }

    /// Set animation interval in milliseconds.
    pub fn set_interval_ms(&mut self, ms: u64) {
        self.interval_ms = ms;
    }

    /// Set message text shown after the spinner.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }

    /// Start the animation.
    pub fn start(&mut self) {
        self.started = true;
        self.last_render = std::time::Instant::now();
    }

    /// Stop the animation.
    pub fn stop(&mut self) {
        self.started = false;
    }

    /// Advance to the next frame if enough time has passed.
    pub fn tick(&mut self) {
        if !self.started || self.frames.is_empty() {
            return;
        }
        let elapsed = self.last_render.elapsed();
        if elapsed.as_millis() >= self.interval_ms as u128 {
            self.current_frame = (self.current_frame + 1) % self.frames.len();
            self.last_render = std::time::Instant::now();
        }
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Loader {
    fn render(&self, width: usize) -> Vec<String> {
        if self.frames.is_empty() {
            return vec![String::new()];
        }

        let frame = &self.frames[self.current_frame % self.frames.len()];

        let line = if self.message.is_empty() {
            frame.clone()
        } else {
            format!("{} {}", frame, self.message)
        };

        // Truncate to width
        let result = if line.len() > width {
            line[..width].to_string()
        } else {
            line
        };

        vec![result]
    }
}
