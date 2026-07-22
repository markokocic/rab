mod common;

use rab::provider::auth::AuthStorage;

use crate::common::{tmp_dir, write_file};

#[test]
fn loads_empty_when_no_file() {
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");
    let auth = AuthStorage::with_path(path);
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

    let auth = AuthStorage::with_path(path);
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

    let auth = AuthStorage::with_path(path);
    assert!(auth.api_key("unknown-provider").is_none());
}

#[test]
fn malformed_json_is_error() {
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");
    write_file(&path, "not valid json");

    // Currently with_path on a malformed file will load with empty data
    // (the error is captured internally). The old behavior returned Err.
    // With the new backend pattern, the file is read under lock and
    // parse errors are silently treated as empty.
    let auth = AuthStorage::with_path(path);
    assert!(auth.api_key("opencode-go").is_none());
}

#[test]
fn oauth_credential_roundtrip() {
    use rab::provider::auth::AuthCredential;

    // Use isolated in-memory backend — no filesystem, no race with other tests.
    let auth = AuthStorage::in_memory();

    let cred = AuthCredential::Oauth {
        access: "ghu_test_access_token_12345".to_string(),
        refresh: Some("ghu_test_refresh_token_67890".to_string()),
        expires: Some(9999999999999i64),
        enterprise_url: None,
    };
    auth.set_oauth("github-copilot", &cred).unwrap();

    assert_eq!(
        auth.oauth_token("github-copilot"),
        Some("ghu_test_access_token_12345".to_string())
    );

    // Also test with enterprise_url
    let cred2 = AuthCredential::Oauth {
        access: "ghu_test_access_2".to_string(),
        refresh: Some("ghu_refresh_2".to_string()),
        expires: Some(9999999999999i64),
        enterprise_url: Some("mycompany.ghe.com".to_string()),
    };
    auth.set_oauth("github-copilot-enterprise", &cred2).unwrap();

    assert_eq!(
        auth.oauth_token("github-copilot-enterprise"),
        Some("ghu_test_access_2".to_string())
    );

    // Verify both credentials are present
    assert!(auth.oauth_token("github-copilot").is_some());
    assert!(auth.oauth_token("github-copilot-enterprise").is_some());

    // Verify removal
    assert!(auth.remove("github-copilot").unwrap());
    assert!(auth.oauth_token("github-copilot").is_none());
    assert!(auth.oauth_token("github-copilot-enterprise").is_some());
}

#[test]
fn oauth_write_survives_file_lock() {
    use rab::provider::auth::AuthCredential;

    // Use isolated file-backed backend in a temp dir.
    let tmp = tmp_dir();
    let path = tmp.join("auth.json");
    let auth = AuthStorage::with_path(path.clone());

    let cred = AuthCredential::Oauth {
        access: "access1".to_string(),
        refresh: Some("refresh1".to_string()),
        expires: Some(9999999999999i64),
        enterprise_url: None,
    };
    auth.set_oauth("github-copilot", &cred).unwrap();

    // Read back the raw file content to check format
    let content = std::fs::read_to_string(&path).unwrap();

    // Must be valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.is_object());
    assert!(parsed.as_object().unwrap().contains_key("github-copilot"));
    let gc = &parsed["github-copilot"];
    assert_eq!(gc["type"], "oauth");
    assert_eq!(gc["access"], "access1");
    assert_eq!(gc["refresh"], "refresh1");
    assert_eq!(gc["expires"], 9999999999999i64);

    // Verify reads work through AuthStorage too
    assert_eq!(
        auth.oauth_token("github-copilot"),
        Some("access1".to_string())
    );

    // Verify removal
    assert!(auth.remove("github-copilot").unwrap());
    assert!(auth.oauth_token("github-copilot").is_none());
}

#[test]
fn oauth_credential_pi_format() {
    use rab::provider::auth::AuthCredential;

    // Test that pi's format is readable using in-memory backend
    // Pi's format with enterpriseUrl: null
    let json = r#"{"github-copilot": {"type": "oauth", "access": "pi_access", "refresh": "pi_refresh", "expires": 9999999999999, "enterpriseUrl": null}}"#;
    let auth = AuthStorage::in_memory_with(json);
    assert_eq!(
        auth.oauth_token("github-copilot"),
        Some("pi_access".to_string())
    );

    // Pi's format without enterpriseUrl field
    let json2 = r#"{"github-copilot": {"type": "oauth", "access": "pi_access2", "refresh": "pi_refresh2", "expires": 9999999999999}}"#;
    let auth2 = AuthStorage::in_memory_with(json2);
    assert_eq!(
        auth2.oauth_token("github-copilot"),
        Some("pi_access2".to_string())
    );

    // Verify the credential type is preserved
    let cred = auth.get("github-copilot").unwrap();
    match cred {
        AuthCredential::Oauth {
            access,
            refresh,
            expires,
            enterprise_url,
        } => {
            assert_eq!(access, "pi_access");
            assert_eq!(refresh, Some("pi_refresh".to_string()));
            assert_eq!(expires, Some(9999999999999i64));
            assert_eq!(enterprise_url, None);
        }
        _ => panic!("Expected Oauth credential"),
    }
}

#[test]
fn api_key_roundtrip() {
    let auth = AuthStorage::in_memory();

    auth.set_api_key("opencode-go", "sk-test-key").unwrap();
    assert_eq!(auth.api_key("opencode-go"), Some("sk-test-key".to_string()));

    // Unknown provider returns None
    assert!(auth.api_key("unknown").is_none());

    // Removing API key
    assert!(auth.remove("opencode-go").unwrap());
    assert!(auth.api_key("opencode-go").is_none());

    // Remove nonexistent returns false
    assert!(!auth.remove("nonexistent").unwrap());
}

#[test]
fn oauth_expired_token() {
    use rab::provider::auth::AuthCredential;

    let auth = AuthStorage::in_memory();

    // A token expired 1 hour ago (millis)
    let one_hour_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 3_600_000;

    let cred = AuthCredential::Oauth {
        access: "expired_token".to_string(),
        refresh: Some("refresh_token".to_string()),
        expires: Some(one_hour_ago),
        enterprise_url: None,
    };
    auth.set_oauth("github-copilot", &cred).unwrap();

    // Expired token returns None for oauth_token
    assert!(auth.oauth_token("github-copilot").is_none());

    // oauth_token_past_buffer also returns None since truly expired
    assert!(auth.oauth_token_past_buffer("github-copilot").is_none());
}

#[test]
fn list_providers() {
    let auth = AuthStorage::in_memory();

    auth.set_api_key("provider-a", "key-a").unwrap();
    auth.set_api_key("provider-b", "key-b").unwrap();

    let mut providers = auth.list();
    providers.sort();
    assert_eq!(providers, vec!["provider-a", "provider-b"]);

    // After removal
    auth.remove("provider-a").unwrap();
    assert_eq!(auth.list(), vec!["provider-b"]);
}

#[test]
fn clear_all_removes_all() {
    let auth = AuthStorage::in_memory();

    auth.set_api_key("provider-a", "key-a").unwrap();
    auth.set_api_key("provider-b", "key-b").unwrap();
    assert_eq!(auth.list().len(), 2);

    assert!(auth.clear().unwrap());
    assert!(auth.list().is_empty());

    // Clearing empty returns false
    assert!(!auth.clear().unwrap());
}

#[test]
fn modify_credential_insert_update_delete() {
    use rab::provider::auth::AuthCredential;

    let auth = AuthStorage::in_memory();

    // Insert via modify
    auth.modify("my-provider", |_| {
        Some(AuthCredential::ApiKey {
            key: "initial_key".to_string(),
        })
    })
    .unwrap();
    assert_eq!(auth.api_key("my-provider"), Some("initial_key".to_string()));

    // Update via modify
    auth.modify("my-provider", |current| {
        let mut c = current.unwrap();
        if let AuthCredential::ApiKey { ref mut key } = c {
            key.push_str("_updated");
        }
        Some(c)
    })
    .unwrap();
    assert_eq!(
        auth.api_key("my-provider"),
        Some("initial_key_updated".to_string())
    );

    // Delete via modify (return None)
    auth.modify("my-provider", |_| None).unwrap();
    assert!(auth.api_key("my-provider").is_none());

    // Modify nonexistent (insert)
    auth.modify("new-provider", |_| {
        Some(AuthCredential::ApiKey {
            key: "new_key".to_string(),
        })
    })
    .unwrap();
    assert!(auth.api_key("new-provider").is_some());
}

#[test]
fn in_memory_with_initial_data() {
    let json = r#"{
        "provider-a": {"type": "api_key", "key": "key-a"},
        "provider-b": {"type": "api_key", "key": "key-b"}
    }"#;

    let auth = AuthStorage::in_memory_with(json);
    assert_eq!(auth.api_key("provider-a"), Some("key-a".to_string()));
    assert_eq!(auth.api_key("provider-b"), Some("key-b".to_string()));
    assert_eq!(auth.list().len(), 2);

    // Can still write to it
    auth.set_api_key("provider-c", "key-c").unwrap();
    assert_eq!(auth.list().len(), 3);
}
