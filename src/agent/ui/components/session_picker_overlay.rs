use crate::agent::session::SessionManager;
use crate::agent::ui::components::session_picker::{SessionPicker, SessionPickerResult};
use crate::agent::AgentSession;
use crate::agent::DefaultSessionRepo;
use crate::tui::Component;
use crate::tui::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;

/// Overlay for interactive session selection.
///
/// Renders a session list with keyboard navigation, filtering, and selection.
/// Emits a SessionPickerResult via the `result` field after Enter or Esc.
pub struct SessionPickerOverlay {
    picker: SessionPicker,
    result: Option<SessionPickerResult>,
    theme: Box<dyn Theme>,
}

impl SessionPickerOverlay {
    pub fn new(theme: Box<dyn Theme>) -> Self {
        let mut picker = SessionPicker::new();
        let repo = DefaultSessionRepo::new();
        picker.load_sessions(&repo);

        Self {
            picker,
            result: None,
            theme,
        }
    }

    /// Take the result if the user made a selection.
    pub fn take_result(&mut self) -> Option<SessionPickerResult> {
        self.result.take()
    }

    /// Apply the selected result: switch the app's session.
    pub fn apply_result(
        result: SessionPickerResult,
        app_session: &mut Option<AgentSession>,
        cwd: &Path,
    ) -> Option<String> {
        match result {
            SessionPickerResult::Select(path) => {
                let new_sm = SessionManager::open(&path, None, Some(cwd));
                let new_session = AgentSession::new(new_sm);
                let session_id = new_session.session().session_id().to_string();
                *app_session = Some(new_session);
                Some(session_id)
            }
            SessionPickerResult::Cancel | SessionPickerResult::Info(_) | SessionPickerResult::Delete(_) => None,
        }
    }
}

impl Component for SessionPickerOverlay {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.picker.render(width, self.theme.as_ref()).0
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        // If we already have a result, consume any key and reset
        if self.result.is_some() {
            self.result = None;
            return true;
        }

        match key.code {
            KeyCode::Esc => {
                self.result = Some(SessionPickerResult::Cancel);
                true
            }
            KeyCode::Enter => {
                if let Some(path) = self.picker.selected_path() {
                    self.result = Some(SessionPickerResult::Select(path));
                }
                true
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.picker.select_prev();
                true
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.picker.select_next();
                true
            }
            KeyCode::Char('/') => {
                // Start filtering — the filter is managed internally
                // The next printable chars will be appended to the filter
                self.picker.set_filter("");
                true
            }
            KeyCode::Char(c) => {
                // Append printable char to filter
                let mut filter = self.picker.filter().to_string();
                filter.push(c);
                self.picker.set_filter(&filter);
                true
            }
            KeyCode::Backspace => {
                let mut filter = self.picker.filter().to_string();
                filter.pop();
                self.picker.set_filter(&filter);
                true
            }
            _ => false,
        }
    }
}
