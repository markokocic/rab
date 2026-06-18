use rab::builtin::read::ReadExtension;
use rab::extension::Extension;

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
        )
        .await
        .unwrap();

    assert!(result.contains("hello world"));
    assert!(result.contains("line two"));
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
        )
        .await
        .unwrap();

    // Line 5 should be first, line 1-4 should not appear as content
    assert!(
        result.contains("5: line 5"),
        "should contain line 5: {result}"
    );
    assert!(
        !result.contains("1: line 1"),
        "should not contain line 1: {result}"
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
        )
        .await
        .unwrap();

    assert!(result.contains("1: line 1"));
    assert!(result.contains("3: line 3"));
    assert!(!result.contains("4: line 4"));
}

#[tokio::test]
async fn read_nonexistent_file_errors() {
    let tmp = tmp_dir();
    let ext = ReadExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute("id".into(), serde_json::json!({"path": "nonexistent.txt"}))
        .await;
    assert!(result.is_err());
}
