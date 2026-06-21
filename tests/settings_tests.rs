use rab::agent::settings::Settings;

fn write_file(path: &std::path::Path, json: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, json).unwrap();
}

fn read_file(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Extract --model value from CLI args (matches main.rs logic).
fn resolve_model_override(args: &[&str]) -> Option<String> {
    let pos = args.iter().position(|a| a == &"--model")?;
    args.get(pos + 1).map(|s| s.to_string())
}

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn defaults_when_no_config() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.model(), "deepseek-v4-flash");
    assert!(s.default_model.is_none());
    assert!(!s.verbose);
}

#[test]
fn project_override_takes_precedence() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(
        &global,
        r#"{"defaultModel": "global", "defaultThinkingLevel": "low"}"#,
    );
    write_file(
        &tmp.join(".rab").join("settings.json"),
        r#"{"defaultModel": "project"}"#,
    );

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.model(), "project");
    assert_eq!(s.default_thinking_level.as_deref(), Some("low"));
}

#[test]
fn tools_replaced_by_project_not_merged() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"tools": ["read", "write"]}"#);
    write_file(
        &tmp.join(".rab").join("settings.json"),
        r#"{"tools": ["bash"]}"#,
    );

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.tools, vec!["bash"]);
}

#[test]
fn verbose_true_in_either_wins() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"verbose": true}"#);
    write_file(
        &tmp.join(".rab").join("settings.json"),
        r#"{"verbose": false}"#,
    );

    let s = Settings::load_from(global, &tmp).unwrap();
    assert!(s.verbose);
}

#[test]
fn global_bad_json_is_error() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, "not valid json");
    assert!(Settings::load_from(global, &tmp).is_err());
}

#[test]
fn project_bad_json_is_graceful() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"defaultModel": "global"}"#);
    write_file(&tmp.join(".rab").join("settings.json"), "bad json");

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.model(), "global");
}

// ── hideThinkingBlock deserialization ──────────────────────────────

#[test]
fn hide_thinking_block_defaults_to_false() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{}"#);

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.hide_thinking, None);
}

#[test]
fn hide_thinking_block_deserializes_from_pi_key() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"hideThinkingBlock": true}"#);

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.hide_thinking, Some(true));
}

#[test]
fn hide_thinking_block_false_is_read() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"hideThinkingBlock": false}"#);

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.hide_thinking, Some(false));
}

// ── collapseToolOutput deserialization ────────────────────────────

#[test]
fn collapse_tool_output_defaults_to_false() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{}"#);

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.collapse_tool_output, None);
}

#[test]
fn collapse_tool_output_deserializes() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"collapseToolOutput": true}"#);

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.collapse_tool_output, Some(true));
}

// ── Merge behavior ────────────────────────────────────────────────

#[test]
fn hide_thinking_project_overrides_global() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"hideThinkingBlock": true}"#);
    write_file(
        &tmp.join(".rab").join("settings.json"),
        r#"{"hideThinkingBlock": false}"#,
    );

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.hide_thinking, Some(false)); // project wins
}

#[test]
fn collapse_tool_output_project_overrides_global() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"collapseToolOutput": false}"#);
    write_file(
        &tmp.join(".rab").join("settings.json"),
        r#"{"collapseToolOutput": true}"#,
    );

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.collapse_tool_output, Some(true)); // project wins
}

#[test]
fn hide_thinking_global_used_when_project_not_set() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"hideThinkingBlock": true}"#);
    // No project settings file

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.hide_thinking, Some(true));
}

#[test]
fn collapse_tool_output_global_used_when_project_not_set() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"collapseToolOutput": true}"#);

    let s = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s.collapse_tool_output, Some(true));
}

// ── Serialization (save) ──────────────────────────────────────────

#[test]
fn save_writes_hide_thinking_block() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{}"#);

    Settings {
        hide_thinking: Some(true),
        ..Default::default()
    }
    .save_to(global.clone())
    .unwrap();

    let content = read_file(&global);
    assert!(content.contains(r#"hideThinkingBlock"#));
    assert!(content.contains(r#"true"#));
}

#[test]
fn save_writes_collapse_tool_output() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{}"#);

    Settings {
        collapse_tool_output: Some(true),
        ..Default::default()
    }
    .save_to(global.clone())
    .unwrap();

    let content = read_file(&global);
    assert!(content.contains(r#"collapseToolOutput"#));
    assert!(content.contains(r#"true"#));
}

#[test]
fn save_roundtrip_preserves_all_fields() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(
        &global,
        r#"{
            "defaultModel": "deepseek-v4-pro",
            "hideThinkingBlock": true,
            "collapseToolOutput": true
        }"#,
    );

    // Load, save, load again
    let s1 = Settings::load_from(global.clone(), &tmp).unwrap();
    s1.save_to(global.clone()).unwrap();

    let s2 = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(s2.model(), "deepseek-v4-pro");
    assert_eq!(s2.hide_thinking, Some(true));
    assert_eq!(s2.collapse_tool_output, Some(true));
}

#[test]
fn save_creates_parent_directory() {
    let tmp = tmp_dir();
    // Use a path in a non-existent subdirectory
    let deep_path = tmp.join("sub").join("dir").join("settings.json");

    let s = Settings::default();
    s.save_to(deep_path.clone()).unwrap();
    assert!(deep_path.exists());

    let content = read_file(&deep_path);
    // Should contain all default fields
    assert!(content.contains(r#"hideThinkingBlock"#));
    assert!(content.contains(r#"collapseToolOutput"#));
}

// ── Model persistence ──────────────────────────────────────────────

#[test]
fn model_save_persists_default_model() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{}"#);

    Settings {
        default_model: Some("deepseek-v4-pro".into()),
        ..Default::default()
    }
    .save_to(global.clone())
    .unwrap();

    let content = read_file(&global);
    assert!(content.contains(r#"defaultModel"#));
    assert!(content.contains(r#"deepseek-v4-pro"#));
}

#[test]
fn model_save_roundtrip() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");

    Settings {
        default_model: Some("deepseek-v4-flash".into()),
        ..Default::default()
    }
    .save_to(global.clone())
    .unwrap();

    let loaded = Settings::load_from(global, &tmp).unwrap();
    assert_eq!(loaded.model(), "deepseek-v4-flash");
}

#[test]
fn model_override_takes_precedence_over_settings() {
    // Simulates main.rs behavior: --model flag > settings.defaultModel
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"defaultModel": "deepseek-v4-flash"}"#);

    let settings = Settings::load_from(global.clone(), &tmp).unwrap();
    // Simulate CLI arg: --model=deepseek-v4-pro overrides settings
    let model = resolve_model_override(&["--model", "deepseek-v4-pro"])
        .unwrap_or_else(|| settings.model().to_string());

    assert_eq!(model, "deepseek-v4-pro");
    // Settings still has the original value
    assert_eq!(settings.model(), "deepseek-v4-flash");
}

#[test]
fn model_loads_from_settings_when_no_override() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{"defaultModel": "deepseek-v4-pro"}"#);

    let settings = Settings::load_from(global.clone(), &tmp).unwrap();
    let model = settings.model().to_string();

    assert_eq!(model, "deepseek-v4-pro");
}

#[test]
fn model_defaults_when_not_set() {
    let tmp = tmp_dir();
    let global = tmp.join("global.json");
    write_file(&global, r#"{}"#);

    let settings = Settings::load_from(global, &tmp).unwrap();
    // Uses the hardcoded default in settings.model()
    assert_eq!(settings.model(), "deepseek-v4-flash");
}
