use rab::agent::extension::Extension;
use rab::builtin::write::WriteExtension;
use tokio_util::sync::CancellationToken;
use yoagent::types::{Content, ToolContext, ToolResult};

fn tool_ctx() -> ToolContext {
    ToolContext {
        tool_call_id: "id".into(),
        tool_name: String::new(),
        cancel: CancellationToken::new(),
        on_update: None,
        on_progress: None,
    }
}

fn text_content(result: &ToolResult) -> String {
    result.content.iter()
        .filter_map(|c| if let Content::Text { text } = c { Some(text.clone()) } else { None })
        .collect::<Vec<_>>().join("")
}

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn writes_file() {
    let tmp = tmp_dir();
    let path = tmp.join("test.txt");

    let ext = WriteExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(
            serde_json::json!({"path": path.to_str().unwrap(), "content": "hello world"}),
            tool_ctx(),
        )
        .await
        .unwrap();
    assert!(text_content(&result).contains("Successfully wrote"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
}

#[tokio::test]
async fn overwrites_existing_file() {
    let tmp = tmp_dir();
    let path = tmp.join("overwrite.txt");
    std::fs::write(&path, "old content").unwrap();

    let ext = WriteExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    tool.execute(
        serde_json::json!({"path": path.to_str().unwrap(), "content": "new content"}),
        tool_ctx(),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
}

#[tokio::test]
async fn creates_parent_directories() {
    let tmp = tmp_dir();
    let path = tmp.join("subdir").join("nested.txt");

    let ext = WriteExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    tool.execute(
        serde_json::json!({"path": path.to_str().unwrap(), "content": "nested file"}),
        tool_ctx(),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested file");
}
