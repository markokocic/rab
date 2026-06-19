use rab::adapter;
use rab::agent::{self, AgentEvent, LoopConfig};
use rab::builtin::{
    bash::BashExtension, commands::CommandsExtension, edit::EditExtension, read::ReadExtension,
    write::WriteExtension,
};
use rab::extension::Extension;
use rab::session::SessionManager;
use rab::settings::Settings;
use std::io::Write;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let system_prompt = build_system_prompt(&extensions);
    let tools = rab::agent::collect_tool_defs(&extensions);
    let agent_tools: Vec<Box<dyn rab::extension::AgentTool>> =
        extensions.iter().flat_map(|ext| ext.tools()).collect();

    let thinking_level = settings.default_thinking_level.as_deref();
    let provider = adapter::GenaiProvider::new(&auth, thinking_level)?;

    if message_parts.is_empty() {
        let git_branch = get_git_branch(&cwd);
        let config = rab::tui::TuiConfig {
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
            hide_thinking: settings.hide_thinking.unwrap_or(false),
            collapse_tool_output: settings.collapse_tool_output.unwrap_or(false),
        };
        rab::tui::run(config, session).await
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
    tool_defs: Vec<rab::provider::ToolDef>,
    agent_tools: Vec<Box<dyn rab::extension::AgentTool>>,
    extensions: Vec<Box<dyn Extension>>,
    provider: adapter::GenaiProvider,
    history: Vec<rab::types::AgentMessage>,
    session: &mut SessionManager,
) -> anyhow::Result<()> {
    let loop_config = LoopConfig {
        model: model.clone(),
        system_prompt,
        tools: tool_defs,
        agent_tools: &agent_tools,
        extensions: &extensions,
    };

    let prompt = rab::types::AgentMessage::user(&message);

    // Persist the user prompt
    session.append_message(&prompt);

    let mut thinking_prefix_printed = false;
    let mut emitter = |event: AgentEvent| match event {
        AgentEvent::TextDelta { delta } => {
            print!("{}", delta);
            let _ = std::io::stdout().flush();
        }
        AgentEvent::ThinkingDelta { ref delta } => {
            if !thinking_prefix_printed {
                eprint!("{}", colored::Colorize::dimmed("… "));
                thinking_prefix_printed = true;
            }
            eprint!("{}", colored::Colorize::dimmed(delta.as_str()));
            let _ = std::io::stderr().flush();
        }
        AgentEvent::ToolCall {
            ref name, ref args, ..
        } => {
            eprintln!(
                "\n{} {} {}",
                colored::Colorize::dimmed("⚙"),
                colored::Colorize::bold(name.as_str()),
                colored::Colorize::dimmed(serde_json::to_string(args).unwrap_or_default().as_str())
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
        AgentEvent::AgentStart | AgentEvent::TurnStart | AgentEvent::TurnEnd => {}
        AgentEvent::AgentEnd { .. } => {
            eprintln!();
        }
    };

    let new_messages =
        agent::run_agent_loop(vec![prompt], history, &loop_config, &provider, &mut emitter).await?;

    // Persist all new assistant + tool result messages
    for msg in &new_messages {
        if msg.role != rab::types::Role::User {
            session.append_message(msg);
        }
    }

    if let Some(last_assistant) = new_messages
        .iter()
        .rev()
        .find(|m| m.role == rab::types::Role::Assistant)
        && !last_assistant.content.is_empty()
        && !last_assistant.content.ends_with('\n')
    {
        println!();
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

fn build_system_prompt(extensions: &[Box<dyn Extension>]) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are an expert coding assistant operating inside a terminal coding harness.\n",
    );
    prompt.push_str(
        "You help users by reading files, executing commands, editing code, and writing new files.\n\n",
    );

    prompt.push_str("Available tools:\n");
    for ext in extensions {
        for tool in ext.tools() {
            prompt.push_str(&format!("- {}: {}\n", tool.name(), tool.description()));
        }
    }

    prompt.push_str("\nGuidelines:\n");
    prompt.push_str("- Be concise in your responses\n");
    prompt.push_str("- Show file paths clearly when working with files\n");
    prompt.push_str("- Use the edit tool for precise changes with exact text matching\n");
    prompt.push_str("- When reading files, use offset/limit to handle large files\n");
    prompt.push_str(
        "- Always write complete files with the write tool, never use shell redirection\n",
    );

    prompt
}
