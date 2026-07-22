use rab::builtin::extension::BuiltinExtension;
use rab::extension::{CommandResult, Extension as _};

fn test_ext() -> BuiltinExtension {
    BuiltinExtension::new(std::path::PathBuf::from("."))
}

#[test]
fn test_quit_command() {
    let cmds = test_ext().commands();
    let quit_cmd = cmds.iter().find(|c| c.name == "quit").unwrap();
    let result = quit_cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Quit => {}
        other => panic!("Expected Quit, got {:?}", other),
    }
}

#[test]
fn test_model_command_no_args_opens_selector() {
    let ext = test_ext().with_available_models(vec!["m1".into(), "m2".into()]);
    let cmds = ext.commands();
    let model_cmd = cmds.iter().find(|c| c.name == "model").unwrap();
    let result = model_cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::OpenModelSelector => {}
        other => panic!("Expected OpenModelSelector, got {:?}", other),
    }
}

#[test]
fn test_model_command_valid_model() {
    let ext = test_ext()
        .with_available_models(vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()]);
    let cmds = ext.commands();
    let model_cmd = cmds.iter().find(|c| c.name == "model").unwrap();
    let result = model_cmd.handler.execute("deepseek-v4-flash");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::ModelChanged(ref name) => assert_eq!(name, "deepseek-v4-flash"),
        other => panic!("Expected ModelChanged, got {:?}", other),
    }
}

#[test]
fn test_model_command_unknown_model() {
    let ext = test_ext().with_available_models(vec!["m1".into()]);
    let cmds = ext.commands();
    let model_cmd = cmds.iter().find(|c| c.name == "model").unwrap();
    let result = model_cmd.handler.execute("nonexistent");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Info(ref text) => assert!(text.contains("Unknown model")),
        other => panic!("Expected Info, got {:?}", other),
    }
}

#[test]
fn test_model_argument_completions() {
    let ext = test_ext()
        .with_available_models(vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()]);
    let cmds = ext.commands();
    let model_cmd = cmds.iter().find(|c| c.name == "model").unwrap();
    let completions = model_cmd.handler.argument_completions("deep");
    assert_eq!(completions.len(), 2);
    let completions = model_cmd.handler.argument_completions("flash");
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].value, "deepseek-v4-flash");
    let completions = model_cmd.handler.argument_completions("zzz");
    assert_eq!(completions.len(), 0);
}

// ── /hotkeys ──────────────────────────────────────────────────────

#[test]
fn hotkeys_command_returns_show_help() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "hotkeys").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::ShowHelp => {}
        other => panic!("Expected ShowHelp, got {:?}", other),
    }
}

// ── /reload ───────────────────────────────────────────────────────

#[test]
fn reload_command_returns_reloaded() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "reload").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Reloaded => {}
        other => panic!("Expected Reloaded, got {:?}", other),
    }
}

// ── /new ──────────────────────────────────────────────────────────

#[test]
fn new_command_returns_new_session() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "new").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::NewSession => {}
        other => panic!("Expected NewSession, got {:?}", other),
    }
}

// ── /resume ───────────────────────────────────────────────────────

#[test]
fn resume_command_opens_session_selector() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "resume").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::OpenSessionSelector => {}
        other => panic!("Expected OpenSessionSelector, got {:?}", other),
    }
}

// ── /session ──────────────────────────────────────────────────────

// ── /session no-info is no longer possible; handler always returns sentinel.
// See session_command_returns_sentinel above.

#[test]
fn session_command_returns_sentinel() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "session").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::SessionInfo {
            session_id,
            message_count,
            ..
        } => {
            // Handler returns sentinel with empty fields; real data is filled
            // in by app.rs from the live Session.
            assert!(session_id.is_empty());
            assert_eq!(message_count, 0);
        }
        other => panic!("Expected SessionInfo, got {:?}", other),
    }
}

// ── /name ─────────────────────────────────────────────────────────

#[test]
fn name_command_sets_name() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "name").unwrap();
    let result = cmd.handler.execute("My Task");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::SessionNamed { ref name } => {
            assert_eq!(name, "My Task");
        }
        other => panic!("Expected SessionNamed, got {:?}", other),
    }
}

#[test]
fn name_command_empty_shows_usage() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "name").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Info(ref text) => {
            assert!(text.contains("Usage"));
            assert!(text.contains("/name"));
        }
        other => panic!("Expected Info, got {:?}", other),
    }
}

#[test]
fn name_command_trims_whitespace() {
    let cmds = test_ext().commands();
    let cmd = cmds.iter().find(|c| c.name == "name").unwrap();
    let result = cmd.handler.execute("   spaced   ");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::SessionNamed { ref name } => {
            assert_eq!(name, "spaced");
        }
        other => panic!("Expected SessionNamed, got {:?}", other),
    }
}
