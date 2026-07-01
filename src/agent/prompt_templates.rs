//! Prompt templates — load `.md` files from prompt directories and expand `/name` commands.
//!
//! Follows pi's prompt template system:
//! - `.md` files in `~/.rab/agent/prompts/` or `.rab/prompts/`
//! - Filename (minus `.md`) becomes the `/name` command
//! - Frontmatter supports `description` and `argument-hint`
//! - Body supports `$1`, `$2`, ..., `$@`, `$ARGUMENTS`, `${N:-default}`, `${@:N}`, `${@:N:L}`

use std::fs;
use std::path::{Path, PathBuf};

/// A loaded prompt template.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    /// Template name (filename without .md), used as `/name`.
    pub name: String,
    /// Short description for autocomplete (from frontmatter or first line).
    pub description: String,
    /// Optional argument hint for autocomplete (from frontmatter `argument-hint`).
    pub argument_hint: Option<String>,
    /// Template body with `$1`, `$@`, etc. placeholders.
    pub content: String,
    /// Absolute path to the `.md` file.
    pub file_path: PathBuf,
}

/// Load prompt templates from one or more directories.
///
/// Scans each directory for `.md` files (non-recursive), parses YAML frontmatter
/// for `description` and `argument-hint`. Missing directories are silently skipped.
/// Later directories override earlier ones on name conflict.
pub fn load_prompt_templates(dirs: &[impl AsRef<Path>]) -> Vec<PromptTemplate> {
    // Collect all templates (allow duplicates)
    let mut all: Vec<PromptTemplate> = Vec::new();

    for dir in dirs {
        let dir = dir.as_ref();
        if !dir.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let name = match path.file_stem().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let (description, argument_hint, body) = parse_template(&content);
            all.push(PromptTemplate {
                name,
                description,
                argument_hint,
                content: body,
                file_path: fs::canonicalize(&path).unwrap_or(path),
            });
        }
    }

    // Deduplicate: later entries override earlier ones on name conflict.
    // Reverse so first occurrence (which should be last) wins in the dedup map,
    // then reverse back to preserve original order within the same dir.
    let mut templates: Vec<PromptTemplate> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in all.into_iter().rev() {
        if seen.insert(t.name.clone()) {
            templates.push(t);
        }
    }
    templates.reverse();
    // Sort by name for stable output
    templates.sort_by(|a, b| a.name.cmp(&b.name));
    templates
}

/// Parse frontmatter and extract description, argument-hint, and body.
fn parse_template(content: &str) -> (String, Option<String>, String) {
    let trimmed = content.trim_start();

    // Default: no frontmatter
    let mut description = String::new();
    let mut argument_hint: Option<String> = None;
    let body: String;

    if let Some(after_open) = trimmed.strip_prefix("---") {
        if let Some(end) = after_open.find("\n---") {
            let yaml_block = &after_open[..end];
            body = after_open[end + 4..].trim().to_string();

            // Simple line-by-line frontmatter parsing (no YAML dep)
            for line in yaml_block.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("description:") {
                    description = unquote(rest.trim());
                } else if let Some(rest) = line.strip_prefix("argument-hint:") {
                    argument_hint = Some(unquote(rest.trim()));
                }
            }
        } else {
            body = trimmed.to_string();
        }
    } else {
        body = trimmed.to_string();
    }

    // Fallback: first non-empty line truncated to 60 chars
    if description.is_empty()
        && let Some(first) = body.lines().find(|l| !l.trim().is_empty())
    {
        let first = first.trim();
        if first.len() > 60 {
            description = format!("{}...", &first[..60]);
        } else {
            description = first.to_string();
        }
    }

    (description, argument_hint, body)
}

/// Remove surrounding quotes from a value.
fn unquote(s: &str) -> String {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse command arguments respecting quoted strings (shell-style).
///
/// Supports single and double quotes. Unclosed quotes include the rest of the string.
pub fn parse_command_args(input: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for ch in input.chars() {
        match in_quote {
            Some(quote) => {
                if ch == quote {
                    in_quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                } else if ch.is_whitespace() {
                    if !current.is_empty() {
                        args.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(ch);
                }
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Substitute argument placeholders in template content.
///
/// Supports:
/// - `$1`, `$2`, ... — positional args (1-indexed, empty string if missing)
/// - `$@` and `$ARGUMENTS` — all args joined by space
/// - `${N:-default}` — positional arg with default when missing/empty
/// - `${@:N}` — all args from position N (1-indexed)
/// - `${@:N:L}` — L args starting from position N
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let all_args = args.join(" ");
    let mut result = String::new();
    let mut rest = content;

    loop {
        match rest.find('$') {
            None => {
                result.push_str(rest);
                break;
            }
            Some(dollar_pos) => {
                // Push everything before $
                result.push_str(&rest[..dollar_pos]);
                rest = &rest[dollar_pos + 1..]; // skip $

                if rest.is_empty() {
                    result.push('$');
                    break;
                }

                // Check what follows $
                if rest.starts_with('{') {
                    // ${...}
                    rest = &rest[1..]; // skip {
                    let close = match rest.find('}') {
                        Some(i) => i,
                        None => {
                            // No closing brace — literal
                            result.push_str("${");
                            result.push_str(rest);
                            break;
                        }
                    };
                    let inner = &rest[..close];
                    rest = &rest[close + 1..]; // skip }

                    if let Some(default_idx) = inner.find(":-") {
                        // ${N:-default}
                        let num_str = &inner[..default_idx];
                        let default = &inner[default_idx + 2..];
                        if let Ok(idx) = num_str.parse::<usize>() {
                            let value = args
                                .get(idx.wrapping_sub(1))
                                .map(|s| s.as_str())
                                .unwrap_or("");
                            if value.is_empty() {
                                result.push_str(default);
                            } else {
                                result.push_str(value);
                            }
                        }
                    } else if let Some(colon) = inner.find(':') {
                        // ${@:N} or ${@:N:L}
                        let prefix = &inner[..colon];
                        let rest_slice = &inner[colon + 1..];
                        if prefix == "@" {
                            if let Some(len_str) = rest_slice.find(':') {
                                // ${@:N:L}
                                let start_str = &rest_slice[..len_str];
                                let length_str = &rest_slice[len_str + 1..];
                                if let Ok(start) = start_str.parse::<usize>() {
                                    let start_idx = start.saturating_sub(1);
                                    if let Ok(len) = length_str.parse::<usize>() {
                                        let slice: Vec<&str> = args
                                            .iter()
                                            .skip(start_idx)
                                            .take(len)
                                            .map(|s| s.as_str())
                                            .collect();
                                        result.push_str(&slice.join(" "));
                                    }
                                }
                            } else {
                                // ${@:N}
                                if let Ok(start) = rest_slice.parse::<usize>() {
                                    let start_idx = start.saturating_sub(1);
                                    let slice: Vec<&str> =
                                        args.iter().skip(start_idx).map(|s| s.as_str()).collect();
                                    result.push_str(&slice.join(" "));
                                }
                            }
                        }
                    }
                } else if rest.starts_with('@') {
                    // $@
                    result.push_str(&all_args);
                    rest = &rest[1..];
                } else if rest.starts_with("ARGUMENTS") {
                    // $ARGUMENTS
                    result.push_str(&all_args);
                    rest = &rest[9..];
                } else if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    // $N
                    let digit_end =
                        rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                    let num_str = &rest[..digit_end];
                    if let Ok(idx) = num_str.parse::<usize>() {
                        let value = args
                            .get(idx.wrapping_sub(1))
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        result.push_str(value);
                    }
                    rest = &rest[digit_end..];
                } else {
                    // Lone $ or $ followed by non-special char — literal
                    result.push('$');
                    // Rest already advanced past $, so continue
                }
            }
        }
    }

    result
}

/// Expand a prompt template if the text matches a `/name` command.
///
/// Returns the expanded content if a matching template is found,
/// or the original text if no template matches.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return text.to_string();
    }

    // Extract "/name" and optional args
    let rest = &trimmed[1..];
    let (name, args_str) = match rest.find(' ') {
        Some(pos) => (&rest[..pos], rest[pos + 1..].trim()),
        None => (rest, ""),
    };

    // Find matching template
    if let Some(template) = templates.iter().find(|t| t.name == name) {
        let args = parse_command_args(args_str);
        substitute_args(&template.content, &args)
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_template(dir: &Path, name: &str, description: &str, body: &str) {
        let content = format!("---\ndescription: {}\n---\n\n{}", description, body);
        fs::write(dir.join(format!("{}.md", name)), content).unwrap();
    }

    #[test]
    fn test_load_templates_from_directory() {
        let tmp = TempDir::new().unwrap();
        create_template(tmp.path(), "fix", "Fix a compiler error", "Run $1");
        create_template(tmp.path(), "test", "Run tests", "Run tests for $@");

        let templates = load_prompt_templates(&[tmp.path()]);
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0].name, "fix");
        assert_eq!(templates[1].name, "test");
    }

    #[test]
    fn test_load_templates_skips_non_md() {
        let tmp = TempDir::new().unwrap();
        create_template(tmp.path(), "fix", "Fix", "content");
        fs::write(tmp.path().join("notes.txt"), "not a template").unwrap();

        let templates = load_prompt_templates(&[tmp.path()]);
        assert_eq!(templates.len(), 1);
    }

    #[test]
    fn test_parse_command_args_basic() {
        let args = parse_command_args("hello world");
        assert_eq!(args, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_quoted() {
        let args = parse_command_args("hello \"quoted world\" end");
        assert_eq!(args, vec!["hello", "quoted world", "end"]);
    }

    #[test]
    fn test_parse_command_args_single_quotes() {
        let args = parse_command_args("hello 'single quoted' end");
        assert_eq!(args, vec!["hello", "single quoted", "end"]);
    }

    #[test]
    fn test_substitute_args_positional() {
        let result = substitute_args("fix $1 and $2", &["foo".into(), "bar".into()]);
        assert_eq!(result, "fix foo and bar");
    }

    #[test]
    fn test_substitute_args_all() {
        let result = substitute_args("run $@", &["a".into(), "b".into(), "c".into()]);
        assert_eq!(result, "run a b c");
    }

    #[test]
    fn test_substitute_args_arguments() {
        let result = substitute_args("run $ARGUMENTS", &["x".into(), "y".into()]);
        assert_eq!(result, "run x y");
    }

    #[test]
    fn test_substitute_args_default() {
        let result = substitute_args("fix ${1:-main.rs}", &[] as &[String]);
        assert_eq!(result, "fix main.rs");
    }

    #[test]
    fn test_substitute_args_default_override() {
        let result = substitute_args("fix ${1:-main.rs}", &["lib.rs".into()]);
        assert_eq!(result, "fix lib.rs");
    }

    #[test]
    fn test_substitute_args_slice() {
        let result = substitute_args("run ${@:2}", &["a".into(), "b".into(), "c".into()]);
        assert_eq!(result, "run b c");
    }

    #[test]
    fn test_substitute_args_slice_with_length() {
        let result = substitute_args(
            "run ${@:2:2}",
            &["a".into(), "b".into(), "c".into(), "d".into()],
        );
        assert_eq!(result, "run b c");
    }

    #[test]
    fn test_substitute_args_missing_positional() {
        let result = substitute_args("fix $1 and $2", &["only".into()]);
        assert_eq!(result, "fix only and ");
    }

    #[test]
    fn test_expand_prompt_template_found() {
        let t = PromptTemplate {
            name: "fix".into(),
            description: "Fix".into(),
            argument_hint: None,
            content: "Fix $1".to_string(),
            file_path: PathBuf::from("/tmp/fix.md"),
        };
        let result = expand_prompt_template("/fix src/main.rs", &[t]);
        assert_eq!(result, "Fix src/main.rs");
    }

    #[test]
    fn test_expand_prompt_template_not_found() {
        let templates = [PromptTemplate {
            name: "fix".into(),
            description: "Fix".into(),
            argument_hint: None,
            content: "Fix $1".into(),
            file_path: PathBuf::from("/tmp/fix.md"),
        }];
        let result = expand_prompt_template("/other args", &templates);
        assert_eq!(result, "/other args");
    }

    #[test]
    fn test_expand_prompt_template_no_match_falls_through() {
        let templates: Vec<PromptTemplate> = vec![];
        let result = expand_prompt_template("/unknown", &templates);
        assert_eq!(result, "/unknown");
    }

    #[test]
    fn test_expand_prompt_template_no_slash() {
        let result = expand_prompt_template("not a template", &[]);
        assert_eq!(result, "not a template");
    }

    #[test]
    fn test_description_from_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.md");
        fs::write(
            &path,
            "---\ndescription: Custom description\n---\n\nBody here",
        )
        .unwrap();
        let templates = load_prompt_templates(&[tmp.path()]);
        assert_eq!(templates[0].description, "Custom description");
    }

    #[test]
    fn test_description_from_first_line() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.md");
        fs::write(&path, "First line of body\n\nSecond line").unwrap();
        let templates = load_prompt_templates(&[tmp.path()]);
        assert_eq!(templates[0].description, "First line of body");
    }

    #[test]
    fn test_duplicate_names_later_dir_wins() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        create_template(dir1.path(), "fix", "First version", "fix $1");
        create_template(dir2.path(), "fix", "Second version", "fix $1 and $2");

        let templates = load_prompt_templates(&[dir1.path(), dir2.path()]);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].description, "Second version");
    }

    #[test]
    fn test_argument_hint_from_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.md");
        fs::write(
            path,
            "---\ndescription: Test\nargument-hint: <file>\n---\n\nBody",
        )
        .unwrap();
        let templates = load_prompt_templates(&[tmp.path()]);
        assert_eq!(templates[0].argument_hint.as_deref(), Some("<file>"));
    }
}
