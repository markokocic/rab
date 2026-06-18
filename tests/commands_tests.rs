use rab::builtin::commands::CommandsExtension;
use rab::extension::{CommandResult, Extension};

/// Simulate the prefix matching logic used by `submit_message`.
/// Returns (command_name, args) when exactly one prefix match, or None.
fn resolve_command(typed: &str, exts: &[Box<dyn Extension>]) -> Option<(String, String)> {
    let (cmd_part, args) = match typed.trim().split_once(' ') {
        Some((cmd, rest)) => (cmd.trim_start_matches('/').to_string(), rest.to_string()),
        None => (
            typed.trim().trim_start_matches('/').to_string(),
            String::new(),
        ),
    };
    let lower = cmd_part.to_lowercase();

    // Collect owned command names (commands() returns temporary Vecs)
    let mut cmds: Vec<String> = Vec::new();
    for ext in exts {
        for c in ext.commands() {
            if !cmds.contains(&c.name) {
                cmds.push(c.name.clone());
            }
        }
    }

    // Exact match
    if cmds.contains(&cmd_part) {
        return Some((cmd_part, args));
    }

    // Prefix match
    let matches: Vec<&String> = cmds
        .iter()
        .filter(|n| n.to_lowercase().starts_with(&lower))
        .collect();
    if matches.len() == 1 {
        Some((matches[0].clone(), args))
    } else {
        None
    }
}

#[test]
fn exact_quit() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec!["m1".into(), "m2".into()]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    let result = resolve_command("/quit", &exts);
    assert_eq!(result, Some(("quit".into(), String::new())));
}

#[test]
fn prefix_q_resolves_to_quit() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec!["m1".into(), "m2".into()]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    let result = resolve_command("/q", &exts);
    assert_eq!(result, Some(("quit".into(), String::new())));
}

#[test]
fn exact_model_with_args() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec!["m1".into(), "m2".into()]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    let result = resolve_command("/model deepseek-v4-flash", &exts);
    assert_eq!(result, Some(("model".into(), "deepseek-v4-flash".into())));
}

#[test]
fn prefix_mo_resolves_to_model() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec!["m1".into(), "m2".into()]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    // /mo uniquely matches /model
    let result = resolve_command("/mo", &exts);
    assert_eq!(result, Some(("model".into(), String::new())));
}

#[test]
fn prefix_hotkeys_resolves_to_hotkeys() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec![]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    let result = resolve_command("/hot", &exts);
    assert_eq!(result, Some(("hotkeys".into(), String::new())));
}

#[test]
fn prefix_reload_resolves_to_reload() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec![]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    let result = resolve_command("/rel", &exts);
    assert_eq!(result, Some(("reload".into(), String::new())));
}

#[test]
fn unknown_command_no_match() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec!["m1".into(), "m2".into()]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    let result = resolve_command("/unknown", &exts);
    assert_eq!(result, None);
}

#[test]
fn prefix_match_is_case_insensitive() {
    let ext: Box<dyn Extension> = Box::new(CommandsExtension::new(vec!["m1".into(), "m2".into()]));
    let exts: Vec<Box<dyn Extension>> = vec![ext];
    let result = resolve_command("/Q", &exts);
    assert_eq!(result, Some(("quit".into(), String::new())));
}

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
fn test_model_command_no_args_lists_models() {
    let ext = CommandsExtension::new(vec!["m1".into(), "m2".into()]);
    let cmds = ext.commands();
    let model_cmd = cmds.iter().find(|c| c.name == "model").unwrap();
    let result = model_cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::Info(ref text) => {
            assert!(text.contains("Available models"));
            assert!(text.contains("m1"));
        }
        other => panic!("Expected Info, got {:?}", other),
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
fn hotkeys_command_returns_show_help() {
    let ext = CommandsExtension::new(vec![]);
    let cmds = ext.commands();
    let cmd = cmds.iter().find(|c| c.name == "hotkeys").unwrap();
    let result = cmd.handler.execute("");
    assert!(result.is_ok());
    match result.unwrap() {
        CommandResult::ShowHelp => {}
        other => panic!("Expected ShowHelp, got {:?}", other),
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
        CommandResult::ShowHelp => {}
        other => panic!("Expected ShowHelp, got {:?}", other),
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
