use rab::agent::extension::Extension;
use rab::builtin::read::ReadExtension;
use tokio_util::sync::CancellationToken;
use yoagent::types::{Content, ToolContext, ToolResult};

fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

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
            serde_json::json!({"path": path.to_str().unwrap()}),
            tool_ctx(),
        )
        .await
        .unwrap();
    let txt = text_content(&result);
    assert!(txt.contains("hello world"));
    assert!(txt.contains("line two"));
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
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 5}),
            tool_ctx(),
        )
        .await
        .unwrap();
    let txt = text_content(&result);
    assert!(txt.contains("line 5"), "should contain line 5: {}", txt);
    assert!(
        !txt.lines().any(|l| l == "line 1"),
        "should not contain line 1: {}",
        txt
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
            serde_json::json!({"path": path.to_str().unwrap(), "offset": 1, "limit": 3}),
            tool_ctx(),
        )
        .await
        .unwrap();
    let txt = text_content(&result);
    assert!(txt.contains("line 1"));
    assert!(txt.contains("line 3"));
    assert!(!txt.contains("line 4"));
}

#[tokio::test]
async fn read_nonexistent_file_errors() {
    let tmp = tmp_dir();
    let ext = ReadExtension::new(tmp.clone());
    let tools = ext.tools();
    let tool = &tools[0];

    let result = tool
        .execute(serde_json::json!({"path": "nonexistent.txt"}), tool_ctx())
        .await;
    assert!(result.is_err(), "Expected error for nonexistent file");
}
