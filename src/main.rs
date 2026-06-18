use rab::adapter;
use rab::agent::{self, AgentEvent, LoopConfig};
use rab::builtin::{
    bash::BashExtension, edit::EditExtension, read::ReadExtension, write::WriteExtension,
};
use rab::extension::Extension;
use rab::settings::Settings;
use std::io::Write;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // Parse CLI: rab [--model <model>] [<message>]
    // No message → interactive TUI. Message → print mode.
    let args: Vec<String> = std::env::args().collect();
    let mut model_override: Option<String> = None;
    let mut message_parts: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                if i < args.len() {
                    model_override = Some(args[i].clone());
                }
            }
            other => {
                message_parts.push(other.to_string());
            }
        }
        i += 1;
    }

    // Load settings
    let settings = Settings::load(&cwd)?;
    let model = model_override.unwrap_or_else(|| settings.model().to_string());

    // Load auth
    let auth = rab::auth::AuthStorage::load()?;

    // Build extensions
    let extensions: Vec<Box<dyn Extension>> = vec![
        Box::new(ReadExtension::new(cwd.clone())),
        Box::new(WriteExtension::new(cwd.clone())),
        Box::new(EditExtension::new(cwd.clone())),
        Box::new(BashExtension::new(cwd.clone())),
    ];

    // Build system prompt
    let system_prompt = build_system_prompt(&extensions);

    // Build tool defs
    let tools = rab::agent::collect_tool_defs(&extensions);
    let agent_tools: Vec<Box<dyn rab::extension::AgentTool>> =
        extensions.iter().flat_map(|ext| ext.tools()).collect();

    // Create provider
    let thinking_level = settings.default_thinking_level.as_deref();
    let provider = adapter::GenaiProvider::new(&auth, thinking_level)?;

    if message_parts.is_empty() {
        run_interactive(
            cwd,
            model,
            system_prompt,
            tools,
            agent_tools,
            extensions,
            provider,
        )
        .await
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
        )
        .await
    }
}

async fn run_print_mode(
    message: String,
    model: String,
    system_prompt: String,
    tool_defs: Vec<rab::provider::ToolDef>,
    agent_tools: Vec<Box<dyn rab::extension::AgentTool>>,
    extensions: Vec<Box<dyn Extension>>,
    provider: adapter::GenaiProvider,
) -> anyhow::Result<()> {
    let loop_config = LoopConfig {
        model: model.clone(),
        system_prompt,
        tools: tool_defs,
        agent_tools: &agent_tools,
        extensions: &extensions,
    };

    let prompt = rab::types::AgentMessage::user(&message);

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
        agent::run_agent_loop(vec![prompt], &loop_config, &provider, &mut emitter).await?;

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

async fn run_interactive(
    cwd: std::path::PathBuf,
    model: String,
    system_prompt: String,
    tools: Vec<rab::provider::ToolDef>,
    agent_tools: Vec<Box<dyn rab::extension::AgentTool>>,
    extensions: Vec<Box<dyn Extension>>,
    provider: adapter::GenaiProvider,
) -> anyhow::Result<()> {
    let config = rab::tui::TuiConfig {
        model,
        system_prompt,
        tools,
        agent_tools,
        extensions,
        provider: Box::new(provider),
        cwd,
    };
    rab::tui::run(config).await
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
