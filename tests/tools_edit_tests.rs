use rab::builtin::edit::EditExtension;
use rab::extension::Extension;

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn single_edit_replaces_text() {
    let tmp = tmp_dir();
    let path = tmp.join("file.txt");
    std::fs::write(&path, "hello world\nfoo bar\n").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    tool.execute(
        "id".into(),
        serde_json::json!({
            "path": path.to_str().unwrap(),
            "edits": [{"oldText": "foo bar", "newText": "baz qux"}]
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "hello world\nbaz qux\n"
    );
}

#[tokio::test]
async fn multiple_edits_replaces_all() {
    let tmp = tmp_dir();
    let path = tmp.join("file.txt");
    std::fs::write(&path, "aaa\nbbb\nccc\n").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    tool.execute(
        "id".into(),
        serde_json::json!({
            "path": path.to_str().unwrap(),
            "edits": [
                {"oldText": "aaa", "newText": "111"},
                {"oldText": "ccc", "newText": "333"}
            ]
        }),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "111\nbbb\n333\n");
}

#[tokio::test]
async fn non_unique_oldtext_errors() {
    let tmp = tmp_dir();
    let path = tmp.join("file.txt");
    std::fs::write(&path, "dup\ndup\n").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "dup", "newText": "x"}]
            }),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn missing_oldtext_errors() {
    let tmp = tmp_dir();
    let path = tmp.join("file.txt");
    std::fs::write(&path, "content\n").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [{"oldText": "not found", "newText": "x"}]
            }),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn overlapping_edits_error() {
    let tmp = tmp_dir();
    let path = tmp.join("file.txt");
    std::fs::write(&path, "abcdef\n").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({
                "path": path.to_str().unwrap(),
                "edits": [
                    {"oldText": "abc", "newText": "1"},
                    {"oldText": "bcd", "newText": "2"}
                ]
            }),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn empty_edits_errors() {
    let tmp = tmp_dir();
    let path = tmp.join("file.txt");
    std::fs::write(&path, "content\n").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"path": path.to_str().unwrap(), "edits": []}),
        )
        .await;
    assert!(result.is_err());
}
