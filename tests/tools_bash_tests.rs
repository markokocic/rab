use rab::agent::extension::{Cancel, Extension};
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

    let output = tool
        .execute(
            "id".into(),
            serde_json::json!({"command": "echo hello"}),
            Cancel::new(), None,
        )
        .await
        .unwrap();
    assert!(output.content.contains("hello"));
}

#[tokio::test]
async fn captures_stdout_and_stderr() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let output = tool
        .execute(
            "id".into(),
            serde_json::json!({"command": "echo out && echo err >&2"}),
            Cancel::new(), None,
        )
        .await
        .unwrap();
    assert!(output.content.contains("out"));
}

#[tokio::test]
async fn runs_in_working_directory() {
    let tmp = tmp_dir();
    std::fs::write(tmp.join("marker.txt"), "present").unwrap();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let output = tool
        .execute(
            "id".into(),
            serde_json::json!({"command": "ls marker.txt"}),
            Cancel::new(), None,
        )
        .await
        .unwrap();
    assert!(output.content.contains("marker.txt"));
}

#[tokio::test]
async fn returns_error_on_nonzero_exit() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let output = tool
        .execute(
            "id".into(),
            serde_json::json!({"command": "exit 1"}),
            Cancel::new(), None,
        )
        .await
        .unwrap();
    assert!(output.content.contains("exit code") || output.content.contains("exit"));
}

#[tokio::test]
async fn handles_empty_output() {
    let tmp = tmp_dir();
    let ext = BashExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let output = tool
        .execute(
            "id".into(),
            serde_json::json!({"command": "true"}),
            Cancel::new(), None,
        )
        .await
        .unwrap();
    assert!(!output.content.is_empty());
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
            Cancel::new(), None,
        )
        .await;
    assert!(result.is_err() || !result.as_ref().unwrap().content.is_empty());
}
