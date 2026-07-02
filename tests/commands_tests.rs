use rab::agent::extension::{CommandResult, Extension};
use rab::builtin::commands::{CommandsExtension, SessionInfoInternal};

#[test]
fn test_quit_command() {
    let ext = CommandsExtension::new(vec!["m1".into(), "m2".into()]);
    let cmds = ext.commands();
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
    let ext = CommandsExtension::new(vec!["m1".into(), "m2".into()]);
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
    let ext = CommandsExtension::new(vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()]);
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
    let ext = CommandsExtension::new(vec!["m1".into()]);
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
    let ext = CommandsExtension::new(vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()]);
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
fn hotkeys_command_returns_not_implemented() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "hotkeys").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Info(msg) => {
            assert!(msg.contains("not implemented"), "unexpected info: {}", msg);
        }
        other => panic!("Expected Info(not implemented), got {:?}", other),
    }
}

#[test]
fn hotkeys_ignores_args() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "hotkeys").unwrap();
    let result = cmd.handler.execute("anything");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Info(msg) => {
            assert!(msg.contains("not implemented"), "unexpected info: {}", msg);
        }
        other => panic!("Expected Info(not implemented), got {:?}", other),
    }
}

// ── /reload ───────────────────────────────────────────────────────

#[test]
fn reload_command_returns_reloaded() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
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
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "new").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::NewSession => {}
        other => panic!("Expected NewSession, got {:?}", other),
    }
}

#[test]
fn new_ignores_args() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "new").unwrap();
    let result = cmd.handler.execute("some args");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::NewSession => {}
        other => panic!("Expected NewSession, got {:?}", other),
    }
}

// ── /resume ───────────────────────────────────────────────────────

#[test]
fn resume_command_opens_session_selector() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "resume").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::OpenSessionSelector => {}
        other => panic!("Expected OpenSessionSelector, got {:?}", other),
    }
}

#[test]
fn resume_ignores_args() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "resume").unwrap();
    let result = cmd.handler.execute("some args");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::OpenSessionSelector => {}
        other => panic!("Expected OpenSessionSelector, got {:?}", other),
    }
}

// ── /session ──────────────────────────────────────────────────────

#[test]
fn session_command_no_info() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "session").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Info(ref text) => {
            assert!(text.contains("No active session"));
        }
        other => panic!("Expected Info, got {:?}", other),
    }
}

#[test]
fn session_command_with_info() {
    let ext = CommandsExtension::new(vec![]);
    ext.set_session_info(SessionInfoInternal {
        session_id: "abc123".to_string(),
        file_path: Some(std::path::PathBuf::from("/tmp/test.jsonl")),
        name: Some("Test".to_string()),
        message_count: 42,
        user_messages: 10,
        assistant_messages: 8,
        tool_calls: 15,
        tool_results: 12,
        total_tokens: 5000,
        input_tokens: 3000,
        output_tokens: 1500,
        cache_read_tokens: 500,
        cache_write_tokens: 0,
        cost: 0.0123,
    });
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "session").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::SessionInfo {
            session_id,
            file_path,
            name,
            message_count,
            ..
        } => {
            assert_eq!(session_id, "abc123");
            assert_eq!(file_path, Some(std::path::PathBuf::from("/tmp/test.jsonl")));
            assert_eq!(name, Some("Test".to_string()));
            assert_eq!(message_count, 42);
        }
        other => panic!("Expected SessionInfo, got {:?}", other),
    }
}

// ── /name ─────────────────────────────────────────────────────────

#[test]
fn name_command_sets_name() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
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
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
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
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
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
