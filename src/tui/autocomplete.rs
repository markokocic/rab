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
                if name == ".git" || (name.starts_with('.') && !file_prefix.starts_with('.')) {
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
                    // When rel_part has a trailing slash (e.g., ".rab/agent/"),
                    // use it directly as the base to preserve the last folder.
                    // Path::new().parent() would strip it (e.g., ".rab/agent/" → ".rab").
                    let base = if rel_part.ends_with('/') {
                        format!("~/{}", rel_part)
                    } else {
                        let parent_path = Path::new(rel_part)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if rel_part.is_empty() || parent_path.is_empty() || parent_path == "." {
                            "~/".to_string()
                        } else {
                            format!("~/{}/", parent_path)
                        }
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
            if let Some(suggestions) = self.get_slash_suggestions(cmd) {
                return Some(suggestions);
            }
            // No slash command match – fall through to file completion for absolute paths like /tmp
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

        // ── ~ path completion (tilde expansion) ──
        if let Some(pos) = text_before.rfind('~') {
            let is_token_start =
                pos == 0 || text_before[..pos].ends_with(' ') || text_before[..pos].ends_with('\t');
            if is_token_start {
                let path = &text_before[pos..];
                return self.get_file_suggestions(path);
            }
        }

        // ── Absolute path completion (/) – automatic (non-force) fallback for paths
        //     that didn't match any slash command ──
        if text_before.starts_with('/') && !text_before.contains(' ') && text_before.len() > 1 {
            return self.get_file_suggestions(text_before);
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

        // Determine if this is a slash command completion or a file path completion.
        // Slash commands have item.value = "help" (no leading /, ~, or . path chars).
        // File paths have item.value = "/tmp/", "~/.rab/agent/", or "src/main.rs".
        let is_slash_command = prefix.starts_with('/')
            && !item.value.starts_with('/')
            && !item.value.starts_with('~')
            && !item.value.starts_with('.');

        let (new_line, new_col) = if is_slash_command {
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
                // Only block Tab completion for known slash commands on line 0.
                // Absolute paths like /usr/share/ should still get file completion.
                if text.starts_with('/') && !text.contains(' ') && cursor_line == 0 {
                    let cmd_input = text[1..].trim();
                    if cmd_input.is_empty() {
                        // Just "/" — don't trigger file completion yet
                        return false;
                    }
                    // If text matches a known slash command, don't trigger file completion
                    if self
                        .slash_commands
                        .iter()
                        .any(|c| c.name.starts_with(cmd_input))
                    {
                        return false;
                    }
                    // Otherwise it's an absolute path — allow file completion
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

    // ── Tests for fixed bugs ──

    #[test]
    fn test_apply_completion_absolute_path_no_double_slash() {
        // Bug 1: completing / → tmp/ should give /tmp/ not //tmp/
        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());
        // Absolute path file completion (item.value starts with /)
        let item = AutocompleteItem {
            value: "/tmp/".into(),
            label: "tmp/".into(),
            description: None,
        };
        let lines = vec!["/".into()];
        let (new_lines, _new_line, _new_col) = provider.apply_completion(&lines, 0, 1, &item, "/");
        // Should NOT produce //tmp/
        assert_eq!(
            new_lines[0], "/tmp/",
            "Absolute path completion must not add extra slash"
        );
    }

    #[test]
    fn test_apply_completion_slash_command_still_works() {
        // Slash commands should still produce /cmd (with one slash)
        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());
        let item = AutocompleteItem {
            value: "help".into(),
            label: "/help".into(),
            description: None,
        };
        let lines = vec!["/".into()];
        let (new_lines, _new_line, new_col) = provider.apply_completion(&lines, 0, 1, &item, "/");
        assert_eq!(new_lines[0], "/help ");
        assert_eq!(new_col, 6);
    }

    #[test]
    fn test_get_file_suggestions_absolute_path() {
        // Bug 1: get_suggestions for absolute paths like /tmp should work
        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());
        let lines = vec!["/tmp".into()];
        let result = provider.get_suggestions(&lines, 0, 4, false);
        // /tmp is a directory, should show its contents
        assert!(
            result.is_some(),
            "Absolute path /tmp should produce suggestions"
        );
        let suggestions = result.unwrap();
        assert!(
            !suggestions.items.is_empty(),
            "Should have entries from /tmp"
        );
        assert_eq!(suggestions.prefix, "/tmp");
    }

    #[test]
    fn test_get_suggestions_slash_falls_through_to_file_completion() {
        // When no slash command matches, absolute paths should get file completion
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
        let lines = vec!["/tmp".into()];
        // /tmp doesn't match any slash command, should fall through to file completion
        let result = provider.get_suggestions(&lines, 0, 4, false);
        assert!(
            result.is_some(),
            "/tmp should fall through to file completion"
        );
    }

    #[test]
    fn test_get_suggestions_tilde_path() {
        // Bug 2: ~ paths should trigger file completion (non-force)
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() || !std::path::Path::new(&home).is_dir() {
            // Skip if HOME is not set or not a directory
            return;
        }
        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());
        let lines = vec!["~/".into()];
        let result = provider.get_suggestions(&lines, 0, 2, false);
        assert!(result.is_some(), "~ path should produce file suggestions");
    }

    #[test]
    fn test_hidden_file_filter_with_dot_prefix() {
        // Bug 2: when query starts with '.', hidden files should be shown
        let tmp = std::env::temp_dir();
        // Create a temp dir with a hidden file
        let dir = tmp.join("autocomplete_test_dot");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".hidden_file"), "").unwrap();
        std::fs::write(dir.join("visible_file"), "").unwrap();
        std::fs::create_dir(dir.join(".hidden_dir")).unwrap();
        std::fs::create_dir(dir.join("visible_dir")).unwrap();

        let provider = CombinedAutocompleteProvider::new(vec![], dir.to_string_lossy().to_string());
        let dir_str = dir.to_string_lossy();

        // Query with dot prefix should show hidden files
        let result = provider.get_file_suggestions(&format!("{}/.h", dir_str));
        assert!(
            result.is_some(),
            "Dot prefix query should find hidden files"
        );
        if let Some(suggestions) = result {
            let values: Vec<&str> = suggestions.items.iter().map(|i| i.value.as_str()).collect();
            assert!(
                values.iter().any(|v| v.contains(".hidden")),
                "Should find .hidden_file or .hidden_dir, got: {:?}",
                values
            );
        }

        // Query without dot prefix should NOT show hidden files
        let result2 = provider.get_file_suggestions(&format!("{}/v", dir_str));
        assert!(result2.is_some(), "Non-dot prefix query should find files");
        if let Some(suggestions) = result2 {
            let values: Vec<&str> = suggestions.items.iter().map(|i| i.value.as_str()).collect();
            assert!(
                values.iter().any(|v| v.contains("visible")),
                "Should find visible_file or visible_dir"
            );
            assert!(
                !values.iter().any(|v| v.contains(".hidden")),
                "Should NOT find hidden files with non-dot prefix"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_get_suggestions_slash_command_still_works() {
        // Existing slash command completion should not be broken
        let provider = CombinedAutocompleteProvider::new(
            vec![SlashCommand {
                name: "help".into(),
                description: Some("Show help".into()),
                argument_hint: None,
                argument_completions: None,
                get_argument_completions: None,
            }],
            "/tmp".into(),
        );

        let lines = vec!["/he".into()];
        let result = provider.get_suggestions(&lines, 0, 3, false);
        assert!(result.is_some());
        let suggestions = result.unwrap();
        assert_eq!(suggestions.items.len(), 1);
        assert_eq!(suggestions.items[0].value, "help");
    }

    // ── Path completion regression tests ──

    /// Create a temp directory structure for path completion tests.
    /// Structure:
    ///   temp/
    ///     src/
    ///       autocomplete/
    ///         mod.rs
    ///       editor.rs
    ///       components/
    ///         select_list.rs
    fn setup_path_test_dir() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let root = dir.path().to_string_lossy().to_string();

        // Create structure
        std::fs::create_dir_all(format!("{}/src/autocomplete", root)).unwrap();
        std::fs::create_dir_all(format!("{}/src/components", root)).unwrap();
        std::fs::write(format!("{}/src/autocomplete/mod.rs", root), "").unwrap();
        std::fs::write(format!("{}/src/editor.rs", root), "").unwrap();
        std::fs::write(format!("{}/src/components/select_list.rs", root), "").unwrap();

        (dir, root)
    }

    #[test]
    fn test_get_file_suggestions_relative_path_with_folder() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Typed "src/au" -> should find "src/autocomplete/"
        let result = provider.get_file_suggestions("src/au");
        assert!(result.is_some(), "src/au should produce suggestions");
        let suggestions = result.unwrap();
        assert_eq!(
            suggestions.prefix, "src/au",
            "prefix should be the typed text"
        );
        assert!(
            !suggestions.items.is_empty(),
            "should have at least one item"
        );

        // The item value should include the full relative path
        let has_autocomplete = suggestions
            .items
            .iter()
            .any(|i| i.value == "src/autocomplete/");
        assert!(
            has_autocomplete,
            "should contain src/autocomplete/ as a completion candidate, got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_get_file_suggestions_relative_path_trailing_slash() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Typed "src/" -> should show contents of src/
        let result = provider.get_file_suggestions("src/");
        assert!(result.is_some(), "src/ should produce suggestions");
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, "src/", "prefix should be src/");

        // Should contain entries like src/autocomplete/, src/editor.rs, src/components/
        let values: Vec<&str> = suggestions.items.iter().map(|i| i.value.as_str()).collect();
        assert!(
            values.contains(&"src/autocomplete/"),
            "should contain src/autocomplete/, got: {:?}",
            values
        );
        assert!(
            values.contains(&"src/editor.rs"),
            "should contain src/editor.rs, got: {:?}",
            values
        );
        assert!(
            values.contains(&"src/components/"),
            "should contain src/components/, got: {:?}",
            values
        );
    }

    #[test]
    fn test_get_file_suggestions_deep_path() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Typed "src/components/s" -> should find "src/components/select_list.rs"
        let result = provider.get_file_suggestions("src/components/s");
        assert!(
            result.is_some(),
            "src/components/s should produce suggestions"
        );
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, "src/components/s");

        let has_select_list = suggestions
            .items
            .iter()
            .any(|i| i.value == "src/components/select_list.rs");
        assert!(
            has_select_list,
            "should contain src/components/select_list.rs, got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_get_suggestions_force_triggers_file_completion() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Simulate Tab (force=true) with "src/au" typed
        let lines = vec!["src/au".into()];
        let result = provider.get_suggestions(&lines, 0, 6, true);
        assert!(
            result.is_some(),
            "Force should trigger file completion for src/au"
        );
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, "src/au");

        let has_autocomplete = suggestions
            .items
            .iter()
            .any(|i| i.value == "src/autocomplete/");
        assert!(
            has_autocomplete,
            "Should suggest src/autocomplete/, got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_get_suggestions_at_prefix_file_completion() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Typed "@src/au" should complete to "@src/autocomplete/"
        let lines = vec!["@src/au".into()];
        let result = provider.get_suggestions(&lines, 0, 7, false);
        assert!(result.is_some(), "@src/au should produce suggestions");
        let suggestions = result.unwrap();
        // Prefix should NOT include the @
        assert_eq!(suggestions.prefix, "src/au", "prefix should not include @");

        let has_autocomplete = suggestions
            .items
            .iter()
            .any(|i| i.value == "src/autocomplete/");
        assert!(
            has_autocomplete,
            "Should suggest src/autocomplete/, got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_apply_completion_relative_path_with_folder() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // User typed "src/au", cursor at end. Accept proposal "src/autocomplete/"
        let item = AutocompleteItem {
            value: "src/autocomplete/".into(),
            label: "autocomplete/".into(),
            description: None,
        };
        let lines = vec!["src/au".into()];
        let (new_lines, new_line, new_col) =
            provider.apply_completion(&lines, 0, 6, &item, "src/au");

        assert_eq!(
            new_lines[0], "src/autocomplete/",
            "Should replace src/au with src/autocomplete/"
        );
        assert_eq!(new_line, 0);
        assert_eq!(new_col, 17); // "src/autocomplete/".len() = 17
    }

    #[test]
    fn test_apply_completion_relative_path_trailing_slash() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // User typed "src/", cursor at end. Accept proposal "src/autocomplete/"
        let item = AutocompleteItem {
            value: "src/autocomplete/".into(),
            label: "autocomplete/".into(),
            description: None,
        };
        let lines = vec!["src/".into()];
        let (new_lines, new_line, new_col) = provider.apply_completion(&lines, 0, 4, &item, "src/");

        assert_eq!(
            new_lines[0], "src/autocomplete/",
            "Should replace src/ with src/autocomplete/"
        );
        assert_eq!(new_line, 0);
        assert_eq!(new_col, 17);
    }

    #[test]
    fn test_apply_completion_at_prefix() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // User typed "@src/au", cursor at end. Accept proposal "src/autocomplete/"
        let item = AutocompleteItem {
            value: "src/autocomplete/".into(),
            label: "autocomplete/".into(),
            description: None,
        };
        let lines = vec!["@src/au".into()];
        // cursor_col = 7 (position after "@src/au"), prefix = "src/au" (without @)
        let (new_lines, new_line, new_col) =
            provider.apply_completion(&lines, 0, 7, &item, "src/au");

        assert_eq!(
            new_lines[0], "@src/autocomplete/",
            "Should replace src/au with src/autocomplete/, keeping @ prefix"
        );
        assert_eq!(new_line, 0);
        assert_eq!(new_col, 18); // "@src/autocomplete/".len() = 18
    }

    #[test]
    fn test_apply_completion_deep_path() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // User typed "src/components/s", cursor at end. Accept "src/components/select_list.rs"
        let item = AutocompleteItem {
            value: "src/components/select_list.rs".into(),
            label: "select_list.rs".into(),
            description: None,
        };
        let lines = vec!["src/components/s".into()];
        let (new_lines, new_line, new_col) =
            provider.apply_completion(&lines, 0, 16, &item, "src/components/s");

        assert_eq!(
            new_lines[0], "src/components/select_list.rs ",
            "Should complete deep path correctly"
        );
        assert_eq!(new_line, 0);
        assert_eq!(new_col, 30); // "src/components/select_list.rs ".len() = 30
    }

    #[test]
    fn test_apply_completion_at_prefix_deep_path() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // User typed "@src/components/s", cursor at end. Accept "src/components/select_list.rs"
        let item = AutocompleteItem {
            value: "src/components/select_list.rs".into(),
            label: "select_list.rs".into(),
            description: None,
        };
        let lines = vec!["@src/components/s".into()];
        // cursor_col = 17 (position after "@src/components/s"), prefix = "src/components/s"
        let (new_lines, new_line, new_col) =
            provider.apply_completion(&lines, 0, 17, &item, "src/components/s");

        assert_eq!(
            new_lines[0], "@src/components/select_list.rs ",
            "Should complete deep @-path correctly"
        );
        assert_eq!(new_line, 0);
        assert_eq!(new_col, 31); // "@src/components/select_list.rs ".len() = 31
    }

    #[test]
    fn test_apply_completion_after_folder_completion_then_deeper() {
        // Regression: after completing src/ -> src/autocomplete/, then typing more to go deeper
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Step 1: complete src/ -> src/autocomplete/
        let item1 = AutocompleteItem {
            value: "src/autocomplete/".into(),
            label: "autocomplete/".into(),
            description: None,
        };
        let lines = vec!["src/".into()];
        let (new_lines, _, _) = provider.apply_completion(&lines, 0, 4, &item1, "src/");
        assert_eq!(new_lines[0], "src/autocomplete/");

        // Step 2: user types more, now text is "src/autocomplete/m"
        let text = format!("{}m", new_lines[0]);
        let cursor_col = text.len(); // "src/autocomplete/m" is 18 chars
        let lines2 = vec![text];
        // Get suggestions
        let result = provider.get_suggestions(&lines2, 0, cursor_col, true);
        assert!(
            result.is_some(),
            "src/autocomplete/m should produce suggestions"
        );
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, "src/autocomplete/m");

        // Should find "src/autocomplete/mod.rs"
        let has_mod = suggestions
            .items
            .iter()
            .any(|i| i.value == "src/autocomplete/mod.rs");
        assert!(
            has_mod,
            "Should suggest src/autocomplete/mod.rs, got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );

        // Step 3: accept the completion
        let item2 = AutocompleteItem {
            value: "src/autocomplete/mod.rs".into(),
            label: "mod.rs".into(),
            description: None,
        };
        let (final_lines, _, _) =
            provider.apply_completion(&lines2, 0, cursor_col, &item2, "src/autocomplete/m");
        assert_eq!(
            final_lines[0], "src/autocomplete/mod.rs ",
            "After completing deeper, should keep the full path"
        );
    }

    /// Test that get_file_suggestions produces item values that, when passed
    /// back to apply_completion, produce the correct result (round-trip test).
    #[test]
    fn test_file_suggestions_roundtrip() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Get suggestions for "src/au"
        let result = provider.get_file_suggestions("src/au").unwrap();
        assert_eq!(result.prefix, "src/au");

        // For each suggestion, verify that apply_completion works correctly
        for item in &result.items {
            let lines = vec!["src/au".into()];
            let (new_lines, _, _) = provider.apply_completion(&lines, 0, 6, item, "src/au");
            let _expected_len = "src/au".len() + item.value.len() - "src/au".len();
            // The item.value should be the replacement text (replacing the prefix)
            // Since the prefix is at the start, the result should start with item.value
            assert!(
                new_lines[0].starts_with(item.value.trim_end_matches(' ')),
                "apply_completion({}, {:?}) should produce text starting with '{}', got '{}'",
                "src/au",
                item.value,
                item.value.trim_end_matches(' '),
                new_lines[0]
            );
        }
    }

    #[test]
    fn test_at_suggestions_roundtrip() {
        let (_dir, root) = setup_path_test_dir();
        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Get suggestions for "@src/au" (prefix should be "src/au")
        let lines = vec!["@src/au".into()];
        let result = provider.get_suggestions(&lines, 0, 7, false).unwrap();
        assert_eq!(result.prefix, "src/au");

        // For each suggestion, verify that apply_completion works correctly
        for item in &result.items {
            let lines = vec!["@src/au".into()];
            let (new_lines, _, _) = provider.apply_completion(&lines, 0, 7, item, "src/au");

            // The @ should be preserved, followed by the completion value
            assert!(
                new_lines[0].starts_with('@'),
                "apply_completion for @src/au should preserve @ prefix, got '{}'",
                new_lines[0]
            );
            // The @ should be followed by the completion value (minus trailing space)
            let after_at = &new_lines[0][1..];
            let trimmed = after_at.trim_end_matches(' ');
            assert_eq!(
                trimmed, item.value,
                "Text after @ should match item.value, got '{}' vs '{}'",
                trimmed, item.value
            );
        }
    }

    #[test]
    fn test_tilde_path_completion_does_not_drop_folder() {
        // Regression: completing ~/.rab/agent/skills must NOT produce ~/.rab/skills/
        let (_dir, root) = setup_path_test_dir();

        // Create a nested structure matching the user's scenario:
        //   temp/
        //     sub/
        //       deep/
        //         target/
        //           file.txt
        // To test: complete "sub/deep/tar" -> "sub/deep/target/"
        std::fs::create_dir_all(format!("{}/sub/deep/target", root)).unwrap();
        std::fs::write(format!("{}/sub/deep/target/file.txt", root), "").unwrap();

        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Test get_file_suggestions produces correct relative path
        let result = provider.get_file_suggestions("sub/deep/tar");
        assert!(result.is_some(), "sub/deep/tar should produce suggestions");
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, "sub/deep/tar");

        let has_target = suggestions
            .items
            .iter()
            .any(|i| i.value == "sub/deep/target/");
        assert!(
            has_target,
            "Should suggest sub/deep/target/, not target/ alone. Got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );

        // Test apply_completion produces the full path
        let item = AutocompleteItem {
            value: "sub/deep/target/".into(),
            label: "target/".into(),
            description: None,
        };
        let lines = vec!["sub/deep/tar".into()];
        let (new_lines, _, _) = provider.apply_completion(&lines, 0, 12, &item, "sub/deep/tar");
        assert_eq!(
            new_lines[0], "sub/deep/target/",
            "Must produce sub/deep/target/ not target/ alone"
        );
    }

    #[test]
    fn test_nested_path_with_get_suggestions_force() {
        let (_dir, root) = setup_path_test_dir();

        std::fs::create_dir_all(format!("{}/sub/deep/target", root)).unwrap();
        std::fs::write(format!("{}/sub/deep/target/file.txt", root), "").unwrap();

        let provider = CombinedAutocompleteProvider::new(vec![], root.clone());

        // Simulate Tab (force) with "sub/deep/tar"
        let lines = vec!["sub/deep/tar".into()];
        let result = provider.get_suggestions(&lines, 0, 13, true);
        assert!(
            result.is_some(),
            "Force should trigger file completion for sub/deep/tar"
        );
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, "sub/deep/tar");

        let has_target = suggestions
            .items
            .iter()
            .any(|i| i.value == "sub/deep/target/");
        assert!(
            has_target,
            "Force should suggest sub/deep/target/. Got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_nested_path_with_tilde_prefix() {
        // Test that ~/ path completion preserves nested folders
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return;
        }

        // Create nested dir inside home
        let test_dir = std::path::Path::new(&home).join(".rab_test_autocomplete");
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(test_dir.join("sub/deep/target")).unwrap();
        std::fs::write(test_dir.join("sub/deep/target/file.txt"), "").unwrap();

        // The CWD doesn't matter for ~/ paths since we use HOME
        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());

        let tilde_path = "~/.rab_test_autocomplete/sub/deep/tar".to_string();
        let result = provider.get_file_suggestions(&tilde_path);
        assert!(result.is_some(), "~/ path should produce suggestions");
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, tilde_path);

        let expected_value = "~/.rab_test_autocomplete/sub/deep/target/".to_string();
        let has_target = suggestions.items.iter().any(|i| i.value == expected_value);
        assert!(
            has_target,
            "Should suggest ~/.rab_test_autocomplete/sub/deep/target/, not target/ alone. Got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );

        // Test apply_completion preserves the full ~/ path
        let item = AutocompleteItem {
            value: expected_value.clone(),
            label: "target/".into(),
            description: None,
        };
        let lines = vec![tilde_path.clone()];
        let cursor_col = tilde_path.len();
        let (new_lines, _, _) =
            provider.apply_completion(&lines, 0, cursor_col, &item, &tilde_path);
        assert_eq!(
            new_lines[0], expected_value,
            "Must preserve full ~/ path, not drop folders"
        );

        // Clean up
        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_tilde_path_with_trailing_slash_preserves_folder() {
        // Regression: completing "~/.rab/agent/" and selecting "skills"
        // should produce "~/.rab/agent/skills/", not "~/.rab/skills/"
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return;
        }

        let test_dir = std::path::Path::new(&home).join(".rab_test_trailing");
        let _ = std::fs::remove_dir_all(&test_dir);
        // Create: ~/test_trailing/sub/deep/target/
        std::fs::create_dir_all(test_dir.join("sub/deep/target")).unwrap();
        std::fs::write(test_dir.join("sub/deep/target/file.txt"), "").unwrap();

        let provider = CombinedAutocompleteProvider::new(vec![], "/tmp".into());

        // User typed "~/.rab_test_trailing/sub/deep/" (trailing slash)
        let tilde_path = "~/.rab_test_trailing/sub/deep/".to_string();
        let result = provider.get_file_suggestions(&tilde_path);
        assert!(
            result.is_some(),
            "~/ path with trailing slash should produce suggestions"
        );
        let suggestions = result.unwrap();
        assert_eq!(suggestions.prefix, tilde_path);

        // The suggestion value should include the full path, not just the last component
        let expected_value = "~/.rab_test_trailing/sub/deep/target/".to_string();
        let has_target = suggestions.items.iter().any(|i| i.value == expected_value);
        assert!(
            has_target,
            "Must suggest full path ~/.rab_test_trailing/sub/deep/target/, not target/ alone. Got: {:?}",
            suggestions
                .items
                .iter()
                .map(|i| &i.value)
                .collect::<Vec<_>>()
        );

        // Test apply_completion with this prefix
        let item = AutocompleteItem {
            value: expected_value.clone(),
            label: "target/".into(),
            description: None,
        };
        let lines = vec![tilde_path.clone()];
        let cursor_col = tilde_path.len();
        let (new_lines, _, _) =
            provider.apply_completion(&lines, 0, cursor_col, &item, &tilde_path);
        assert_eq!(
            new_lines[0], expected_value,
            "Must produce full path, not drop the last folder"
        );

        // Clean up
        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
