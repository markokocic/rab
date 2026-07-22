//! rab — a lightweight, extensible, Rust-based coding agent.
//!
//! Thin entry point. Parses CLI arguments, orchestrates startup via
//! `rab::cli::run`, and dispatches to interactive or print mode.

use rab::cli::args::{get_agent_dir, load_append_system_md, load_system_md};
use rab::cli::run;
use rab::settings::Settings;
use rab::tui::keybindings::init_keybindings;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();

    // Subcommand: rab generate-models
    if raw_args.get(1).map(|s| s.as_str()) == Some("generate-models") {
        return rab::provider::generate_models::run_generate_models().await;
    }

    // Parse CLI arguments
    let args = rab::cli::args::parse_args(&raw_args[1..]);

    let cwd = std::env::current_dir()?;

    // Validate flag conflicts
    run::validate_flag_conflicts(&args);

    // Load settings and auth
    let settings = Settings::load(&cwd)?;
    let model = args
        .model_override
        .clone()
        .unwrap_or_else(|| settings.model().to_string());
    let auth = rab::provider::auth::AuthStorage::create()?;

    // Load provider registry
    let agent_dir = get_agent_dir();
    let registry = rab::provider::ProviderRegistry::load(&agent_dir)?;
    let resolved = registry
        .resolve(&model, settings.default_provider.as_deref())
        .ok();

    // Available models from registry
    let available_models: Vec<String> = registry.list_models();
    let provider_models: Vec<(String, String)> = registry
        .list_model_provider_tuples()
        .into_iter()
        .map(|(p, m, _)| (p, m))
        .collect();

    // Load custom keybindings
    init_keybindings(run::load_keybindings());

    let session_dir = args
        .session_dir_override
        .as_ref()
        .map(std::path::PathBuf::from);

    // Handle --export
    if args.export_path.is_some() {
        eprintln!("Export to HTML is not yet implemented. See --export in pi.");
        std::process::exit(1);
    }

    // Build session
    let mut session = run::build_session(&args, &cwd, session_dir.as_deref());

    // Set session name if provided
    if let Some(ref name) = args.session_name
        && !name.trim().is_empty()
    {
        session.session_mut().append_session_info(name);
    }

    // Build extensions and tools
    let context = session.session().build_context();
    let builtin_ext =
        run::build_builtin_extension(&cwd, &available_models, &provider_models, &settings);
    let mut extensions = run::build_extensions(&cwd).await;
    extensions.insert(0, Box::new(builtin_ext));

    // Load context files
    let context_files = if args.no_context_files {
        Vec::new()
    } else {
        rab::agent::load_context_files(&cwd, &agent_dir)
    };

    // Load SYSTEM.md / APPEND_SYSTEM.md
    let custom_system_md = args
        .system_prompt_override
        .clone()
        .or_else(|| load_system_md(&cwd, &agent_dir));
    let append_system_md = args
        .append_system_prompt_override
        .clone()
        .or_else(|| load_append_system_md(&cwd, &agent_dir));

    // Collect context file display names
    let context_file_names: Vec<String> = context_files
        .iter()
        .map(|cf| rab::cli::args::format_context_path(&cf.path, &cwd))
        .collect();

    // Collect tools, hooks, snippets from enabled extensions
    let (tool_snippets, tool_guidelines, agent_tools) =
        run::build_tools_and_snippets(&extensions, &settings);

    // Register hooks from enabled extensions
    run::register_hooks(&extensions, &settings);

    // Load skills
    let (skill_dirs, skill_set, skills) =
        run::load_skills(&agent_dir, &cwd, &extensions, &settings);

    // Load prompt templates
    let (prompt_template_dirs, prompt_templates) = run::load_prompt_templates(&agent_dir, &cwd);

    // Build system prompt
    let has_read_tool = tool_snippets.iter().any(|t| t.name == "read");
    let system_prompt = rab::agent::SystemPromptBuilder::new()
        .tool_snippets(tool_snippets)
        .guidelines(tool_guidelines)
        .context_files(context_files)
        .custom_prompt(custom_system_md)
        .append_prompt(append_system_md)
        .skills(skill_set.clone())
        .has_read_tool(has_read_tool)
        .cwd(&cwd)
        .build();

    // Determine initial thinking level
    let has_thinking_entries = !session
        .session()
        .find_entries("thinking_level_change")
        .is_empty();
    let thinking_level = if has_thinking_entries {
        Some(context.thinking_level.clone())
    } else {
        settings.default_thinking_level.clone()
    };
    let thinking_level_str = thinking_level.as_deref().or(Some("max"));

    // Dispatch to interactive or print mode
    if args.message_parts.is_empty() {
        run::run_interactive(
            args,
            model,
            settings,
            system_prompt,
            extensions,
            cwd,
            thinking_level_str,
            available_models,
            context_file_names,
            skills,
            skill_dirs,
            agent_dir,
            prompt_templates,
            prompt_template_dirs,
            resolved,
            auth,
            registry,
            session,
        )
        .await
    } else {
        let message = args.message_parts.join(" ");
        run::run_print(
            message,
            model,
            settings,
            system_prompt,
            agent_tools,
            resolved,
            auth,
            session,
        )
        .await
    }
}
