use crate::agent::ui::theme::RabTheme;
use crate::tui::Component;

/// Options for configuring the working indicator's appearance.
/// Mirrors pi's `LoaderIndicatorOptions` / `WorkingIndicatorOptions`.
#[derive(Clone)]
pub struct IndicatorOptions {
    /// Animation frames. Empty array hides the indicator entirely.
    pub frames: Vec<String>,
    /// Frame interval in milliseconds for animated indicators.
    pub interval_ms: u64,
}

impl Default for IndicatorOptions {
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

/// Loader shown during agent streaming - a spinner + message.
/// Mirrors pi's `Loader` component (spinner + "Working..." message).
pub struct WorkingIndicator {
    options: IndicatorOptions,
    frame: usize,
    last_tick: std::time::Instant,
    theme: RabTheme,
    pub active: bool,
    message: String,
}

impl WorkingIndicator {
    pub fn new() -> Self {
        let theme = crate::agent::ui::theme::current_theme().clone();
        Self {
            options: IndicatorOptions::default(),
            frame: 0,
            last_tick: std::time::Instant::now(),
            theme,
            active: false,
            message: "Working...".into(),
        }
    }

    pub fn start(&mut self) {
        self.active = true;
        self.last_tick = std::time::Instant::now();
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Set the message shown alongside the spinner (e.g. "Working...").
    /// Mirrors pi's `Loader::setMessage()`.
    pub fn set_message(&mut self, message: String) {
        self.message = message;
    }

    /// Configure the indicator frames and interval.
    /// Mirrors pi's `Loader::setIndicator()`.
    pub fn set_indicator(&mut self, options: Option<IndicatorOptions>) {
        self.options = options.unwrap_or_default();
        self.frame = 0;
    }

    /// Returns true if the frame changed (caller should re-render).
    pub fn tick(&mut self) -> bool {
        if !self.active || self.options.frames.is_empty() {
            return false;
        }
        let elapsed = self.last_tick.elapsed();
        if elapsed.as_millis() >= self.options.interval_ms as u128 {
            self.frame = (self.frame + 1) % self.options.frames.len();
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
    fn render(&mut self, _width: usize) -> Vec<String> {
        if !self.active || self.options.frames.is_empty() {
            return vec![];
        }
        let frame = &self.options.frames[self.frame % self.options.frames.len()];
        // Matches pi's Loader::updateDisplay(): colored spinner + space + colored message
        // pi uses accent for spinner, muted for message.
        // pi's Text paddingX=1 adds one space on each side.
        let line = format!(
            " {} {} ",
            self.theme.accent(frame),
            self.theme.muted(&self.message)
        );
        // pi's Loader.render() prepends a blank line: ["", " ⠋ Working... "]
        vec![String::new(), line]
    }
}
