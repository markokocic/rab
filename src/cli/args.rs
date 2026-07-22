//! CLI argument parsing and helper functions for rab.
//!
//! Matches pi's `packages/coding-agent/src/cli/args.ts` for argument parsing
//! and `packages/coding-agent/src/core/agent-session-runtime.ts` for
//! SYSTEM.md / APPEND_SYSTEM.md loading.

use std::path::{Path, PathBuf};

/// Parsed CLI arguments.
#[derive(Debug, Default)]
pub struct CliArgs {
    pub model_override: Option<String>,
    pub message_parts: Vec<String>,
    pub continue_session: bool,
    pub resume_session: bool,
    pub session_path: Option<String>,
    pub session_id: Option<String>,
    pub fork_source: Option<String>,
    pub export_path: Option<String>,
    pub no_session: bool,
    pub session_name: Option<String>,
    pub session_dir_override: Option<String>,
    pub no_context_files: bool,
    pub system_prompt_override: Option<String>,
    pub append_system_prompt_override: Option<String>,
}

/// Parse CLI flags from raw argument strings.
pub fn parse_args(args: &[String]) -> CliArgs {
    let mut result = CliArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                if i < args.len() {
                    result.model_override = Some(args[i].clone());
                }
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--version" | "-v" => {
                println!("rab {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-c" | "--continue" => {
                result.continue_session = true;
            }
            "-r" | "--resume" => {
                result.resume_session = true;
            }
            "--session" => {
                i += 1;
                if i < args.len() {
                    result.session_path = Some(args[i].clone());
                }
            }
            "--session-id" => {
                i += 1;
                if i < args.len() {
                    result.session_id = Some(args[i].clone());
                }
            }
            "--fork" => {
                i += 1;
                if i < args.len() {
                    result.fork_source = Some(args[i].clone());
                }
            }
            "--export" => {
                i += 1;
                if i < args.len() {
                    result.export_path = Some(args[i].clone());
                }
            }
            "--no-session" => {
                result.no_session = true;
            }
            "--name" | "-n" => {
                i += 1;
                if i < args.len() {
                    result.session_name = Some(args[i].clone());
                }
            }
            "--no-context-files" | "-nc" => {
                result.no_context_files = true;
            }
            "--system-prompt" => {
                i += 1;
                if i < args.len() {
                    result.system_prompt_override = Some(args[i].clone());
                }
            }
            "--append-system-prompt" => {
                i += 1;
                if i < args.len() {
                    result.append_system_prompt_override = Some(args[i].clone());
                }
            }
            "--session-dir" => {
                i += 1;
                if i < args.len() {
                    result.session_dir_override = Some(args[i].clone());
                }
            }
            other if other.starts_with('-') => {
                // Ignore unknown flags for now
            }
            other => {
                result.message_parts.push(other.to_string());
            }
        }
        i += 1;
    }
    result
}

/// Print usage information and supported flags.
pub fn print_help() {
    let help = r#"rab — a lightweight, extensible, Rust-based coding agent.

Usage:
  rab [options] [<message>...]
  rab generate-models

Options:
  --model <model>              Model override (e.g. "anthropic/claude-sonnet-4-20250514")
  -c, --continue               Continue most recent session
  -r, --resume                 Select a session to resume
  --session <path|id>          Use specific session file or partial UUID
  --session-id <id>            Use exact project session ID, creating it if missing
  --fork <path|id>             Fork session into a new session
  --export <file>              Export session to HTML (not yet implemented)
  --no-session                 Don't save session (ephemeral)
  -n, --name <name>            Set session display name
  --no-context-files, -nc      Disable AGENTS.md / CLAUDE.md discovery
  --system-prompt <text>       Override system prompt
  --append-system-prompt <text> Append text to system prompt
  --session-dir <dir>          Session storage directory
  -h, --help                   Show this help
  -v, --version                Show version

Examples:
  rab                           Interactive mode
  rab "List all .rs files"      Interactive mode with initial prompt
  rab -c "What did we discuss?" Continue previous session
  rab -n "Refactor" "Fix this"  Named session with messages
  rab generate-models           Generate model definitions
"#;
    print!("{help}");
}

/// Get the agent config directory (~/.rab/agent).
pub fn get_agent_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".rab").join("agent"))
        .unwrap_or_else(|| PathBuf::from("/tmp/.rab/agent"))
}

/// Load SYSTEM.md: project `.rab/SYSTEM.md` first, then global `~/.rab/agent/SYSTEM.md`.
pub fn load_system_md(cwd: &Path, agent_dir: &Path) -> Option<String> {
    let project_path = cwd.join(".rab").join("SYSTEM.md");
    if project_path.exists() {
        return std::fs::read_to_string(&project_path).ok();
    }
    let global_path = agent_dir.join("SYSTEM.md");
    if global_path.exists() {
        return std::fs::read_to_string(&global_path).ok();
    }
    None
}

/// Load APPEND_SYSTEM.md: project `.rab/APPEND_SYSTEM.md` first, then global.
pub fn load_append_system_md(cwd: &Path, agent_dir: &Path) -> Option<String> {
    let project_path = cwd.join(".rab").join("APPEND_SYSTEM.md");
    if project_path.exists() {
        return std::fs::read_to_string(&project_path).ok();
    }
    let global_path = agent_dir.join("APPEND_SYSTEM.md");
    if global_path.exists() {
        return std::fs::read_to_string(&global_path).ok();
    }
    None
}

/// Format a context file path for display, pi-style:
/// - Show path relative to cwd if under cwd
/// - Otherwise replace home directory with `~/`
pub fn format_context_path(path: &Path, cwd: &Path) -> String {
    // Try relative to cwd first
    if let Ok(rel) = path.strip_prefix(cwd) {
        return rel.to_string_lossy().to_string();
    }
    // Replace home dir with ~/
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return "~/".to_string() + &rel.to_string_lossy();
    }
    // Try parent of cwd (for subdirectory cases)
    if let Some(parent) = cwd.parent()
        && let Ok(rel) = path.strip_prefix(parent)
    {
        return "..".to_string() + std::path::MAIN_SEPARATOR_STR + &rel.to_string_lossy();
    }
    // Fallback: absolute path
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_empty() {
        let args = parse_args(&[]);
        assert!(args.message_parts.is_empty());
        assert!(!args.continue_session);
        assert!(args.model_override.is_none());
    }

    #[test]
    fn parse_args_model_override() {
        let args = parse_args(&["--model".into(), "anthropic/claude-4".into()]);
        assert_eq!(args.model_override.unwrap(), "anthropic/claude-4");
    }

    #[test]
    fn parse_args_continue_flags() {
        let args = parse_args(&["-c".into()]);
        assert!(args.continue_session);

        let args = parse_args(&["--continue".into()]);
        assert!(args.continue_session);

        let args = parse_args(&["-r".into()]);
        assert!(args.resume_session);

        let args = parse_args(&["--resume".into()]);
        assert!(args.resume_session);
    }

    #[test]
    fn parse_args_session_path() {
        let args = parse_args(&["--session".into(), "/tmp/mysession.jsonl".into()]);
        assert_eq!(args.session_path.unwrap(), "/tmp/mysession.jsonl");
    }

    #[test]
    fn parse_args_session_id() {
        let args = parse_args(&["--session-id".into(), "abc-123".into()]);
        assert_eq!(args.session_id.unwrap(), "abc-123");
    }

    #[test]
    fn parse_args_fork() {
        let args = parse_args(&["--fork".into(), "abc-123".into()]);
        assert_eq!(args.fork_source.unwrap(), "abc-123");
    }

    #[test]
    fn parse_args_no_session() {
        let args = parse_args(&["--no-session".into()]);
        assert!(args.no_session);
    }

    #[test]
    fn parse_args_name() {
        let args = parse_args(&["-n".into(), "my-session".into()]);
        assert_eq!(args.session_name.unwrap(), "my-session");

        let args = parse_args(&["--name".into(), "other".into()]);
        assert_eq!(args.session_name.unwrap(), "other");
    }

    #[test]
    fn parse_args_no_context_files() {
        let args = parse_args(&["--no-context-files".into()]);
        assert!(args.no_context_files);

        let args = parse_args(&["-nc".into()]);
        assert!(args.no_context_files);
    }

    #[test]
    fn parse_args_system_prompt() {
        let args = parse_args(&["--system-prompt".into(), "be concise".into()]);
        assert_eq!(args.system_prompt_override.unwrap(), "be concise");
    }

    #[test]
    fn parse_args_append_system_prompt() {
        let args = parse_args(&["--append-system-prompt".into(), "extra instructions".into()]);
        assert_eq!(
            args.append_system_prompt_override.unwrap(),
            "extra instructions"
        );
    }

    #[test]
    fn parse_args_session_dir() {
        let args = parse_args(&["--session-dir".into(), "/tmp/sessions".into()]);
        assert_eq!(args.session_dir_override.unwrap(), "/tmp/sessions");
    }

    #[test]
    fn parse_args_message_parts() {
        let args = parse_args(&["hello".into(), "world".into()]);
        assert_eq!(args.message_parts, vec!["hello", "world"]);
    }

    #[test]
    fn parse_args_mixed_flags_and_message() {
        let args = parse_args(&[
            "--model".into(),
            "claude-4".into(),
            "list".into(),
            "files".into(),
        ]);
        assert_eq!(args.model_override.unwrap(), "claude-4");
        assert_eq!(args.message_parts, vec!["list", "files"]);
    }

    #[test]
    fn parse_args_unknown_flags_ignored() {
        let args = parse_args(&["--unknown-flag".into(), "value".into(), "hello".into()]);
        // --unknown-flag is consumed but value becomes a message part
        // because parse_args only recognizes known flags
        assert_eq!(args.message_parts, vec!["value", "hello"]);
    }

    #[test]
    fn format_context_path_relative_to_cwd() {
        let path = Path::new("/project/sub/AGENTS.md");
        let cwd = Path::new("/project");
        let result = format_context_path(path, cwd);
        assert_eq!(result, "sub/AGENTS.md");
    }

    #[test]
    fn format_context_path_absolute_fallback() {
        let path = Path::new("/some/other/path");
        let cwd = Path::new("/project");
        let result = format_context_path(path, cwd);
        // Falls through to parent check: cwd.parent() = "/", path starts with "/"
        assert!(result.ends_with("some/other/path"));
    }

    #[test]
    fn format_context_path_with_parent() {
        let path = Path::new("/project/sibling/file.md");
        let cwd = Path::new("/project/sub");
        let result = format_context_path(path, cwd);
        assert_eq!(result, "../sibling/file.md");
    }

    #[test]
    fn get_agent_dir_returns_something() {
        let dir = get_agent_dir();
        assert!(dir.ends_with(".rab/agent"));
    }
}
