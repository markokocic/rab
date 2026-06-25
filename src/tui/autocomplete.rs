use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

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
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct SlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    /// Static argument completions (pi-compat: `getArgumentCompletions`).
    /// When set, these are filtered by the typed prefix and shown.
    /// When None and `get_argument_completions` is also None, file completion is used.
    pub argument_completions: Option<Vec<AutocompleteItem>>,
    /// Dynamic argument completions callback (pi-style `getArgumentCompletions`).
    /// Called with the typed argument prefix, returns matching items.
    /// Takes precedence over `argument_completions` when set.
    pub get_argument_completions: Option<Arc<dyn Fn(&str) -> Vec<AutocompleteItem> + Send + Sync>>,
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

// ── fd helpers (pi-compat) ───────────────────────────────────────────

/// Find the `fd` binary in PATH.
fn find_fd() -> Option<String> {
    std::env::var("PATH").ok().and_then(|path| {
        for dir in path.split(':') {
            for name in &["fd", "fdfind"] {
                let p = format!("{}/{}", dir, name);
                if std::path::Path::new(&p).is_file() {
                    return Some(p);
                }
            }
        }
        None
    })
}

/// Build the fd query from a user-typed path prefix (matches pi's buildFdPathQuery).
fn build_fd_path_query(query: &str) -> String {
    let normalized = query.replace('\\', "/");
    if !normalized.contains('/') {
        return normalized;
    }
    let has_trailing = normalized.ends_with('/');
    let trimmed = normalized.trim_matches('/');
    if trimmed.is_empty() {
        return normalized;
    }
    let sep = "[\\\\/]";
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    let mut pattern = segments
        .iter()
        .map(|s| regex::escape(s))
        .collect::<Vec<_>>()
        .join(sep);
    if has_trailing {
        pattern.push_str(sep);
    }
    pattern
}

/// Walk directory tree with `fd` (fast, respects .gitignore).
/// Mirrors pi's walkDirectoryWithFd().
fn walk_directory_with_fd(
    fd_path: &str,
    base_dir: &str,
    query: &str,
    max_results: usize,
) -> Vec<(String, bool)> {
    let mr = max_results.to_string();
    let mut cmd = Command::new(fd_path);
    cmd.arg("--base-directory")
        .arg(base_dir)
        .arg("--max-results")
        .arg(&mr)
        .arg("--type")
        .arg("f")
        .arg("--type")
        .arg("d")
        .arg("--follow")
        .arg("--hidden")
        .arg("--exclude")
        .arg(".git")
        .arg("--exclude")
        .arg(".git/*")
        .arg("--exclude")
        .arg(".git/**");

    if query.contains('/') {
        cmd.arg("--full-path");
    }

    if !query.is_empty() {
        cmd.arg(build_fd_path_query(query));
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let output = match cmd.output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let display = line.replace('\\', "/");
            if display == ".git" || display.starts_with(".git/") || display.contains("/.git/") {
                return None;
            }
            let has_trailing = display.ends_with('/');
            let normalized = if has_trailing {
                &display[..display.len() - 1]
            } else {
                &display
            };
            Some((normalized.to_string(), has_trailing))
        })
        .collect()
}

/// Score an entry against the query (higher = better match).
/// Directories get a bonus to appear first.
fn score_entry(file_path: &str, query: &str, is_directory: bool) -> usize {
    let file_name = Path::new(file_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let lower_name = file_name.to_lowercase();
    let lower_query = query.to_lowercase();

    let mut score: usize = 0;
    if lower_name == lower_query {
        score = 100;
    } else if lower_name.starts_with(&lower_query) {
        score = 80;
    } else if lower_name.contains(&lower_query) {
        score = 50;
    } else if file_path.to_lowercase().contains(&lower_query) {
        score = 30;
    }
    if is_directory && score > 0 {
        score += 10;
    }
    score
}

// ── Quoted prefix helpers (pi-compat) ─────────────────────────────────

const PATH_DELIMITERS: &[char] = &[' ', '\t', '"', '\'', '='];

/// Find an unclosed `"` or `@"` start in the text before cursor.
/// Returns the start index and the prefix slice (including @ if present).
fn find_unclosed_quote_prefix(text: &str) -> Option<(usize, &str)> {
    let mut in_quotes = false;
    let mut quote_start = 0;
    for (i, c) in text.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = i;
            }
        }
    }
    if !in_quotes {
        return None;
    }
    // Check for @" prefix
    if quote_start > 0 && text.as_bytes().get(quote_start - 1) == Some(&b'@') {
        let before_at = if quote_start > 1 {
            &text[..quote_start - 1]
        } else {
            ""
        };
        if before_at.is_empty() || before_at.ends_with(PATH_DELIMITERS) {
            return Some((quote_start - 1, &text[quote_start - 1..]));
        }
    }
    // Check for plain " prefix (token boundary)
    let before = &text[..quote_start];
    if before.is_empty() || before.ends_with(PATH_DELIMITERS) {
        return Some((quote_start, &text[quote_start..]));
    }
    None
}

/// Parse a prefix (possibly with @ or "@) into its components.
/// Returns (stripped_query, is_at_prefix, is_quoted).
fn parse_completion_prefix(prefix: &str) -> (&str, bool, bool) {
    if let Some(stripped) = prefix.strip_prefix("@\"") {
        (stripped, true, true)
    } else if let Some(stripped) = prefix.strip_prefix('"') {
        (stripped, false, true)
    } else if let Some(stripped) = prefix.strip_prefix('@') {
        (stripped, true, false)
    } else {
        (prefix, false, false)
    }
}

/// Resolve a scoped fd query: split `src/au` into base_dir=`CWD/src/` and query=`au`.
fn resolve_scoped_fd_query(raw_query: &str, base_path: &str) -> Option<(String, String, String)> {
    let normalized = raw_query.replace('\\', "/");
    let slash_index = normalized.rfind('/')?;
    let display_base = normalized[..=slash_index].to_string();
    let query = normalized[slash_index + 1..].to_string();

    let base_dir = if let Some(stripped) = display_base.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{}/{}", home, stripped)
    } else if display_base.starts_with('/') {
        display_base.clone()
    } else {
        format!("{}/{}", base_path, display_base)
    };

    if !Path::new(&base_dir).is_dir() {
        return None;
    }

    Some((base_dir, query, display_base))
}

// =============================================================================
// CombinedAutocompleteProvider - handles slash commands + file paths
// =============================================================================

/// Combined provider that handles slash commands and file path completion.
pub struct CombinedAutocompleteProvider {
    slash_commands: Vec<SlashCommand>,
    base_path: String,
    fd_path: Option<String>,
}

impl CombinedAutocompleteProvider {
    pub fn new(slash_commands: Vec<SlashCommand>, base_path: String) -> Self {
        let fd_path = find_fd();
        Self {
            slash_commands,
            base_path,
            fd_path,
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
                    (Some(d), Some(h)) => Some(format!("{} - {}", h, d)),
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

    /// Fuzzy file search using `fd` (fast, respects .gitignore).
    /// Matches pi's getFuzzyFileSuggestions().
    fn get_fuzzy_file_suggestions(&self, query: &str) -> Option<AutocompleteSuggestions> {
        let fd_path = self.fd_path.as_ref()?;

        let (fd_base_dir, fd_query, display_base) = resolve_scoped_fd_query(query, &self.base_path)
            .unwrap_or_else(|| {
                // No scope - search from base_path with the full query
                (self.base_path.clone(), query.to_string(), String::new())
            });

        let entries = walk_directory_with_fd(fd_path, &fd_base_dir, &fd_query, 100);
        if entries.is_empty() {
            return None;
        }

        let scored: Vec<(String, bool, usize)> = entries
            .into_iter()
            .map(|(path, is_dir)| {
                let score = if fd_query.is_empty() {
                    1
                } else {
                    score_entry(&path, &fd_query, is_dir)
                };
                (path, is_dir, score)
            })
            .filter(|(_, _, score)| *score > 0)
            .collect();

        if scored.is_empty() {
            return None;
        }

        // Sort by score descending, then take top 20
        let mut scored = scored;
        scored.sort_by_key(|b| std::cmp::Reverse(b.2));
        scored.truncate(20);

        let items: Vec<AutocompleteItem> = scored
            .into_iter()
            .map(|(entry_path, is_dir, _score)| {
                let entry_name = Path::new(&entry_path)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                let display_path = if display_base.is_empty() {
                    entry_path.clone()
                } else {
                    format!("{}{}", display_base, entry_path)
                };
                let completion_path = if is_dir {
                    format!("{}/", display_path)
                } else {
                    display_path.clone()
                };
                AutocompleteItem {
                    value: completion_path,
                    label: format!("{}/", entry_name),
                    description: Some(display_path),
                }
            })
            .collect();

        Some(AutocompleteSuggestions {
            items,
            prefix: query.to_string(),
        })
    }

    fn get_file_suggestions(&self, prefix: &str) -> Option<AutocompleteSuggestions> {
        // Determine search directory and file prefix
        let expanded = if let Some(stripped) = prefix.strip_prefix("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            format!("{}/{}", home, stripped)
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
            let parent = p
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or("/".into());
            let file = p
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            (
                if parent.is_empty() {
                    "/".into()
                } else {
                    parent
                },
                file,
            )
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

                let display = if prefix.starts_with('/') {
                    let base_dir = dir.clone();
                    if base_dir.ends_with('/') {
                        format!("{}{}{}", base_dir, name, suffix)
                    } else {
                        format!("{}/{}{}", base_dir, name, suffix)
                    }
                } else if let Some(rel_part) = prefix.strip_prefix("~/") {
                    let parent_path = Path::new(rel_part)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let base =
                        if rel_part.is_empty() || parent_path.is_empty() || parent_path == "." {
                            "~/".to_string()
                        } else {
                            format!("~/{}/", parent_path)
                        };
                    format!("{}{}{}", base, name, suffix)
                } else if prefix == "~" {
                    format!("~/{}{}", name, suffix)
                } else if prefix.ends_with('/') {
                    format!("{}{}{}", prefix, name, suffix)
                } else if prefix.contains('/') {
                    let p = Path::new(prefix);
                    let parent = p
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let base = if parent.is_empty() || parent == "." {
                        String::new()
                    } else {
                        format!("{}/", parent)
                    };
                    if prefix.starts_with("./") && !base.starts_with("./") {
                        format!("./{}{}{}", base, name, suffix)
                    } else {
                        format!("{}{}{}", base, name, suffix)
                    }
                } else {
                    format!("{}{}", name, suffix)
                };

                items.push(AutocompleteItem {
                    value: display,
                    label: format!("{}{}", name, suffix),
                    description: None,
                });
            }
        }

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

        // ── Slash command completion ──
        if text_before.starts_with('/') && !text_before.contains(' ') {
            let cmd = &text_before[1..];
            return self.get_slash_suggestions(cmd);
        }

        // ── Slash command argument completion ──
        if let Some(space_pos) = text_before.find(' ') {
            if space_pos == 0 {
                return None;
            }
            let cmd_name = &text_before[1..space_pos];
            let arg_text = &text_before[space_pos + 1..];
            for cmd in &self.slash_commands {
                if cmd.name == cmd_name {
                    // Check for dynamic argument completions callback (pi-style)
                    if let Some(ref get_completions) = cmd.get_argument_completions {
                        let items = get_completions(arg_text);
                        if !items.is_empty() {
                            return Some(AutocompleteSuggestions {
                                items,
                                prefix: arg_text.to_string(),
                            });
                        }
                    }
                    // Check for static argument completions (pi-compat)
                    if let Some(ref completions) = cmd.argument_completions {
                        let lower = arg_text.to_lowercase();
                        let filtered: Vec<AutocompleteItem> = completions
                            .iter()
                            .filter(|c| c.value.to_lowercase().starts_with(&lower))
                            .cloned()
                            .collect();
                        if !filtered.is_empty() {
                            return Some(AutocompleteSuggestions {
                                items: filtered,
                                prefix: arg_text.to_string(),
                            });
                        }
                    }
                    // Fall back to file path completion
                    if force
                        || arg_text.contains('/')
                        || arg_text.contains('.')
                        || arg_text.is_empty()
                    {
                        return self.get_file_suggestions(arg_text);
                    }
                    return None;
                }
            }
        }

        // ── Quoted prefix (@""" or """ for paths with spaces, pi-style) ──
        if let Some((_start, full_prefix)) = find_unclosed_quote_prefix(text_before) {
            let (query, _is_at, _is_quoted) = parse_completion_prefix(full_prefix);
            // Use fd for simple queries (no /) to find files anywhere
            if !query.contains('/')
                && !query.contains('.')
                && self.fd_path.is_some()
                && !query.is_empty()
                && let Some(suggestions) = self.get_fuzzy_file_suggestions(query)
            {
                return Some(suggestions);
            }
            return self.get_file_suggestions(query);
        }

        // ── @ and # file/attachment completion ──
        if let Some(pos) = text_before.rfind(['@', '#']) {
            let is_token_start =
                pos == 0 || text_before[..pos].ends_with(' ') || text_before[..pos].ends_with('\t');
            if is_token_start {
                let path = &text_before[pos + 1..];
                // If path doesn't contain / and fd is available, use fd for project-wide search
                if !path.contains('/')
                    && self.fd_path.is_some()
                    && !path.is_empty()
                    && let Some(suggestions) = self.get_fuzzy_file_suggestions(path)
                {
                    return Some(suggestions);
                }
                return self.get_file_suggestions(path);
            }
        }

        // ── Forced completion (Tab) ──
        if force && self.should_trigger_file_completion(lines, cursor_line, cursor_col) {
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
            // Slash command: insert with trailing space
            (
                format!("{}/{} {}", before, item.value, after),
                before.len() + 1 + item.value.len() + 1,
            )
        } else {
            // File path: use the item value directly (it's already built by the provider)
            let item_val = &item.value;
            let suffix = if item_val.ends_with('/') { "" } else { " " };
            (
                format!("{}{}{}{}", before, item_val, suffix, after),
                before.len() + item_val.len() + suffix.len(),
            )
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
        let current_line = lines
            .get(cursor_line)
            .map(|l| &l[..cursor_col.min(l.len())]);
        match current_line {
            Some(text) => {
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

    fn build_completion_value(
        path: &str,
        is_directory: bool,
        is_at_prefix: bool,
        is_quoted_prefix: bool,
    ) -> String {
        let needs_quotes = is_quoted_prefix || path.contains(' ');
        let at = if is_at_prefix { "@" } else { "" };
        let suffix = if is_directory { "/" } else { "" };
        if needs_quotes {
            format!("{}\"{}{}\"", at, path, suffix)
        } else {
            format!("{}{}{}", at, path, suffix)
        }
    }

    #[test]
    fn test_slash_suggestions() {
        let provider = CombinedAutocompleteProvider::new(
            vec![
                SlashCommand {
                    name: "help".into(),
                    description: Some("Show help".into()),
                    argument_hint: None,
                    argument_completions: None,
                    get_argument_completions: None,
                },
                SlashCommand {
                    name: "history".into(),
                    description: Some("Show history".into()),
                    argument_hint: None,
                    argument_completions: None,
                    get_argument_completions: None,
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
                argument_completions: None,
                get_argument_completions: None,
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
        let (new_lines, new_line, new_col) = provider.apply_completion(&lines, 0, 1, &item, "/");
        assert_eq!(new_lines[0], "/help ");
        assert_eq!(new_line, 0);
        assert_eq!(new_col, 6);
    }

    #[test]
    fn test_find_unclosed_quote_prefix_basic() {
        assert!(find_unclosed_quote_prefix("hello \"world").is_some());
        assert!(find_unclosed_quote_prefix("hello \"world\"").is_none());
        assert!(find_unclosed_quote_prefix("no quotes").is_none());
    }

    #[test]
    fn test_find_unclosed_quote_prefix_at() {
        let result = find_unclosed_quote_prefix("hello @\"path");
        assert!(result.is_some());
        let (_start, prefix) = result.unwrap();
        assert_eq!(&prefix[..1], "@");
    }

    #[test]
    fn test_parse_completion_prefix() {
        let (q, at, quoted) = parse_completion_prefix("@\"path");
        assert_eq!(q, "path");
        assert!(at);
        assert!(quoted);

        let (q, at, quoted) = parse_completion_prefix("\"path");
        assert_eq!(q, "path");
        assert!(!at);
        assert!(quoted);

        let (q, at, quoted) = parse_completion_prefix("@path");
        assert_eq!(q, "path");
        assert!(at);
        assert!(!quoted);

        let (q, at, quoted) = parse_completion_prefix("path");
        assert_eq!(q, "path");
        assert!(!at);
        assert!(!quoted);
    }

    #[test]
    fn test_build_completion_value() {
        let v = build_completion_value("foo.rs", false, true, false);
        assert_eq!(v, "@foo.rs");

        let v = build_completion_value("foo.rs", false, false, false);
        assert_eq!(v, "foo.rs");

        let v = build_completion_value("my dir/file.rs", false, true, false);
        assert_eq!(v, "@\"my dir/file.rs\"");
    }

    #[test]
    fn test_is_empty_items_on_empty_dir() {
        let tmp = std::env::temp_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], tmp.to_string_lossy().to_string());
        let result = provider.get_file_suggestions("");
        assert!(result.is_some(), "Should find files in temp dir");
    }

    #[test]
    fn test_build_fd_path_query() {
        assert_eq!(build_fd_path_query("hello"), "hello");
        assert_eq!(build_fd_path_query("src/main.rs"), "src[\\\\/]main\\.rs");
        assert!(build_fd_path_query("src/").ends_with("[\\\\/]"));
    }

    #[test]
    fn test_score_entry() {
        let s = score_entry("src/main.rs", "main", false);
        assert!(s > 0, "Should score positive for matching name");
        let s = score_entry("src/main.rs", "nomatch", false);
        assert_eq!(s, 0, "Should score zero for no match");
    }
}
