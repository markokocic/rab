use rab::agent::extension::{Cancel, Extension};
use rab::builtin::read::ReadExtension;

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn reads_file_content() {
    let tmp = tmp_dir();
    let path = tmp.join("test.txt");
    std::fs::write(&path, "hello world\nline two\n").unwrap();

    let ext = ReadExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"path": path.to_str().unwrap()}),
            Cancel::new(),
            None,
        )
        .await
        .unwrap();
    assert!(result.content.contains("hello world"));
    assert!(result.content.contains("line two"));
}

#[tokio::test]
async fn read_respects_offset() {
    let tmp = tmp_dir();
    let path = tmp.join("test.txt");
    let lines: Vec<String> = (1..=10).map(|i| format!("line {}", i)).collect();
    std::fs::write(&path, lines.join("\n")).unwrap();

    let ext = ReadExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 5}),
            Cancel::new(),
            None,
        )
        .await
        .unwrap();

    assert!(
        result.content.contains("line 5"),
        "should contain line 5: {}",
        result.content
    );
    assert!(
        !result.content.lines().any(|l| l == "line 1"),
        "should not contain line 1: {}",
        result.content
    );
}

#[tokio::test]
async fn read_respects_limit() {
    let tmp = tmp_dir();
    let path = tmp.join("test.txt");
    let lines: Vec<String> = (1..=10).map(|i| format!("line {}", i)).collect();
    std::fs::write(&path, lines.join("\n")).unwrap();

    let ext = ReadExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 1, "limit": 3}),
            Cancel::new(),
            None,
        )
        .await
        .unwrap();

    assert!(result.content.contains("line 1"));
    assert!(result.content.contains("line 3"));
    assert!(!result.content.contains("line 4"));
}

#[tokio::test]
async fn read_nonexistent_file_errors() {
    let tmp = tmp_dir();
    let ext = ReadExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"path": "nonexistent.txt"}),
            Cancel::new(),
            None,
        )
        .await;
    assert!(result.is_err());
}
