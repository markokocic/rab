//! CLI startup orchestration — builds sessions, extensions, tools, and dispatches
//! to interactive or print mode.
//!
//! These functions are called from `src/main.rs` and are placed here so they
//! live in the library crate and are testable.

use std::sync::Arc;

use crate::agent::ui;
use crate::builtin::{bash::BashToolOptions, extension::BuiltinExtension};
use crate::cli::session::resolve_session_arg;
use crate::extension::Extension;
use crate::settings::Settings;
use crate::tui::keybindings::Keybindings;
use yoagent::types::AgentTool as _;

// ── Flag validation ────────────────────────────────────────────

pub fn validate_flag_conflicts(args: &crate::cli::args::CliArgs) {
    let conflicting_flags: Vec<&str> = [
        (args.fork_source.is_some(), "--fork"),
        (args.continue_session, "--continue"),
        (args.resume_session, "--resume"),
        (args.no_session, "--no-session"),
    ]
    .into_iter()
    .filter_map(|(cond, name)| if cond { Some(name) } else { None })
    .collect();

    if args.fork_source.is_some() && conflicting_flags.len() > 1 {
        for f in &conflicting_flags[1..] {
            eprintln!("Error: --fork cannot be combined with {}", f);
        }
        std::process::exit(1);
    }

    if args.session_id.is_some() {
        let mut conflicting: Vec<&str> = Vec::new();
        if args.session_path.is_some() {
            conflicting.push("--session");
        }
        if args.continue_session {
            conflicting.push("--continue");
        }
        if args.resume_session {
            conflicting.push("--resume");
        }
        if args.no_session {
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
}

// ── Session building ───────────────────────────────────────────

pub fn build_session(
    args: &crate::cli::args::CliArgs,
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
) -> crate::agent::AgentSession {
    if let Some(ref fork_arg) = args.fork_source {
        let resolved = match resolve_session_arg(fork_arg, cwd, session_dir) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };
        if let Some(ref sid) = args.session_id {
            let sessions_dir = session_dir
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| crate::agent::session::get_default_session_dir(cwd));
            let sessions = crate::agent::session::list_sessions(&sessions_dir);
            if sessions.iter().any(|s| s.id == *sid) {
                eprintln!("Session already exists with id '{}'", sid);
                std::process::exit(1);
            }
        }
        match crate::agent::AgentSession::fork_from(resolved.path(), cwd, session_dir) {
            Ok(s) => {
                eprintln!("Forked session {}", s.session_id());
                s
            }
            Err(e) => {
                eprintln!("Error: fork failed: {}", e);
                std::process::exit(1);
            }
        }
    } else if args.no_session {
        crate::agent::AgentSession::in_memory(cwd)
    } else if let Some(ref path_or_id) = args.session_path {
        match resolve_session_arg(path_or_id, cwd, session_dir) {
            Ok(resolved) => {
                if let Some(session_cwd) = resolved.cwd() {
                    let resolved_cwd = std::path::Path::new(session_cwd);
                    if resolved_cwd != cwd {
                        eprintln!("Warning: session from different project: {}", session_cwd);
                        eprintln!("Use --fork to fork it into the current directory.");
                    }
                }
                let path = resolved.path().to_path_buf();
                crate::agent::AgentSession::open(&path, session_dir, None)
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else if args.resume_session || args.continue_session {
        crate::agent::AgentSession::continue_recent(cwd, session_dir)
    } else if let Some(ref sid) = args.session_id {
        let sessions_dir = session_dir
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| crate::agent::session::get_default_session_dir(cwd));
        let sessions = crate::agent::session::list_sessions(&sessions_dir);
        let existing = sessions.iter().find(|s| s.id == *sid);
        if let Some(s) = existing {
            crate::agent::AgentSession::open(&s.path, session_dir, None)
        } else {
            crate::agent::AgentSession::create(cwd, session_dir)
        }
    } else {
        crate::agent::AgentSession::create(cwd, session_dir)
    }
}

// ── Keybindings ────────────────────────────────────────────────

pub fn load_keybindings() -> Keybindings {
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
    keybindings
}

// ── Extension building ─────────────────────────────────────────

pub fn build_builtin_extension(
    cwd: &std::path::Path,
    available_models: &[String],
    provider_models: &[(String, String)],
    settings: &Settings,
) -> BuiltinExtension {
    let builtin_ext = BuiltinExtension::new(cwd.to_path_buf());
    builtin_ext.set_available_models(available_models.to_vec());
    builtin_ext.set_provider_models(provider_models.to_vec());

    let bash_options = BashToolOptions {
        command_prefix: settings.shell_command_prefix.clone(),
        shell_path: settings.shell_path.clone(),
        operations: None,
    };
    builtin_ext.with_bash_options(bash_options)
}

pub async fn build_extensions(cwd: &std::path::Path) -> Vec<Box<dyn Extension>> {
    let mut extensions: Vec<Box<dyn Extension>> = Vec::new();

    let file_search_ext =
        crate::extensions::file_search::FileSearchExtension::new(cwd.to_path_buf());
    extensions.push(Box::new(file_search_ext));

    let mcp_ext = crate::extensions::mcp::McpExtension::from_cwd(cwd);
    mcp_ext.restore_cache().await;
    mcp_ext.bootstrap_direct_tools().await;
    extensions.push(Box::new(mcp_ext));

    let ts_ext = crate::extensions::tree_sitter::TreeSitterExtension::new();
    extensions.push(Box::new(ts_ext));

    extensions
}

// ── Tool assembly ──────────────────────────────────────────────

pub fn build_tools_and_snippets(
    extensions: &[Box<dyn Extension>],
    settings: &Settings,
) -> (
    Vec<crate::agent::ToolSnippet>,
    Vec<String>,
    Vec<Box<dyn yoagent::types::AgentTool>>,
) {
    let all_tools: Vec<crate::extension::ToolDefinition> = extensions
        .iter()
        .filter(|ext| crate::extension::is_extension_enabled(ext.as_ref(), settings))
        .flat_map(|ext| ext.tools())
        .collect();

    let tool_snippets: Vec<crate::agent::ToolSnippet> = all_tools
        .iter()
        .map(|twm| crate::agent::ToolSnippet {
            name: twm.name().to_string(),
            description: twm.snippet.to_string(),
        })
        .collect();

    let tool_guidelines: Vec<String> = all_tools
        .iter()
        .flat_map(|twm| twm.guidelines.iter().copied())
        .map(|s| s.to_string())
        .collect();

    let agent_tools: Vec<Box<dyn yoagent::types::AgentTool>> = all_tools
        .into_iter()
        .map(|twm| Box::new(twm) as Box<dyn yoagent::types::AgentTool>)
        .collect();

    (tool_snippets, tool_guidelines, agent_tools)
}

pub fn register_hooks(extensions: &[Box<dyn Extension>], settings: &Settings) {
    let all_hooks: Vec<crate::extension::HookRegistration> = extensions
        .iter()
        .filter(|ext| crate::extension::is_extension_enabled(ext.as_ref(), settings))
        .flat_map(|ext| ext.tool_hooks())
        .collect();
    crate::extension::register_tool_hooks(&all_hooks);
}

// ── Skills loading ─────────────────────────────────────────────

/// Collect directories for a given subdirectory name, walking from agent_dir,
/// home `.agents`, and up the directory tree from cwd.
fn collect_subdirs(
    agent_dir: &std::path::Path,
    cwd: &std::path::Path,
    subdir: &str,
) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(agent_dir.join(subdir));
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
        dirs.push(home.join(".agents").join(subdir));
    }
    let mut current = Some(cwd.to_path_buf());
    while let Some(dir) = current {
        dirs.push(dir.join(".rab").join(subdir));
        dirs.push(dir.join(".agents").join(subdir));
        current = dir.parent().filter(|p| *p != dir).map(|p| p.to_path_buf());
    }
    dirs
}

pub fn load_skills(
    agent_dir: &std::path::Path,
    cwd: &std::path::Path,
    extensions: &[Box<dyn Extension>],
    settings: &Settings,
) -> (
    Vec<std::path::PathBuf>,
    yoagent::skills::SkillSet,
    Vec<yoagent::skills::Skill>,
) {
    let skill_dirs = collect_subdirs(agent_dir, cwd, "skills");

    let mut skill_set = yoagent::skills::SkillSet::load(&skill_dirs).unwrap_or_default();
    for ext in extensions
        .iter()
        .filter(|ext| crate::extension::is_extension_enabled(ext.as_ref(), settings))
    {
        skill_set.merge(ext.skills());
    }
    let skills = skill_set.skills().to_vec();

    (skill_dirs, skill_set, skills)
}

// ── Prompt templates loading ───────────────────────────────────

pub fn load_prompt_templates(
    agent_dir: &std::path::Path,
    cwd: &std::path::Path,
) -> (
    Vec<std::path::PathBuf>,
    Vec<crate::agent::prompt_templates::PromptTemplate>,
) {
    let prompt_template_dirs = collect_subdirs(agent_dir, cwd, "prompts");
    let templates = crate::agent::prompt_templates::load_prompt_templates(&prompt_template_dirs);
    (prompt_template_dirs, templates)
}

// ── Interactive mode dispatch ──────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn run_interactive(
    args: crate::cli::args::CliArgs,
    model: String,
    settings: Settings,
    system_prompt: String,
    extensions: Vec<Box<dyn Extension>>,
    cwd: std::path::PathBuf,
    thinking_level_str: Option<&str>,
    available_models: Vec<String>,
    context_file_names: Vec<String>,
    skills: Vec<yoagent::skills::Skill>,
    skill_dirs: Vec<std::path::PathBuf>,
    agent_dir: std::path::PathBuf,
    prompt_templates: Vec<crate::agent::prompt_templates::PromptTemplate>,
    prompt_template_dirs: Vec<std::path::PathBuf>,
    resolved: Option<crate::provider::ResolvedModel>,
    auth: crate::provider::auth::AuthStorage,
    registry: crate::provider::ProviderRegistry,
    session: crate::agent::AgentSession,
) -> anyhow::Result<()> {
    let api_key = resolved
        .as_ref()
        .map(|r| r.api_key.clone())
        .or_else(|| auth.api_key("opencode-go"))
        .unwrap_or_default();
    let provider = resolved
        .as_ref()
        .map(|r| r.model_config.provider.clone())
        .unwrap_or_default();

    let config = ui::AppConfig {
        model,
        provider,
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
        skill_dirs,
        agent_dir,
        prompt_templates,
        prompt_template_dirs,
        api_key,
        registry: Arc::new(registry),
        open_session_picker: args.resume_session,
    };
    ui::run(config, session).await
}

// ── Print mode dispatch ────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn run_print(
    message: String,
    model: String,
    settings: Settings,
    system_prompt: String,
    agent_tools: Vec<Box<dyn yoagent::types::AgentTool>>,
    resolved: Option<crate::provider::ResolvedModel>,
    auth: crate::provider::auth::AuthStorage,
    mut agent_session: crate::agent::AgentSession,
) -> anyhow::Result<()> {
    let mut api_key = resolved
        .as_ref()
        .map(|r| r.api_key.clone())
        .or_else(|| auth.api_key("opencode-go"))
        .unwrap_or_default();

    // Refresh OAuth token if expired
    let provider = resolved
        .as_ref()
        .map(|r| r.model_config.provider.as_str())
        .unwrap_or("");
    if api_key.is_empty() && crate::provider::oauth::is_built_in(provider) {
        api_key = crate::provider::auth::refresh_oauth_token(provider)
            .await
            .unwrap_or(api_key);
    }

    let mut mc = resolved
        .as_ref()
        .map(|r| r.model_config.clone())
        .unwrap_or_else(|| crate::agent::base_model_config(&model));
    let rab_compat = resolved
        .as_ref()
        .map(|r| r.rab_compat.clone())
        .unwrap_or_default();

    // Inject provider attribution/session headers
    let session_id = Some(agent_session.session_id());
    let enable_telemetry = settings.enable_install_telemetry.unwrap_or(false);
    crate::provider::inject_provider_attribution_headers(
        &mut mc,
        session_id.as_deref(),
        enable_telemetry,
    );

    agent_session.set_compaction_config(
        api_key.clone(),
        &model,
        mc.context_window as u64,
        Some(mc.clone()),
        Some(rab_compat.clone()),
    );
    if let Some(ref cc) = settings.compaction {
        agent_session.apply_compaction_config(cc);
    }

    crate::cli::print_mode::run_print_mode(
        message,
        api_key,
        mc,
        rab_compat,
        system_prompt,
        agent_tools,
        &mut agent_session,
    )
    .await
}
