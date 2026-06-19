use rab::agent::extension::Extension;
use rab::builtin::bash::BashExtension;

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn runs_simple_command() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute("id".into(), serde_json::json!({"command": "echo hello"}))
        .await
        .unwrap();

    assert!(result.contains("hello"));
}

#[tokio::test]
async fn captures_stdout_and_stderr() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"command": "echo stdout; echo stderr >&2"}),
        )
        .await
        .unwrap();

    assert!(result.contains("stdout"));
    assert!(result.contains("stderr"));
}

#[tokio::test]
async fn returns_error_on_nonzero_exit() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute("id".into(), serde_json::json!({"command": "exit 1"}))
        .await
        .unwrap();

    assert!(result.contains("exited with code"));
}

#[tokio::test]
async fn runs_in_working_directory() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute("id".into(), serde_json::json!({"command": "pwd"}))
        .await
        .unwrap();

    assert!(result.contains(tmp.to_str().unwrap()));
}

#[tokio::test]
async fn handles_empty_output() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute("id".into(), serde_json::json!({"command": "true"}))
        .await
        .unwrap();

    assert!(result.contains("(no output)"));
}

#[tokio::test]
async fn timeout_kills_command() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"command": "sleep 10", "timeout": 1}),
        )
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("timed out"));
}
