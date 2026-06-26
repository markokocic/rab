use rab::agent::extension::Extension;
use rab::builtin::edit::EditExtension;
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
    result
        .content
        .iter()
        .filter_map(|c| {
            if let Content::Text { text } = c {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn exec_ok(tool: &dyn yoagent::types::AgentTool, args: serde_json::Value) -> String {
    let result = tool.execute(args, tool_ctx()).await.unwrap();
    text_content(&result)
}

async fn exec_err(tool: &dyn yoagent::types::AgentTool, args: serde_json::Value) -> String {
    let result = tool.execute(args, tool_ctx()).await;
    match result {
        Ok(r) => text_content(&r),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn test_basic_edit() {
    let tmp = tmp_dir();
    let path = tmp.join("test.txt");
    std::fs::write(&path, "hello world").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = exec_ok(
        tool.as_ref(),
        serde_json::json!({
            "path": path.to_str().unwrap(),
            "edits": [{"oldText": "hello", "newText": "goodbye"}],
        }),
    )
    .await;
    assert!(
        result.contains("Applied edit") || result.contains("Successfully replaced"),
        "Result: {}",
        result
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye world");
}

#[tokio::test]
async fn test_multiple_edits() {
    let tmp = tmp_dir();
    let path = tmp.join("test.txt");
    std::fs::write(&path, "line one\nline two\nline three\n").unwrap();

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    exec_ok(
        tool.as_ref(),
        serde_json::json!({
            "path": path.to_str().unwrap(),
            "edits": [
                {"oldText": "line one", "newText": "line 1"},
                {"oldText": "line three", "newText": "line 3"},
            ],
        }),
    )
    .await;
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("line 1"), "Content: {}", content);
    assert!(content.contains("line two"), "Content: {}", content);
    assert!(content.contains("line 3"), "Content: {}", content);
    assert!(!content.contains("line one"), "Content: {}", content);
}

#[tokio::test]
async fn test_edit_nonexistent_file() {
    let tmp = tmp_dir();
    let path = tmp.join("nonexistent.txt");

    let ext = EditExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = exec_err(
        tool.as_ref(),
        serde_json::json!({
            "path": path.to_str().unwrap(),
            "edits": [{"oldText": "anything", "newText": "nothing"}],
        }),
    )
    .await;
    assert!(!result.is_empty(), "Expected error message");
}
