use rab::settings::Settings;

fn write_file(path: &std::path::Path, json: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, json).unwrap();
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
