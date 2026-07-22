use std::time::Instant;

use crate::Component;
use crate::components::text::Text;
use crate::util::visible_width;

/// Options for Loader indicator appearance.
pub struct LoaderIndicatorOptions {
    /// Animation frames. Use an empty vec to hide the indicator.
    pub frames: Vec<String>,
    /// Frame interval in milliseconds for animated indicators.
    pub interval_ms: u64,
}

impl Default for LoaderIndicatorOptions {
    fn default() -> Self {
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
        }
    }
}

/// Loader component with optional spinning animation.
/// Port of pi's `packages/tui/src/components/loader.ts`.
///
/// pi's Loader extends Text. In rab we wrap Text via composition.
pub struct Loader {
    text: Text,
    frames: Vec<String>,
    interval_ms: u64,
    current_frame: usize,
    started: bool,
    last_tick: Instant,
    message: String,
    spinner_color_fn: crate::Style,
    message_color_fn: crate::Style,
    render_indicator_verbatim: bool,
}

impl Loader {
    pub fn new(
        spinner_color_fn: crate::Style,
        message_color_fn: crate::Style,
        message: impl Into<String>,
    ) -> Self {
        let indicator = LoaderIndicatorOptions::default();
        Self {
            text: Text::new("", 1, 0, None),
            frames: indicator.frames,
            interval_ms: indicator.interval_ms,
            current_frame: 0,
            started: false,
            last_tick: Instant::now(),
            message: message.into(),
            spinner_color_fn,
            message_color_fn,
            render_indicator_verbatim: false,
        }
    }

    pub fn start(&mut self) {
        self.started = true;
        self.last_tick = Instant::now();
        self.update_display();
    }

    pub fn stop(&mut self) {
        self.started = false;
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.update_display();
    }

    pub fn set_indicator(&mut self, indicator: LoaderIndicatorOptions) {
        self.render_indicator_verbatim = true;
        self.frames = if indicator.frames.is_empty() {
            vec![] // hide indicator
        } else {
            indicator.frames
        };
        self.interval_ms = if indicator.interval_ms > 0 {
            indicator.interval_ms
        } else {
            80
        };
        self.current_frame = 0;
        self.update_display();
    }

    /// Advance to next frame if interval elapsed. Returns true if display changed.
    pub fn tick(&mut self) -> bool {
        if !self.started || self.frames.is_empty() || self.frames.len() <= 1 {
            return false;
        }
        let elapsed = self.last_tick.elapsed();
        if elapsed.as_millis() >= self.interval_ms as u128 {
            self.current_frame = (self.current_frame + 1) % self.frames.len();
            self.last_tick = Instant::now();
            self.update_display();
            return true;
        }
        false
    }

    fn update_display(&self) -> String {
        let frame = self
            .frames
            .get(self.current_frame)
            .map(|s| s.as_str())
            .unwrap_or("");
        let rendered_frame = if frame.is_empty() {
            String::new()
        } else if self.render_indicator_verbatim {
            frame.to_string()
        } else {
            self.spinner_color_fn.apply(frame)
        };
        let indicator = if frame.is_empty() {
            String::new()
        } else {
            format!("{} ", rendered_frame)
        };
        let display = format!(
            "{}{}",
            indicator,
            self.message_color_fn.apply(&self.message)
        );
        display
    }
}

impl Component for Loader {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Pi: renderer returns ["", ...super.render(width)] — one blank line above for spacing
        let display = self.update_display();
        let mut lines = vec![String::new()]; // blank line above
        let display_line = {
            let vw = visible_width(&display);
            if vw < width {
                format!("{}{}", display, " ".repeat(width - vw))
            } else {
                display
            }
        };
        lines.push(display_line);
        lines
    }

    fn handle_input(&mut self, _key: &crossterm::event::KeyEvent) -> bool {
        false
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loader_renders_with_spacing() {
        let mut loader = Loader::new(crate::Style::new(), crate::Style::new(), "Loading...");
        let lines = loader.render(40);
        assert!(lines.len() >= 2, "Should have blank line + content");
        assert_eq!(lines[0], "", "First line should be blank");
    }

    #[test]
    fn test_loader_message() {
        let mut loader = Loader::new(crate::Style::new(), crate::Style::new(), "Working...");
        let lines = loader.render(40);
        assert!(lines[1].contains("Working..."));
    }

    #[test]
    fn test_loader_tick() {
        let mut loader = Loader::new(crate::Style::new(), crate::Style::new(), "test");
        loader.start();
        // Immediate tick should not change (interval not elapsed)
        assert!(!loader.tick());
    }
}
