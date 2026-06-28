use rab::agent::extension::Extension;
use rab::agent::settings::Settings;
use rab::agent::ui;
use rab::builtin::{
    bash::BashExtension, commands::CommandsExtension, edit::EditExtension, read::ReadExtension,
    write::WriteExtension,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use yoagent::types::AgentTool as _;

use rab::tui::keybindings::{Keybindings, init_keybindings};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // Parse CLI flags
    let args: Vec<String> = std::env::args().collect();
    let mut model_override: Option<String> = None;
    let mut message_parts: Vec<String> = Vec::new();
    let mut continue_session: bool = false;
    let mut resume_session: bool = false;
    let mut session_path: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut fork_source: Option<String> = None;
    let mut export_path: Option<String> = None;
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
            "-r" | "--resume" => {
                resume_session = true;
            }
            "--session" => {
                i += 1;
                if i < args.len() {
                    session_path = Some(args[i].clone());
                }
            }
            "--session-id" => {
                i += 1;
                if i < args.len() {
                    session_id = Some(args[i].clone());
                }
            }
            "--fork" => {
                i += 1;
                if i < args.len() {
                    fork_source = Some(args[i].clone());
                }
            }
            "--export" => {
                i += 1;
                if i < args.len() {
                    export_path = Some(args[i].clone());
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

    // Validate flag conflicts (pi-compatible)
    let conflicting_flags: Vec<&str> = [
        (fork_source.is_some(), "--fork"),
        (continue_session, "--continue"),
        (resume_session, "--resume"),
        (no_session, "--no-session"),
    ]
    .into_iter()
    .filter_map(|(cond, name)| if cond { Some(name) } else { None })
    .collect();

    if fork_source.is_some() && conflicting_flags.len() > 1 {
        for f in &conflicting_flags[1..] {
            eprintln!("Error: --fork cannot be combined with {}", f);
        }
        std::process::exit(1);
    }

    if session_id.is_some() {
        let mut conflicting: Vec<&str> = Vec::new();
        if session_path.is_some() {
            conflicting.push("--session");
        }
        if continue_session {
            conflicting.push("--continue");
        }
        if resume_session {
            conflicting.push("--resume");
        }
        if no_session {
            conflicting.push("--no-session");
        }
        if !conflicting.is_empty() {
            eprintln!(
                "Error: --session-id cannot be combined with {}",
                conflicting.join(", ")
            );
            std::process::exit(1);
        }
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

    let session_dir = session_dir_override.as_ref().map(std::path::PathBuf::from);

    // Resolve session arg (path or partial ID) for --session and --fork
    fn resolve_session_arg(
        arg: &str,
        cwd: &std::path::Path,
        session_dir: Option<&std::path::Path>,
    ) -> Result<ResolvedSession, String> {
        // If it looks like a path (contains separator or ends with .jsonl), use as-is
        if arg.contains('/') || arg.contains('\\') || arg.ends_with(".jsonl") {
            let path = std::path::PathBuf::from(arg);
            if path.is_absolute() {
                return Ok(ResolvedSession::Path(path));
            }
            return Ok(ResolvedSession::Path(cwd.join(&path)));
        }

        // Try to match as session ID prefix (first exact, then prefix)
        let sessions = rab::agent::session::SessionManager::list_all(session_dir);

        // Exact match first
        if let Some(s) = sessions.iter().find(|s| s.id == arg) {
            return Ok(ResolvedSession::Found {
                path: s.path.clone(),
                cwd: s.cwd.clone(),
            });
        }

        // Prefix match
        let matches: Vec<_> = sessions.iter().filter(|s| s.id.starts_with(arg)).collect();
        if matches.len() == 1 {
            return Ok(ResolvedSession::Found {
                path: matches[0].path.clone(),
                cwd: matches[0].cwd.clone(),
            });
        }

        Err(format!("No session found matching '{}'", arg))
    }

    enum ResolvedSession {
        Path(std::path::PathBuf),
        Found {
            path: std::path::PathBuf,
            cwd: String,
        },
    }

    impl ResolvedSession {
        fn path(&self) -> &std::path::Path {
            match self {
                ResolvedSession::Path(p) => p.as_path(),
                ResolvedSession::Found { path, .. } => path.as_path(),
            }
        }

        fn cwd(&self) -> Option<&str> {
            match self {
                ResolvedSession::Path(_) => None,
                ResolvedSession::Found { cwd, .. } => Some(cwd.as_str()),
            }
        }
    }

    // Handle --export: export session and exit
    if let Some(ref _export_dest) = export_path {
        eprintln!("Export to HTML is not yet implemented. See --export in pi.");
        std::process::exit(1);
    }

    // Build session manager
    let session = if let Some(ref fork_arg) = fork_source {
        // Pi-compatible fork: resolve arg, then fork
        let resolved = match resolve_session_arg(fork_arg, &cwd, session_dir.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };
        // Check for session-id conflict: if --session-id is also set, validate it's not taken
        if let Some(ref sid) = session_id {
            let sessions_dir = session_dir
                .clone()
                .unwrap_or_else(|| rab::agent::session::get_default_session_dir(&cwd));
            let sessions = rab::agent::session::list_sessions(&sessions_dir);
            if sessions.iter().any(|s| s.id == *sid) {
                eprintln!("Session already exists with id '{}'", sid);
                std::process::exit(1);
            }
        }
        let fork_options = session_id
            .as_ref()
            .map(|id| rab::agent::session::NewSessionOptions {
                id: Some(id.clone()),
                parent_session: None,
            });
        match rab::agent::session::SessionManager::fork_from(
            resolved.path(),
            &cwd,
            session_dir.as_deref(),
            fork_options.as_ref(),
        ) {
            Ok(sm) => {
                eprintln!("Forked session {}", sm.session_id());
                rab::agent::AgentSession::new(sm)
            }
            Err(e) => {
                eprintln!("Error: fork failed: {}", e);
                std::process::exit(1);
            }
        }
    } else if no_session {
        rab::agent::AgentSession::in_memory(&cwd)
    } else if let Some(ref path_or_id) = session_path {
        // Pi-compatible: resolve path or partial UUID
        match resolve_session_arg(path_or_id, &cwd, session_dir.as_deref()) {
            Ok(resolved) => {
                // Check if this session is from a different project (cross-project fork)
                if let Some(session_cwd) = resolved.cwd() {
                    let resolved_cwd = std::path::Path::new(session_cwd);
                    if resolved_cwd != cwd {
                        eprintln!("Warning: session from different project: {}", session_cwd);
                        eprintln!("Use --fork to fork it into the current directory.");
                    }
                }
                let path = resolved.path().to_path_buf();
                rab::agent::AgentSession::open(&path, session_dir.as_deref(), None)
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else if resume_session {
        // Pi-compatible: --resume opens interactive session picker
        // For now, fall back to continue_recent
        rab::agent::AgentSession::continue_recent(&cwd, session_dir.as_deref())
    } else if continue_session {
        rab::agent::AgentSession::continue_recent(&cwd, session_dir.as_deref())
    } else if let Some(ref sid) = session_id {
        // Use explicit session ID, creating it if missing
        let sessions_dir = session_dir
            .clone()
            .unwrap_or_else(|| rab::agent::session::get_default_session_dir(&cwd));
        let sessions = rab::agent::session::list_sessions(&sessions_dir);
        let existing = sessions.iter().find(|s| s.id == *sid);
        if let Some(s) = existing {
            rab::agent::AgentSession::open(&s.path, session_dir.as_deref(), None)
        } else {
            rab::agent::AgentSession::new(rab::agent::session::SessionManager::create_with_options(
                &cwd,
                session_dir.as_deref(),
                Some(&rab::agent::session::NewSessionOptions {
                    id: Some(sid.clone()),
                    parent_session: None,
                }),
            ))
        }
    } else {
        rab::agent::AgentSession::create(&cwd, session_dir.as_deref())
    };

    let mut session = session; // make mutable for appending

    // Set session name if provided
    if let Some(ref name) = session_name
        && !name.trim().is_empty()
    {
        session.session_mut().append_session_info(name);
    }

    // Load history from session
    let context = session.session().build_session_context();

    // Available models
    let available_models = vec![
        "deepseek-v4-flash".to_string(),
        "deepseek-v4-pro".to_string(),
    ];

    // Build extensions with session info for /session command
    let commands_ext = CommandsExtension::new(available_models.clone());
    let session_info = commands_ext.session_info.clone();

    // Conditionally build extensions based on settings.
    // New tools (grep, find, ls) are disabled by default.
    // Enable them by adding to settings.tools, e.g.:
    //   "tools": ["grep", "find", "ls"]
    // Or use settings.exclude_tools to disable specific core tools.
    fn is_extension_active(name: &str, settings: &Settings) -> bool {
        // exclude_tools always wins
        if settings.exclude_tools.iter().any(|t| t == name) {
            return false;
        }

        let core_extensions: &[&str] = &["commands", "read", "write", "edit", "bash", "mcp"];

        // If tools whitelist is set, only those are active
        if !settings.tools.is_empty() {
            return settings.tools.iter().any(|t| t == name);
        }

        // Core extensions are always active when no whitelist
        core_extensions.contains(&name)
    }

    let mut extensions: Vec<Box<dyn Extension>> = Vec::new();

    if is_extension_active("commands", &settings) {
        extensions.push(Box::new(commands_ext));
    }
    if is_extension_active("read", &settings) {
        extensions.push(Box::new(ReadExtension::new(cwd.clone())));
    }
    if is_extension_active("write", &settings) {
        extensions.push(Box::new(WriteExtension::new(cwd.clone())));
    }
    if is_extension_active("edit", &settings) {
        extensions.push(Box::new(EditExtension::new(cwd.clone())));
    }
    if is_extension_active("bash", &settings) {
        extensions.push(Box::new(BashExtension::new(cwd.clone())));
    }
    if is_extension_active("grep", &settings)
        || is_extension_active("find", &settings)
        || is_extension_active("ls", &settings)
    {
        extensions.push(Box::new(
            rab::extensions::filesystem::FilesystemExtension::new(cwd.clone()),
        ));
    }
    if is_extension_active("mcp", &settings) {
        let mcp_ext = rab::extensions::mcp::McpExtension::from_cwd(&cwd);
        mcp_ext.restore_cache().await;
        // Bootstrap servers with directTools configured so their tools are
        // available as native AgentTools from the start.
        mcp_ext.bootstrap_direct_tools().await;
        extensions.push(Box::new(mcp_ext));
    }

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

    // Collect tools + metadata from all extensions
    let all_tools: Vec<rab::agent::extension::ToolDefinition> =
        extensions.iter().flat_map(|ext| ext.tools()).collect();

    // Build tool snippets and guidelines from ToolDefinition metadata
    let tool_snippets: Vec<rab::agent::ToolSnippet> = all_tools
        .iter()
        .map(|twm| rab::agent::ToolSnippet {
            name: twm.name().to_string(),
            description: twm.snippet.to_string(),
        })
        .collect();

    let tool_guidelines: Vec<String> = all_tools
        .iter()
        .flat_map(|twm| twm.guidelines.iter().copied())
        .map(|s| s.to_string())
        .collect();

    // ToolDefinition IS an AgentTool now — no unwrapping needed
    let agent_tools: Vec<Box<dyn yoagent::types::AgentTool>> = all_tools
        .into_iter()
        .map(|twm| Box::new(twm) as Box<dyn yoagent::types::AgentTool>)
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

    // Load skills for startup display and /skill:name expansion
    let mut skill_dirs = Vec::new();
    skill_dirs.push(agent_dir.join("skills"));
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
        skill_dirs.push(home.join(".agents").join("skills"));
    }
    let mut current = Some(cwd.to_path_buf());
    while let Some(dir) = current {
        skill_dirs.push(dir.join(".rab").join("skills"));
        skill_dirs.push(dir.join(".agents").join("skills"));
        let parent = match dir.parent() {
            Some(p) if p != dir => p.to_path_buf(),
            _ => break,
        };
        current = Some(parent);
    }
    let mut skill_set = yoagent::skills::SkillSet::load(&skill_dirs).unwrap_or_default();
    // Merge skills from extensions
    for ext in &extensions {
        skill_set.merge(ext.skills());
    }
    let skills: Vec<yoagent::skills::Skill> = skill_set.skills().to_vec();

    // Determine initial thinking level: prefer session's recorded level, fall back to settings.
    // Pi-compatible: if the session has thinking level change entries, use the resolved level
    // from the current path. Otherwise fall back to settings default.
    let has_thinking_entries = !session
        .session()
        .find_entries("thinking_level_change")
        .is_empty();
    let thinking_level = if has_thinking_entries {
        Some(context.thinking_level.clone())
    } else {
        settings.default_thinking_level.clone()
    };
    let thinking_level_str = thinking_level.as_deref().or(Some("xhigh"));

    if message_parts.is_empty() {
        let config = ui::AppConfig {
            model,
            system_prompt,
            extensions,
            cwd,
            thinking_level: thinking_level_str.map(|s| s.to_string()),
            available_models,
            hide_thinking: settings.hide_thinking.unwrap_or(true),
            collapse_tool_output: settings.collapse_tool_output.unwrap_or(true),
            interactive: true,
            settings,
            context_files: context_file_names,
            skills,
            model_supports_reasoning: true,
            session_info: Some(session_info),
            api_key: auth.api_key("opencode-go").unwrap_or_default(),
        };
        ui::run(config, session).await
    } else {
        let message = message_parts.join(" ");
        let mut agent_session = session;
        let api_key = auth.api_key("opencode-go").unwrap_or_default();
        let mut mc = yoagent::provider::model::ModelConfig::openai_compat(
            "https://opencode.ai/zen/go/v1",
            &model,
            "opencode-go",
            yoagent::provider::model::OpenAiCompat::deepseek(),
        );
        mc.context_window = rab::agent::compaction::get_model_context_window(&model) as u32;
        agent_session.set_compaction_config(
            api_key.clone(),
            &model,
            rab::agent::compaction::get_model_context_window(&model),
            Some(mc),
        );

        // Populate session info for /session command
        let si = rab::builtin::commands::compute_session_info(agent_session.session());
        if let Ok(mut guard) = session_info.lock() {
            *guard = Some(si);
        }

        // Get API key for yoagent
        let api_key = auth.api_key("opencode-go").unwrap_or_default();
        run_print_mode(
            message,
            model,
            api_key,
            system_prompt,
            agent_tools,
            &mut agent_session,
        )
        .await
    }
}

async fn run_print_mode(
    message: String,
    model: String,
    api_key: String,
    system_prompt: String,
    agent_tools: Vec<Box<dyn yoagent::types::AgentTool>>,
    agent_session: &mut rab::agent::AgentSession,
) -> anyhow::Result<()> {
    let mut mc = yoagent::provider::model::ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        "deepseek-v4-flash",
        "opencode-go",
        yoagent::provider::model::OpenAiCompat::deepseek(),
    );
    mc.context_window = 1_000_000;
    let mut agent = yoagent::agent::Agent::new(yoagent::provider::OpenAiCompatProvider)
        .with_model(&model)
        .with_api_key(&api_key)
        .with_model_config(mc)
        .with_system_prompt(&system_prompt)
        .with_thinking(yoagent::types::ThinkingLevel::High)
        .with_tools(agent_tools)
        .with_execution_limits(yoagent::context::ExecutionLimits {
            max_total_tokens: usize::MAX,
            max_turns: usize::MAX,
            max_duration: std::time::Duration::from_secs(u64::MAX),
        });

    let (yo_tx, mut yo_rx) = tokio::sync::mpsc::unbounded_channel();
    let msg_for_agent = message.clone();

    // Spawn agent loop (it blocks until done, sending events to yo_tx).
    // Keep the abort handle so we can cancel on timeout.
    let agent_handle = tokio::spawn(async move {
        agent.prompt_with_sender(msg_for_agent, yo_tx).await;
    });

    // Persist user prompt via AgentSession
    let rab_prompt = rab::agent::types::user_message(&message);
    agent_session.send_user_message_obj(&rab_prompt);

    let mut thinking_prefix_printed = false;
    const PRINT_MODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    // Process events from yoagent with a timeout to prevent hanging forever
    // if the provider stops responding (network issue, provider crash, etc.).
    loop {
        let event = tokio::time::timeout(PRINT_MODE_TIMEOUT, yo_rx.recv()).await;

        let event = match event {
            Ok(Some(event)) => event,
            Ok(None) => break, // Channel closed normally — agent finished
            Err(_) => {
                // Timeout: abort the agent task and exit
                agent_handle.abort();
                eprintln!(
                    "{}{}",
                    colored::Colorize::red("✗ "),
                    colored::Colorize::red(
                        "Print mode timed out after 120s — the provider may have hung."
                    )
                );
                break;
            }
        };

        agent_session.on_agent_event(&event);

        match &event {
            yoagent::types::AgentEvent::MessageUpdate { delta, .. } => {
                use yoagent::types::StreamDelta;
                match delta {
                    StreamDelta::Text { delta } => {
                        print!("{}", delta);
                        let _ = std::io::stdout().flush();
                    }
                    StreamDelta::Thinking { delta } => {
                        if !thinking_prefix_printed {
                            eprint!("{}", colored::Colorize::dimmed("… "));
                            thinking_prefix_printed = true;
                        }
                        eprint!("{}", colored::Colorize::dimmed(delta.as_str()));
                        let _ = std::io::stderr().flush();
                    }
                    _ => {}
                }
            }
            yoagent::types::AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                eprintln!(
                    "\n{} {} {}",
                    colored::Colorize::dimmed("⚙"),
                    colored::Colorize::bold(tool_name.as_str()),
                    colored::Colorize::dimmed(
                        serde_json::to_string(args).unwrap_or_default().as_str()
                    )
                );
                thinking_prefix_printed = false;
            }
            yoagent::types::AgentEvent::ToolExecutionEnd {
                result, is_error, ..
            } => {
                let content: String = result
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let yoagent::types::Content::Text { text } = c {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if *is_error {
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
            yoagent::types::AgentEvent::ProgressMessage {
                text, tool_name, ..
            } => {
                if tool_name.is_empty() {
                    // General progress message (not tool-specific) — print to stderr
                    eprint!("{}", text);
                } else {
                    print!("{}", text);
                }
                let _ = std::io::stdout().flush();
            }
            yoagent::types::AgentEvent::AgentEnd { .. } => {
                eprintln!();
            }
            yoagent::types::AgentEvent::MessageEnd { message } => {
                // Check for provider errors (network issues, etc.)
                if let Some(err) = rab::agent::types::message_error(message) {
                    let msg = if err.is_empty() {
                        "Provider error: The agent encountered an issue and stopped."
                    } else {
                        err
                    };
                    eprintln!(
                        "{}{}",
                        colored::Colorize::red("✗ "),
                        colored::Colorize::red(msg)
                    );
                } else if rab::agent::types::message_is_system_stop(message) {
                    let text = rab::agent::types::message_text(message);
                    eprintln!(
                        "{}{}",
                        colored::Colorize::red("✗ "),
                        colored::Colorize::red(text.as_str())
                    );
                } else if let Some(text) = rab::agent::types::message_extension_text(message) {
                    eprintln!(
                        "{}{}",
                        colored::Colorize::dimmed("· "),
                        colored::Colorize::dimmed(text.as_str())
                    );
                }
            }
            yoagent::types::AgentEvent::InputRejected { reason } => {
                eprintln!(
                    "{}{}",
                    colored::Colorize::yellow("! "),
                    colored::Colorize::yellow(reason.as_str())
                );
            }
            _ => {}
        }
    }

    // Run auto-compaction if needed
    match agent_session.check_auto_compact().await {
        Ok(true) => eprintln!("{}", colored::Colorize::dimmed("✓ Compaction completed")),
        Ok(false) => {}
        Err(e) => eprintln!(
            "{}",
            colored::Colorize::yellow(format!("Auto-compaction skipped: {}", e).as_str())
        ),
    }

    Ok(())
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
