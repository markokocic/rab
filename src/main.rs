use rab::adapter;
use rab::agent::extension::Extension;
use rab::agent::session::SessionManager;
use rab::agent::settings::Settings;
use rab::agent::ui;
use rab::agent::{AgentEvent, LoopConfig};
use rab::builtin::{
    bash::BashExtension, commands::CommandsExtension, edit::EditExtension, read::ReadExtension,
    write::WriteExtension,
};
use std::io::Write;
use std::path::{Path, PathBuf};

use rab::tui::keybindings::{Keybindings, init_keybindings};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize freeze dump handler (SIGUSR1 → /tmp/rab-freeze.txt)
    rab::diag::init();

    let cwd = std::env::current_dir()?;

    // Parse CLI flags
    let args: Vec<String> = std::env::args().collect();
    let mut model_override: Option<String> = None;
    let mut message_parts: Vec<String> = Vec::new();
    let mut continue_session: bool = false;
    let mut session_path: Option<String> = None;
    let mut no_session: bool = false;
    let mut session_name: Option<String> = None;
    let mut session_dir_override: Option<String> = None;
    let mut no_context_files: bool = false;
    let mut system_prompt_override: Option<String> = None;
    let mut append_system_prompt_override: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                if i < args.len() {
                    model_override = Some(args[i].clone());
                }
            }
            "-c" | "--continue" => {
                continue_session = true;
            }
            "--session" => {
                i += 1;
                if i < args.len() {
                    session_path = Some(args[i].clone());
                }
            }
            "--no-session" => {
                no_session = true;
            }
            "--name" | "-n" => {
                i += 1;
                if i < args.len() {
                    session_name = Some(args[i].clone());
                }
            }
            "--no-context-files" | "-nc" => {
                no_context_files = true;
            }
            "--system-prompt" => {
                i += 1;
                if i < args.len() {
                    system_prompt_override = Some(args[i].clone());
                }
            }
            "--append-system-prompt" => {
                i += 1;
                if i < args.len() {
                    append_system_prompt_override = Some(args[i].clone());
                }
            }
            "--session-dir" => {
                i += 1;
                if i < args.len() {
                    session_dir_override = Some(args[i].clone());
                }
            }
            other if other.starts_with('-') => {
                // Ignore unknown flags for now
            }
            other => {
                message_parts.push(other.to_string());
            }
        }
        i += 1;
    }

    // Load settings and auth
    let settings = Settings::load(&cwd)?;
    let model = model_override.unwrap_or_else(|| settings.model().to_string());
    let auth = rab::auth::AuthStorage::load()?;

    // Load custom keybindings from ~/.rab/keybindings.json, merging with defaults
    let mut keybindings = Keybindings::with_defaults();
    if let Some(home) =
        directories::BaseDirs::new().map(|d| d.home_dir().join(".rab").join("keybindings.json"))
        && home.exists()
    {
        match Keybindings::load(&home) {
            Ok(custom) => keybindings.merge(custom),
            Err(e) => eprintln!("Warning: failed to load keybindings: {}", e),
        }
    }
    init_keybindings(keybindings);

    // Session management
    let session_dir = session_dir_override.map(std::path::PathBuf::from);
    let session = if no_session {
        SessionManager::in_memory(&cwd)
    } else if let Some(ref path) = session_path {
        let path = std::path::PathBuf::from(path);
        SessionManager::open(&path, session_dir.as_deref(), None)
    } else if continue_session {
        SessionManager::continue_recent(&cwd, session_dir.as_deref())
    } else {
        SessionManager::create(&cwd, session_dir.as_deref())
    };

    let mut session = session; // make mutable for appending

    // Set session name if provided
    if let Some(ref name) = session_name
        && !name.trim().is_empty()
    {
        session.append_session_info(name);
    }

    // Load history from session
    let context = session.build_session_context();
    let history = context.messages;

    // Available models
    let available_models = vec![
        "deepseek-v4-flash".to_string(),
        "deepseek-v4-pro".to_string(),
    ];

    // Build extensions with session info for /session command
    let commands_ext = CommandsExtension::new(available_models.clone());

    let extensions: Vec<Box<dyn Extension>> = vec![
        Box::new(commands_ext),
        Box::new(ReadExtension::new(cwd.clone())),
        Box::new(WriteExtension::new(cwd.clone())),
        Box::new(EditExtension::new(cwd.clone())),
        Box::new(BashExtension::new(cwd.clone())),
    ];

    let agent_dir = get_agent_dir();

    // Load context files (AGENTS.md / CLAUDE.md)
    let context_files = if no_context_files {
        Vec::new()
    } else {
        rab::agent::load_context_files(&cwd, &agent_dir)
    };

    // Load SYSTEM.md / APPEND_SYSTEM.md
    let custom_system_md = system_prompt_override.or_else(|| load_system_md(&cwd, &agent_dir));
    let append_system_md =
        append_system_prompt_override.or_else(|| load_append_system_md(&cwd, &agent_dir));

    // Collect context file display names (pi-style: relative to cwd when possible, ~/ for home)
    let context_file_names: Vec<String> = context_files
        .iter()
        .map(|cf| format_context_path(&cf.path, &cwd))
        .collect();

    // Build tool snippets from extensions
    let tool_snippets: Vec<rab::agent::ToolSnippet> = extensions
        .iter()
        .flat_map(|ext| ext.tools())
        .map(|tool| rab::agent::ToolSnippet {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
        })
        .collect();

    // Collect prompt guidelines from all tools (pi-style promptSnippet/promptGuidelines)
    let tool_guidelines: Vec<String> = extensions
        .iter()
        .flat_map(|ext| ext.tools())
        .flat_map(|tool| tool.prompt_guidelines())
        .collect();

    // Build system prompt using the new builder
    let system_prompt = rab::agent::SystemPromptBuilder::new()
        .tool_snippets(tool_snippets)
        .guidelines(tool_guidelines)
        .context_files(context_files)
        .custom_prompt(custom_system_md)
        .append_prompt(append_system_md)
        .cwd(&cwd)
        .build();

    let tools = rab::agent::collect_tool_defs(&extensions);
    let agent_tools: Vec<Box<dyn rab::agent::extension::AgentTool>> =
        extensions.iter().flat_map(|ext| ext.tools()).collect();

    // Load skills for startup display and /skill:name expansion
    let skills = rab::agent::load_skills(rab::agent::LoadSkillsOptions {
        cwd: &cwd,
        agent_dir: &agent_dir,
        extra_skill_paths: &[],
        include_defaults: true,
    });

    let thinking_level = settings.default_thinking_level.as_deref().or(Some("xhigh"));
    let provider = adapter::GenaiProvider::new(&auth, thinking_level)?;

    if message_parts.is_empty() {
        let git_branch = get_git_branch(&cwd);
        let config = ui::AppConfig {
            model,
            system_prompt,
            tools,
            agent_tools,
            extensions,
            provider: Box::new(provider),
            cwd,
            thinking_level: thinking_level.map(|s| s.to_string()),
            git_branch,
            available_models,
            hide_thinking: settings.hide_thinking.unwrap_or(true),
            collapse_tool_output: settings.collapse_tool_output.unwrap_or(true),
            interactive: true,
            settings,
            context_files: context_file_names,
            skills,
            model_supports_reasoning: true,
            tool_execution: rab::agent::ToolExecutionMode::Parallel,
        };
        ui::run(config, session).await
    } else {
        let message = message_parts.join(" ");
        run_print_mode(
            message,
            model,
            system_prompt,
            tools,
            agent_tools,
            extensions,
            provider,
            history,
            &mut session,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_print_mode(
    message: String,
    model: String,
    system_prompt: String,
    tool_defs: Vec<rab::agent::provider::ToolDef>,
    agent_tools: Vec<Box<dyn rab::agent::extension::AgentTool>>,
    extensions: Vec<Box<dyn Extension>>,
    provider: adapter::GenaiProvider,
    history: Vec<rab::agent::types::AgentMessage>,
    session: &mut SessionManager,
) -> anyhow::Result<()> {
    let loop_config = LoopConfig {
        model: model.clone(),
        system_prompt,
        tools: tool_defs,
        agent_tools: &agent_tools,
        extensions: &extensions,
        tool_execution: rab::agent::ToolExecutionMode::Parallel,
        steering_queue: None,
        follow_up_queue: None,
        transform_context: None,
        prepare_next_turn: None,
        should_stop_after_turn: None,
    };

    let prompt = rab::agent::types::AgentMessage::user(&message);

    // Persist the user prompt
    session.append_message(&prompt);

    let mut thinking_prefix_printed = false;
    let mut emitter = |event: AgentEvent| {
        match event {
            AgentEvent::TextDelta { delta } => {
                // Normalize markdown headings in each delta to prevent progressive
                // indentation (indented headings parsed as nested inside lists).
                let normalized =
                    rab::tui::components::markdown::normalize_markdown_headings(&delta);
                print!("{}", normalized);
                let _ = std::io::stdout().flush();
            }
            AgentEvent::ThinkingDelta { ref delta } => {
                if !thinking_prefix_printed {
                    eprint!("{}", colored::Colorize::dimmed("… "));
                    thinking_prefix_printed = true;
                }
                // Normalize headings in thinking output too
                let normalized = rab::tui::components::markdown::normalize_markdown_headings(delta);
                eprint!("{}", colored::Colorize::dimmed(&*normalized));
                let _ = std::io::stderr().flush();
            }
            AgentEvent::ToolCall {
                ref name, ref args, ..
            } => {
                eprintln!(
                    "\n{} {} {}",
                    colored::Colorize::dimmed("⚙"),
                    colored::Colorize::bold(name.as_str()),
                    colored::Colorize::dimmed(
                        serde_json::to_string(args).unwrap_or_default().as_str()
                    )
                );
                thinking_prefix_printed = false;
            }
            AgentEvent::ToolResult {
                ref content,
                is_error,
                ..
            } => {
                if is_error {
                    eprintln!(
                        "{} {}",
                        colored::Colorize::red("✗"),
                        colored::Colorize::red(content.as_str())
                    );
                } else {
                    let truncated: String = content.chars().take(500).collect();
                    eprintln!(
                        "{} {}",
                        colored::Colorize::dimmed("✓"),
                        colored::Colorize::dimmed(truncated.as_str())
                    );
                    if content.len() > 500 {
                        eprintln!("{}", colored::Colorize::dimmed("... (truncated)"));
                    }
                }
            }
            AgentEvent::ToolProgress { ref content, .. } => {
                // Stream output is printed as it arrives
                print!("{}", content);
                let _ = std::io::stdout().flush();
            }
            AgentEvent::AgentStart | AgentEvent::TurnStart | AgentEvent::TurnEnd => {}
            AgentEvent::ToolCallArgsUpdate { .. } => {
                // Progressive args update - no-op in print mode
            }
            AgentEvent::UserMessage { ref content } => {
                // In print mode, show injected queue messages
                eprintln!(
                    "{} {}",
                    colored::Colorize::dimmed("→"),
                    colored::Colorize::dimmed(content.as_str())
                );
            }
            AgentEvent::Aborted { ref reason } => {
                eprintln!(
                    "{} {}",
                    colored::Colorize::red("✗"),
                    colored::Colorize::red(reason.as_str())
                );
            }
            AgentEvent::AgentEnd { .. } => {
                eprintln!();
            }
        }
    };

    let new_messages =
        rab::agent::run_agent_loop(vec![prompt], history, &loop_config, &provider, &mut emitter)
            .await?;

    // Persist all new assistant + tool result messages
    for msg in &new_messages {
        if msg.role != rab::agent::types::Role::User {
            session.append_message(msg);
        }
    }

    // Pi-style: explicitly check the last assistant message after the loop completes.
    // This handles errors that may not have been fully visible during streaming,
    // and ensures proper exit code on failure.
    if let Some(last_assistant) = new_messages
        .iter()
        .rev()
        .find(|m| m.role == rab::agent::types::Role::Assistant)
    {
        if last_assistant.is_error {
            eprintln!(
                "{} {}",
                colored::Colorize::red("✗"),
                colored::Colorize::red(last_assistant.content.as_str())
            );
            // Still return Ok — the error message is in the session.
            // Caller can check last message is_error if needed.
        } else if !last_assistant.content.is_empty() && !last_assistant.content.ends_with('\n') {
            println!();
        }
    }

    Ok(())
}

fn get_git_branch(cwd: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }
    None
}

/// Get the agent config directory (~/.rab/agent).
fn get_agent_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".rab").join("agent"))
        .unwrap_or_else(|| PathBuf::from("/tmp/.rab/agent"))
}

/// Load SYSTEM.md: project `.rab/SYSTEM.md` first, then global `~/.rab/agent/SYSTEM.md`.
fn load_system_md(cwd: &Path, agent_dir: &Path) -> Option<String> {
    // Project-local takes precedence
    let project_path = cwd.join(".rab").join("SYSTEM.md");
    if project_path.exists() {
        return std::fs::read_to_string(&project_path).ok();
    }
    // Global fallback
    let global_path = agent_dir.join("SYSTEM.md");
    if global_path.exists() {
        return std::fs::read_to_string(&global_path).ok();
    }
    None
}

/// Load APPEND_SYSTEM.md: project `.rab/APPEND_SYSTEM.md` first, then global.
fn load_append_system_md(cwd: &Path, agent_dir: &Path) -> Option<String> {
    // Project-local takes precedence
    let project_path = cwd.join(".rab").join("APPEND_SYSTEM.md");
    if project_path.exists() {
        return std::fs::read_to_string(&project_path).ok();
    }
    // Global fallback
    let global_path = agent_dir.join("APPEND_SYSTEM.md");
    if global_path.exists() {
        return std::fs::read_to_string(&global_path).ok();
    }
    None
}

/// Format a context file path for display, pi-style:
/// - Show path relative to cwd if under cwd
/// - Otherwise replace home directory with `~/`
fn format_context_path(path: &Path, cwd: &Path) -> String {
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
