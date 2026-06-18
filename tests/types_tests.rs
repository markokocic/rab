use rab::types::{AgentMessage, Role};

#[test]
fn user_message_has_correct_role() {
    let msg = AgentMessage::user("hello");
    assert_eq!(msg.role, Role::User);
    assert_eq!(msg.content, "hello");
    assert!(msg.tool_calls.is_empty());
    assert!(msg.tool_call_id.is_none());
    assert!(!msg.is_error);
    assert!(!msg.id.is_empty());
}

#[test]
fn tool_result_message_is_error() {
    let msg = AgentMessage::tool_result("call_1", "something went wrong", true);
    assert_eq!(msg.role, Role::ToolResult);
    assert!(msg.is_error);
    assert_eq!(msg.tool_call_id, Some("call_1".into()));
}

#[test]
fn tool_result_message_is_success() {
    let msg = AgentMessage::tool_result("call_2", "all good", false);
    assert!(!msg.is_error);
}

#[test]
fn messages_have_unique_ids() {
    let a = AgentMessage::user("a");
    let b = AgentMessage::user("b");
    assert_ne!(a.id, b.id);
}

#[test]
fn role_serialization() {
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    assert_eq!(
        serde_json::to_string(&Role::Assistant).unwrap(),
        "\"assistant\""
    );
    assert_eq!(
        serde_json::to_string(&Role::ToolResult).unwrap(),
        "\"toolResult\""
    );
}

#[test]
fn message_roundtrip() {
    let msg = AgentMessage::user("test message");
    let json = serde_json::to_string(&msg).unwrap();
    let back: AgentMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.content, "test message");
    assert_eq!(back.role, Role::User);
}
