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
    /// Ensures the indicator renders for at least one frame after `start()`,
    /// even if `stop()` is called before the next `render()`. This prevents
    /// a race where a fast agent loop dispatches both AgentStart and AgentEnd
    /// in the same event batch, causing the spinner to never appear.
    show_once: bool,
    /// True if `start()` was ever called. When idle after at least one
    /// activation, render 2 blank lines (pi's `IdleStatus`) to maintain
    /// vertical space between chat and editor.
    has_been_active: bool,
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
            show_once: false,
            has_been_active: false,
            message: "Working...".into(),
        }
    }

    pub fn start(&mut self) {
        self.active = true;
        self.show_once = true;
        self.has_been_active = true;
        self.last_tick = std::time::Instant::now();
    }

    pub fn stop(&mut self) {
        self.active = false;
        // show_once remains set if start() was called - ensures at least one render
    }

    /// Set the message shown alongside the spinner (e.g. "Working...").
    /// Mirrors pi's `Loader::setMessage()`.
    pub fn set_message(&mut self, message: String) {
        self.message = message;
    }

    /// Returns true if the indicator should be shown (active or show_once).
    /// Clears show_once after the first check so idle frames don't stay at 16ms.
    pub fn should_show(&mut self) -> bool {
        let show = (self.active || self.show_once) && !self.options.frames.is_empty();
        self.show_once = false;
        show
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
    fn render(&mut self, width: usize) -> Vec<String> {
        // During streaming: blank line + spinner with message
        if (self.active || self.show_once) && !self.options.frames.is_empty() {
            let frame = &self.options.frames[self.frame % self.options.frames.len()];
            let line = format!(
                " {} {} ",
                self.theme.accent(frame),
                self.theme.muted(&self.message)
            );
            self.show_once = false;
            // pi's Loader.render() prepends a blank line: ["", " ⠋ Working... "]
            return vec![String::new(), line];
        }

        // After first activation, show idle spacer (pi's IdleStatus: 2 blank lines)
        // to maintain vertical space between chat and editor.
        // On initial startup (never activated), render nothing.
        if self.has_been_active {
            let empty = " ".repeat(width);
            vec![empty.clone(), empty]
        } else {
            vec![]
        }
    }
}
