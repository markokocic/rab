use crate::agent::ui::theme::RabTheme;
use crate::tui::Component;
use crate::tui::components::select_list::{SelectItem, SelectList, SelectListTheme};

/// Full-screen model selector using tui::SelectList.
pub struct ModelSelector {
    select_list: SelectList,
    theme: RabTheme,
    pub selected_model: Option<String>,
}

impl ModelSelector {
    pub fn new(models: Vec<String>, current_model: &str, theme: &RabTheme) -> Self {
        let items: Vec<SelectItem> = models
            .iter()
            .map(|m| {
                let display = m.strip_prefix("opencode_go::").unwrap_or(m);
                SelectItem::new(m.clone(), display.to_string())
            })
            .collect();

        let current_index = models
            .iter()
            .position(|m| m == current_model || format!("opencode_go::{}", m) == current_model)
            .unwrap_or(0);

        let list_theme = SelectListTheme {
            selected_prefix: Box::new(|s| format!("\x1b[38;2;138;190;183m\x1b[1m> {}\x1b[0m", s)),
            selected_text: Box::new(|s| format!("\x1b[38;2;138;190;183m\x1b[1m{}\x1b[0m", s)),
            normal_text: Box::new(|s| format!("  \x1b[38;2;212;212;212m{}\x1b[0m", s)),
            description: Box::new(|s| format!("\x1b[38;2;128;128;128m{}\x1b[0m", s)),
            scroll_info: Box::new(|s| format!("\x1b[38;2;80;80;80m{}\x1b[0m", s)),
            no_match: Box::new(|s| format!("\x1b[38;2;255;255;0m{}\x1b[0m", s)),
            hint: Box::new(|s| format!("\x1b[38;2;128;128;128m{}\x1b[0m", s)),
        };

        let max_visible = models.len().clamp(5, 20);
        let mut select_list = SelectList::new(items, max_visible, list_theme, None);
        select_list = select_list.with_search();

        // Set initial selection
        for _ in 0..current_index {
            select_list.handle_input(&crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            ));
        }

        Self {
            select_list,
            theme: theme.clone(),
            selected_model: None,
        }
    }
}

impl Component for ModelSelector {
    fn render(&self, width: usize) -> Vec<String> {
        let mut lines = vec![
            self.theme.bold(&self.theme.accent("  Select Model")),
            String::new(),
            self.theme.dim("  Type to search…"),
            String::new(),
        ];

        // List
        lines.extend(self.select_list.render(width));

        lines
    }

    fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        let kb = crate::tui::keybindings::get_keybindings();

        if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_CONFIRM) {
            if let Some(item) = self.select_list.selected_item() {
                self.selected_model = Some(item.value.clone());
            }
            return true;
        }

        if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_CANCEL) {
            self.selected_model = None;
            return true;
        }

        self.select_list.handle_input(key)
    }
}
