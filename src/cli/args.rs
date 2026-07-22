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
    // Try parent of cwd (for subdirectory cases)
    if let Some(parent) = cwd.parent()
        && let Ok(rel) = path.strip_prefix(parent)
    {
        return "..".to_string() + std::path::MAIN_SEPARATOR_STR + &rel.to_string_lossy();
    }
    // Replace home dir with ~/
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return "~/".to_string() + &rel.to_string_lossy();
    }
    // Fallback: absolute path
    path.to_string_lossy().to_string()
}
