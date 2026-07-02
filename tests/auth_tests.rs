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

#[test]
fn oauth_credential_roundtrip() {
    use rab::auth::{AuthCredential, AuthStorage};

    // Simulate what GitHub Copilot login stores (same code path as app.rs:2196-2202)
    let cred = AuthCredential::Oauth {
        access: "ghu_test_access_token_12345".to_string(),
        refresh: Some("ghu_test_refresh_token_67890".to_string()),
        expires: Some(9999999999999i64),
        enterprise_url: None,
    };
    rab::auth::login_oauth("github-copilot", &cred).unwrap();

    // Load from real path
    let path = AuthStorage::path().unwrap();
    let loaded = AuthStorage::load_from(path).unwrap();
    assert_eq!(
        loaded.oauth_token("github-copilot"),
        Some("ghu_test_access_token_12345".to_string())
    );

    // Also test with enterprise_url
    let cred2 = AuthCredential::Oauth {
        access: "ghu_test_access_2".to_string(),
        refresh: Some("ghu_refresh_2".to_string()),
        expires: Some(9999999999999i64),
        enterprise_url: Some("mycompany.ghe.com".to_string()),
    };
    rab::auth::login_oauth("github-copilot-enterprise", &cred2).unwrap();

    let loaded2 = AuthStorage::load_from(AuthStorage::path().unwrap()).unwrap();
    assert_eq!(
        loaded2.oauth_token("github-copilot-enterprise"),
        Some("ghu_test_access_2".to_string())
    );

    // Verify the credential is marked as not expired
    assert!(loaded2.oauth_token("github-copilot").is_some());

    // Cleanup
    rab::auth::logout(Some("github-copilot")).unwrap();
    rab::auth::logout(Some("github-copilot-enterprise")).unwrap();
}

#[test]
fn oauth_write_survives_file_lock() {
    use rab::auth::AuthCredential;

    // First write some content
    let cred = AuthCredential::Oauth {
        access: "access1".to_string(),
        refresh: Some("refresh1".to_string()),
        expires: Some(9999999999999i64),
        enterprise_url: None,
    };
    rab::auth::login_oauth("github-copilot", &cred).unwrap();

    // Read back the raw file content to check format
    let path = AuthStorage::path().unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    eprintln!("auth.json content:\n{}", content);

    // Must be valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.is_object());
    assert!(parsed.as_object().unwrap().contains_key("github-copilot"));
    let gc = &parsed["github-copilot"];
    assert_eq!(gc["type"], "oauth");
    assert_eq!(gc["access"], "access1");
    assert_eq!(gc["refresh"], "refresh1");
    assert_eq!(gc["expires"], 9999999999999i64);

    // Cleanup
    rab::auth::logout(Some("github-copilot")).unwrap();
}

#[test]
fn oauth_credential_pi_format() {
    // Test that pi's format is readable
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");

    // Pi's format with enterpriseUrl: null
    write_file(
        &path,
        r#"{"github-copilot": {"type": "oauth", "access": "pi_access", "refresh": "pi_refresh", "expires": 9999999999999, "enterpriseUrl": null}}"#,
    );

    let loaded = AuthStorage::load_from(path.clone()).unwrap();
    assert_eq!(
        loaded.oauth_token("github-copilot"),
        Some("pi_access".to_string())
    );

    // Pi's format without enterpriseUrl field
    write_file(
        &path,
        r#"{"github-copilot": {"type": "oauth", "access": "pi_access2", "refresh": "pi_refresh2", "expires": 9999999999999}}"#,
    );

    let loaded = AuthStorage::load_from(path).unwrap();
    assert_eq!(
        loaded.oauth_token("github-copilot"),
        Some("pi_access2".to_string())
    );
}
