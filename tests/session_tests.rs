use rab::agent::session::SessionManager;
use rab::agent::types::AgentMessage;
use tempfile::TempDir;

// ── Helpers ────────────────────────────────────────────────────────

/// Make an assistant message from text.
fn assistant_msg(content: &str) -> AgentMessage {
    let mut m = AgentMessage::user("dummy");
    m.role = rab::agent::types::Role::Assistant;
    m.content = content.to_string();
    m
}

/// Run a yoagent Agent with a mock provider and return rab messages + events.
async fn run_mock_agent(
    prompts: Vec<AgentMessage>,
    history: Vec<AgentMessage>,
) -> (Vec<AgentMessage>, Vec<rab::agent::provider::AgentEvent>) {
    use tokio::sync::mpsc;
    use yoagent::provider::MockProvider;

    let mut agent = yoagent::agent::Agent::new(MockProvider::text("test response"))
        .with_model("test")
        .with_api_key("test")
        .with_system_prompt("test prompt")
        .with_thinking(yoagent::types::ThinkingLevel::High)
        .without_context_management();

    // Add history messages to the agent
    for m in history {
        let role = match m.role {
            rab::agent::types::Role::User => "user",
            rab::agent::types::Role::Assistant => "assistant",
            rab::agent::types::Role::ToolResult => "toolResult",
        };
        let yo_msg = match role {
            "user" => yoagent::types::AgentMessage::Llm(yoagent::types::Message::User {
                content: vec![yoagent::types::Content::Text { text: m.content }],
                timestamp: 0,
            }),
            "assistant" => yoagent::types::AgentMessage::Llm(yoagent::types::Message::Assistant {
                content: vec![yoagent::types::Content::Text { text: m.content }],
                stop_reason: yoagent::types::StopReason::Stop,
                model: String::new(),
                provider: String::new(),
                usage: yoagent::types::Usage::default(),
                timestamp: 0,
                error_message: None,
            }),
            _ => continue,
        };
        agent.append_message(yo_msg);
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let prompt_text = prompts.into_iter().map(|p| p.content).collect::<Vec<_>>().join(" ");

    // Spawn agent and collect events
    tokio::spawn(async move {
        agent.prompt_with_sender(prompt_text, tx).await;
    });

    let mut rab_events = Vec::new();
    while let Some(event) = rx.recv().await {
        if let Some(rab_event) = rab::agent::yo_bridge::convert_to_rab_event(&event) {
            rab_events.push(rab_event);
        }
    }

    // Extract final messages from AgentEnd
    let final_msgs = rab_events
        .iter()
        .find_map(|e| {
            if let rab::agent::provider::AgentEvent::AgentEnd { messages } = e {
                Some(messages.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    (final_msgs, rab_events)
}

// ── Agent loop tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_agent_loop_with_history() {
    let history = vec![AgentMessage::user("previous question"), assistant_msg("previous answer")];

    let (new_messages, events) = run_mock_agent(vec![AgentMessage::user("new question")], history).await;

    assert!(!new_messages.is_empty(), "Expected at least one message");

    let has_text = events
        .iter()
        .any(|e| matches!(e, rab::agent::provider::AgentEvent::TextDelta { .. }));
    assert!(has_text, "Expected TextDelta events");

    let has_end = events
        .iter()
        .any(|e| matches!(e, rab::agent::provider::AgentEvent::AgentEnd { .. }));
    assert!(has_end, "Expected AgentEnd event");
}

#[tokio::test]
async fn test_agent_loop_no_history() {
    let (new_messages, events) = run_mock_agent(vec![AgentMessage::user("hello")], vec![]).await;

    assert!(!new_messages.is_empty(), "Expected at least one message");

    let has_start = events
        .iter()
        .any(|e| matches!(e, rab::agent::provider::AgentEvent::AgentStart));
    assert!(has_start, "Expected AgentStart event");
}

// ── Session lifecycle integration ──────────────────────────────────

#[tokio::test]
async fn test_session_create_append_continue() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    // Create session and append messages
    let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
    let _session_id = sm.session_id().to_string();
    sm.append_message(&AgentMessage::user("hello"));
    sm.append_message(&assistant_msg("hi there"));
    drop(sm);

    // Small delay for mtime ordering
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Create another session (more recent)
    let mut sm2 = SessionManager::create(&cwd, Some(&sessions_dir));
    let newer_id = sm2.session_id().to_string();
    sm2.append_message(&AgentMessage::user("newer"));
    sm2.append_message(&assistant_msg("newer response"));
    drop(sm2);

    // Continue recent → should get the newer one
    let sm3 = SessionManager::continue_recent(&cwd, Some(&sessions_dir));
    assert_eq!(sm3.session_id(), &newer_id);

    let ctx = sm3.build_session_context();
    assert_eq!(ctx.messages.len(), 2);
}

#[tokio::test]
async fn test_agent_loop_persists_messages_to_session() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    let mut session = SessionManager::create(&cwd, Some(&sessions_dir));

    // Run yoagent agent (returns messages, events)
    let history = session.build_session_context().messages;
    let (new_msgs, _events) = run_mock_agent(vec![AgentMessage::user("run this")], history).await;

    // Persist messages to session (simulating TUI AgentEnd handler)
    for msg in &new_msgs {
        session.append_message(msg);
    }

    // Verify messages are in session
    let ctx = session.build_session_context();
    assert!(
        ctx.messages.len() >= 2,
        "Expected at least 2 messages (user + assistant), got {}",
        ctx.messages.len()
    );
    assert_eq!(ctx.messages[0].role, rab::agent::types::Role::User);
    assert_eq!(ctx.messages[0].content, "run this");

    // Verify file was created and contains entries
    let file_path = session.session_file().unwrap();
    assert!(file_path.exists(), "Session file should exist");

    let entries = rab::agent::session::load_entries_from_file(&file_path);
    assert!(!entries.is_empty(), "Session file should contain entries");
}

#[tokio::test]
async fn test_agent_loop_persists_with_history() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    // Create session with prior messages
    let mut session = SessionManager::create(&cwd, Some(&sessions_dir));
    session.append_message(&AgentMessage::user("previous question"));
    session.append_message(&assistant_msg("previous answer"));

    let file_path = session.session_file().unwrap().to_path_buf();
    let original_count = session.entries().len();
    drop(session);

    // Reopen and run agent loop with history
    let mut session = SessionManager::open(&file_path, Some(&sessions_dir), None);
    let history = session.build_session_context().messages;
    assert_eq!(history.len(), 2, "History should have 2 messages");

    let (new_msgs, _events) = run_mock_agent(vec![AgentMessage::user("new question")], history).await;

    // Persist — simulating TUI AgentEnd
    for msg in &new_msgs {
        session.append_message(msg);
    }

    // Context should have original 2 + new messages
    let ctx = session.build_session_context();
    assert!(
        ctx.messages.len() > original_count,
        "Context should grow: {} > {}",
        ctx.messages.len(),
        original_count
    );

    // Verify log file has the persisted messages
    let current_path = session.session_file().unwrap();
    assert!(current_path.exists());
}
