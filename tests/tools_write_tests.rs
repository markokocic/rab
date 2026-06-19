use rab::agent::extension::Extension;
use rab::builtin::write::WriteExtension;

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn writes_file() {
    let tmp = tmp_dir();
    let path = tmp.join("output.txt");

    let ext = WriteExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            "id".into(),
            serde_json::json!({"path": path.to_str().unwrap(), "content": "written content"}),
        )
        .await
        .unwrap();

    assert!(result.contains("Successfully wrote"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "written content");
}

#[tokio::test]
async fn creates_parent_directories() {
    let tmp = tmp_dir();
    let path = tmp.join("a").join("b").join("output.txt");

    let ext = WriteExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    tool.execute(
        "id".into(),
        serde_json::json!({"path": path.to_str().unwrap(), "content": "nested"}),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested");
}

#[tokio::test]
async fn overwrites_existing_file() {
    let tmp = tmp_dir();
    let path = tmp.join("output.txt");
    std::fs::write(&path, "old content").unwrap();

    let ext = WriteExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    tool.execute(
        "id".into(),
        serde_json::json!({"path": path.to_str().unwrap(), "content": "new content"}),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
}
