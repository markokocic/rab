use rab::auth::AuthStorage;

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
fn loads_empty_when_no_file() {
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");
    let auth = AuthStorage::load_from(path).unwrap();
    assert!(auth.api_key("opencode-go").is_none());
}

#[test]
fn loads_api_key_from_file() {
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");
    write_file(
        &path,
        r#"{"opencode-go": {"type": "api_key", "key": "sk-test-key"}}"#,
    );

    let auth = AuthStorage::load_from(path).unwrap();
    assert_eq!(auth.api_key("opencode-go"), Some("sk-test-key".into()));
}

#[test]
fn returns_none_for_unknown_provider() {
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");
    write_file(
        &path,
        r#"{"opencode-go": {"type": "api_key", "key": "sk-test-key"}}"#,
    );

    let auth = AuthStorage::load_from(path).unwrap();
    assert!(auth.api_key("unknown-provider").is_none());
}

#[test]
fn malformed_json_is_error() {
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");
    write_file(&path, "not valid json");

    assert!(AuthStorage::load_from(path).is_err());
}
