use std::path::Path;

use crate::tui::components::select_list::SelectItem;

/// A suggestion item for autocomplete.
#[derive(Debug, Clone)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl From<AutocompleteItem> for SelectItem {
    fn from(item: AutocompleteItem) -> Self {
        let mut si = SelectItem::new(item.value, item.label);
        if let Some(desc) = item.description {
            si = si.with_description(desc);
        }
        si
    }
}

/// Suggestions returned by an autocomplete provider.
#[derive(Debug, Clone)]
pub struct AutocompleteSuggestions {
    pub items: Vec<AutocompleteItem>,
    /// The prefix that was matched (e.g., "/" or "src/").
    pub prefix: String,
}

/// A slash command definition.
pub struct SlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

/// Provider that generates autocomplete suggestions.
pub trait AutocompleteProvider {
    /// Characters that should naturally trigger this provider at token boundaries.
    fn trigger_characters(&self) -> &[char];

    /// Get suggestions for the current text/cursor position.
    /// Returns None if no suggestions available.
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions>;

    /// Apply the selected completion item.
    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> (Vec<String>, usize, usize);

    /// Whether to trigger file completion on Tab.
    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool;
}

// =============================================================================
// CombinedAutocompleteProvider — handles slash commands + file paths
// =============================================================================

/// Combined provider that handles slash commands and file path completion.
pub struct CombinedAutocompleteProvider {
    slash_commands: Vec<SlashCommand>,
    base_path: String,
}

impl CombinedAutocompleteProvider {
    pub fn new(slash_commands: Vec<SlashCommand>, base_path: String) -> Self {
        Self {
            slash_commands,
            base_path,
        }
    }

    fn get_slash_suggestions(&self, prefix: &str) -> Option<AutocompleteSuggestions> {
        let lower_prefix = prefix.to_lowercase();
        let matching: Vec<AutocompleteItem> = self
            .slash_commands
            .iter()
            .filter(|cmd| cmd.name.to_lowercase().starts_with(&lower_prefix))
            .map(|cmd| {
                let desc = match (&cmd.description, &cmd.argument_hint) {
                    (Some(d), Some(h)) => Some(format!("{} — {}", h, d)),
                    (Some(d), None) => Some(d.clone()),
                    (None, Some(h)) => Some(h.clone()),
                    (None, None) => None,
                };
                AutocompleteItem {
                    value: cmd.name.clone(),
                    label: format!("/{}", cmd.name),
                    description: desc,
                }
            })
            .collect();

        if matching.is_empty() {
            return None;
        }

        Some(AutocompleteSuggestions {
            items: matching,
            prefix: format!("/{}", prefix),
        })
    }

    fn get_file_suggestions(&self, prefix: &str) -> Option<AutocompleteSuggestions> {
        // Determine search directory and file prefix
        let expanded = if prefix.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            format!("{}/{}", home, &prefix[2..])
        } else if prefix == "~" {
            std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
        } else if prefix.starts_with('/') {
            prefix.to_string()
        } else {
            format!("{}/{}", self.base_path, prefix)
        };

        let expanded_clone = expanded.clone();
        let (dir, file_prefix) = if expanded.ends_with('/') {
            (expanded_clone, String::new())
        } else {
            let p = Path::new(&expanded);
            let parent = p.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or("/".into());
            let file = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            (if parent.is_empty() { "/".into() } else { parent }, file)
        };

        let dir_path = Path::new(&dir);
        if !dir_path.exists() || !dir_path.is_dir() {
            return None;
        }

        let lower_prefix = file_prefix.to_lowercase();
        let mut items: Vec<AutocompleteItem> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" || name.starts_with('.') {
                    continue;
                }
                if !name.to_lowercase().starts_with(&lower_prefix) {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let suffix = if is_dir { "/" } else { "" };

                // Build display path relative to base or absolute
                let display = if prefix.starts_with('/') || prefix.starts_with("~/") {
                    let base_dir = if prefix.starts_with("~/") {
                        let _home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                        if prefix.ends_with('/') {
                            // Already expanded
                            expanded.clone()
                        } else {
                            let p = Path::new(&expanded);
                            p.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or("/".into())
                        }
                    } else {
                        dir.clone()
                    };
                    let completion = if is_dir {
                        format!("{}{}/", base_dir, name)
                    } else {
                        format!("{}{}", base_dir, name)
                    };
                    completion
                } else {
                    // Relative to cwd
                    // We need to construct the relative path from base_path
                    let rel_dir = if prefix.is_empty() || !prefix.contains('/') {
                        String::new()
                    } else {
                        let p = Path::new(prefix);
                        let parent = p.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                        if parent.is_empty() { String::new() } else { format!("{}/", parent) }
                    };
                    format!("{}{}{}", rel_dir, name, suffix)
                };

                items.push(AutocompleteItem {
                    value: display.clone(),
                    label: format!("{}{}", name, suffix),
                    description: None,
                });
            }
        }

        // Sort: directories first, then alphabetical
        items.sort_by(|a, b| {
            let a_is_dir = a.value.ends_with('/');
            let b_is_dir = b.value.ends_with('/');
            if a_is_dir && !b_is_dir {
                std::cmp::Ordering::Less
            } else if !a_is_dir && b_is_dir {
                std::cmp::Ordering::Greater
            } else {
                a.label.to_lowercase().cmp(&b.label.to_lowercase())
            }
        });

        if items.is_empty() {
            return None;
        }

        Some(AutocompleteSuggestions {
            items,
            prefix: prefix.to_string(),
        })
    }
}

impl AutocompleteProvider for CombinedAutocompleteProvider {
    fn trigger_characters(&self) -> &[char] {
        &['/', '@', '#']
    }

    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions> {
        let current_line = lines.get(cursor_line)?;
        let text_before = &current_line[..cursor_col.min(current_line.len())];

        // Slash command completion
        if text_before.starts_with('/') && !text_before.contains(' ') {
            let cmd = &text_before[1..]; // Strip leading /
            return self.get_slash_suggestions(cmd);
        }

        // Slash command argument completion (after space in /command ...)
        if let Some(space_pos) = text_before.find(' ') {
            let cmd_name = &text_before[1..space_pos];
            let arg_text = &text_before[space_pos + 1..];

            // Check if this is a known command with argument completion
            for cmd in &self.slash_commands {
                if cmd.name == cmd_name {
                    // File path completion for command arguments
                    if force || arg_text.contains('/') || arg_text.contains('.') || arg_text.is_empty() {
                        return self.get_file_suggestions(arg_text);
                    }
                    return None;
                }
            }
        }

        // @ and # file/attachment completion
        if let Some(pos) = text_before.rfind(['@', '#']) {
            let is_token_start = pos == 0
                || text_before[..pos].ends_with(' ')
                || text_before[..pos].ends_with('\t');
            if is_token_start {
                let path = &text_before[pos + 1..];
                return self.get_file_suggestions(path);
            }
        }

        // Forced completion (Tab) — try file paths
        if force && self.should_trigger_file_completion(lines, cursor_line, cursor_col) {
            // Find the last token
            let last_space = text_before.rfind(|c: char| c.is_whitespace());
            let token = if let Some(pos) = last_space {
                &text_before[pos + 1..]
            } else {
                text_before
            };
            if !token.is_empty() {
                return self.get_file_suggestions(token);
            }
        }

        None
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> (Vec<String>, usize, usize) {
        let current_line = lines[cursor_line].clone();
        let prefix_start = cursor_col.saturating_sub(prefix.len());
        let before = &current_line[..prefix_start];
        let after = &current_line[cursor_col..];

        let (new_line, new_col) = if prefix.starts_with('/') {
            (format!("{}/{} {}", before, item.value, after), before.len() + 1 + item.value.len() + 1)
        } else {
            let suffix = if item.value.ends_with('/') { "" } else { " " };
            (format!("{}{}{}{}", before, item.value, suffix, after), before.len() + item.value.len() + suffix.len())
        };

        let mut new_lines = lines.to_vec();
        new_lines[cursor_line] = new_line;
        (new_lines, cursor_line, new_col)
    }

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let current_line = lines.get(cursor_line).map(|l| &l[..cursor_col.min(l.len())]);
        match current_line {
            Some(text) => {
                // Don't trigger in slash command name context
                if text.starts_with('/') && !text.contains(' ') {
                    return false;
                }
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slash_suggestions() {
        let provider = CombinedAutocompleteProvider::new(
            vec![
                SlashCommand {
                    name: "help".into(),
                    description: Some("Show help".into()),
                    argument_hint: None,
                },
                SlashCommand {
                    name: "history".into(),
                    description: Some("Show history".into()),
                    argument_hint: None,
                },
            ],
            "/tmp".into(),
        );

        let lines = vec!["/he".into()];
        let result = provider.get_suggestions(&lines, 0, 3, false);
        assert!(result.is_some());
        let suggestions = result.unwrap();
        assert_eq!(suggestions.items.len(), 1);
        assert_eq!(suggestions.items[0].value, "help");
    }

    #[test]
    fn test_no_slash_matches() {
        let provider = CombinedAutocompleteProvider::new(
            vec![SlashCommand {
                name: "help".into(),
                description: None,
                argument_hint: None,
            }],
            "/tmp".into(),
        );

        let lines = vec!["/unknown".into()];
        let result = provider.get_suggestions(&lines, 0, 8, false);
        assert!(result.is_none());
    }

    #[test]
    fn test_trigger_characters() {
        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());
        assert_eq!(provider.trigger_characters(), &['/', '@', '#']);
    }

    #[test]
    fn test_apply_completion_slash() {
        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());
        let item = AutocompleteItem {
            value: "help".into(),
            label: "/help".into(),
            description: None,
        };
        let lines = vec!["/".into()];
        let (new_lines, new_line, new_col) =
            provider.apply_completion(&lines, 0, 1, &item, "/");
        assert_eq!(new_lines[0], "/help "); // space appended after command
        assert_eq!(new_line, 0);
        assert_eq!(new_col, 6); // / + "help" + space
    }

    #[test]
    fn test_is_empty_items_on_empty_dir() {
        let tmp = std::env::temp_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], tmp.to_string_lossy().to_string());
        // Empty prefix should show files in temp dir (there should be at least something)
        let result = provider.get_file_suggestions("");
        assert!(result.is_some(), "Should find files in temp dir");
    }
}
