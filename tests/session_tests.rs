use rab::agent::{self, AgentEvent, LoopConfig};
use rab::agent::extension::Extension;
use rab::agent::session::SessionManager;
use tempfile::TempDir;

// ── Helpers ────────────────────────────────────────────────────────

struct NoopProvider;

#[async_trait::async_trait]
impl rab::agent::provider::Provider for NoopProvider {
    async fn stream(
        &self,
        _model: &str,
        _system_prompt: &str,
        _messages: &[rab::agent::types::AgentMessage],
        _tools: &[rab::agent::provider::ToolDef],
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = rab::agent::provider::StreamEvent> + Send>>,
    > {
        // Return a stream that immediately sends Done with a simple response
        let events: Vec<rab::agent::provider::StreamEvent> = vec![
            rab::agent::provider::StreamEvent::TextDelta {
                text: "test response".to_string(),
            },
            rab::agent::provider::StreamEvent::Done {
                text: "test response".to_string(),
                usage: Default::default(),
                stop_reason: rab::agent::provider::StopReason::EndTurn,
                tool_calls: vec![],
            },
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

fn empty_extensions() -> Vec<Box<dyn Extension>> {
    vec![]
}

fn empty_tools() -> Vec<Box<dyn rab::agent::extension::AgentTool>> {
    vec![]
}

// ── Agent loop with history ────────────────────────────────────────

#[tokio::test]
async fn test_agent_loop_with_history() {
    let history = vec![rab::agent::types::AgentMessage::user("previous question"), {
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "previous answer".to_string();
        m
    }];

    let prompt = rab::agent::types::AgentMessage::user("new question");

    let config = LoopConfig {
        model: "test".to_string(),
        system_prompt: "test prompt".to_string(),
        tools: vec![],
        agent_tools: &empty_tools(),
        extensions: &empty_extensions(),
    };

    let provider = NoopProvider;
    let mut events: Vec<AgentEvent> = Vec::new();
    let mut emit = |event: AgentEvent| {
        events.push(event);
    };

    let new_messages = agent::run_agent_loop(
        vec![prompt.clone()],
        history.clone(),
        &config,
        &provider,
        &mut emit,
    )
    .await
    .unwrap();

    // Should have the new prompt + assistant response
    assert!(!new_messages.is_empty());

    // Check that we got events
    let has_text = events
        .iter()
        .any(|e| matches!(e, AgentEvent::TextDelta { .. }));
    assert!(has_text, "Expected TextDelta events");

    let has_end = events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. }));
    assert!(has_end, "Expected AgentEnd event");
}

#[tokio::test]
async fn test_agent_loop_no_history() {
    let prompt = rab::agent::types::AgentMessage::user("hello");

    let config = LoopConfig {
        model: "test".to_string(),
        system_prompt: "test prompt".to_string(),
        tools: vec![],
        agent_tools: &empty_tools(),
        extensions: &empty_extensions(),
    };

    let provider = NoopProvider;
    let mut events: Vec<AgentEvent> = Vec::new();
    let mut emit = |event: AgentEvent| {
        events.push(event);
    };

    let new_messages = agent::run_agent_loop(
        vec![prompt.clone()],
        vec![], // no history
        &config,
        &provider,
        &mut emit,
    )
    .await
    .unwrap();

    assert!(!new_messages.is_empty());

    let has_start = events.iter().any(|e| matches!(e, AgentEvent::AgentStart));
    assert!(has_start);
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
    sm.append_message(&rab::agent::types::AgentMessage::user("hello"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "hi there".to_string();
        m
    });
    drop(sm);

    // Small delay for mtime ordering
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Create another session (more recent)
    let mut sm2 = SessionManager::create(&cwd, Some(&sessions_dir));
    let newer_id = sm2.session_id().to_string();
    sm2.append_message(&rab::agent::types::AgentMessage::user("newer"));
    sm2.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "newer response".to_string();
        m
    });
    drop(sm2);

    // Continue recent → should get the newer one
    let sm3 = SessionManager::continue_recent(&cwd, Some(&sessions_dir));
    assert_eq!(sm3.session_id(), &newer_id);

    let ctx = sm3.build_session_context();
    assert_eq!(ctx.messages.len(), 2);
    assert_eq!(ctx.messages[0].content, "newer");
}

#[tokio::test]
async fn test_session_open_append_more() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    // Create session with initial messages
    let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
    sm.append_message(&rab::agent::types::AgentMessage::user("first"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "first response".to_string();
        m
    });

    let file_path = sm.session_file().unwrap().to_path_buf();
    let session_id = sm.session_id().to_string();
    drop(sm);

    // Open and append more
    let mut sm2 = SessionManager::open(&file_path, Some(&sessions_dir), None);
    assert_eq!(sm2.session_id(), &session_id);

    sm2.append_message(&rab::agent::types::AgentMessage::user("second"));
    sm2.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "second response".to_string();
        m
    });

    let ctx = sm2.build_session_context();
    assert_eq!(ctx.messages.len(), 4);
    assert_eq!(ctx.messages[0].content, "first");
    assert_eq!(ctx.messages[2].content, "second");
}

#[tokio::test]
async fn test_session_name_persistence() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
    assert!(sm.session_name().is_none());

    sm.append_session_info("Bug fix session");
    sm.append_message(&rab::agent::types::AgentMessage::user("hello"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "ok".to_string();
        m
    });

    assert_eq!(sm.session_name(), Some("Bug fix session"));

    // Clear name
    sm.append_session_info("");
    assert!(sm.session_name().is_none());

    // Set another name
    sm.append_session_info("Refactor session");
    assert_eq!(sm.session_name(), Some("Refactor session"));

    let file_path = sm.session_file().unwrap().to_path_buf();
    drop(sm);

    // Reopen and verify name persisted
    let sm2 = SessionManager::open(&file_path, Some(&sessions_dir), None);
    assert_eq!(sm2.session_name(), Some("Refactor session"));
}

#[tokio::test]
async fn test_session_thinking_and_model_changes() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
    sm.append_thinking_level_change("high");
    sm.append_message(&rab::agent::types::AgentMessage::user("hello"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "ok".to_string();
        m
    });
    sm.append_model_change("opencode_go", "deepseek-v4-pro");

    let file_path = sm.session_file().unwrap().to_path_buf();
    drop(sm);

    // Reopen and verify
    let sm2 = SessionManager::open(&file_path, Some(&sessions_dir), None);
    let entries = sm2.entries();
    assert_eq!(entries.len(), 4); // thinking + user + assistant + model

    match &entries[0] {
        rab::agent::session::SessionEntry::ThinkingLevelChange(e) => {
            assert_eq!(e.thinking_level, "high");
        }
        other => panic!("Expected ThinkingLevelChange, got {:?}", other),
    }

    match &entries[3] {
        rab::agent::session::SessionEntry::ModelChange(e) => {
            assert_eq!(e.provider, "opencode_go");
            assert_eq!(e.model_id, "deepseek-v4-pro");
        }
        other => panic!("Expected ModelChange, got {:?}", other),
    }
}

#[tokio::test]
async fn test_session_branching() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));

    // First branch
    let m1 = sm.append_message(&rab::agent::types::AgentMessage::user("question 1"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "answer 1".to_string();
        m
    });

    // Second turn
    sm.append_message(&rab::agent::types::AgentMessage::user("question 2"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "answer 2".to_string();
        m
    });

    // Branch back to after first user message
    sm.set_branch(&m1).unwrap();

    // Append alternate path
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "alternate answer 1".to_string();
        m
    });

    // Context from current leaf should only have branch path
    let ctx = sm.build_session_context();
    assert_eq!(ctx.messages.len(), 2); // user "question 1" + alternate asst
    assert_eq!(ctx.messages[0].content, "question 1");
    assert_eq!(ctx.messages[1].content, "alternate answer 1");
}

#[tokio::test]
async fn test_session_compaction_entry() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
    sm.append_message(&rab::agent::types::AgentMessage::user("old stuff"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "old response".to_string();
        m
    });

    sm.append_compaction("Earlier conversation summarized", "entry_kept_id", 1000);
    sm.append_message(&rab::agent::types::AgentMessage::user("new stuff"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "new response".to_string();
        m
    });

    let file_path = sm.session_file().unwrap().to_path_buf();
    drop(sm);

    // Reopen
    let sm2 = SessionManager::open(&file_path, Some(&sessions_dir), None);
    let entries = sm2.entries();
    assert_eq!(entries.len(), 5);

    let has_compaction = entries
        .iter()
        .any(|e| matches!(e, rab::agent::session::SessionEntry::Compaction(_)));
    assert!(has_compaction, "Expected compaction entry");
}

#[tokio::test]
async fn test_session_in_memory() {
    let cwd = std::path::Path::new("/tmp/test-in-memory");
    let mut sm = SessionManager::in_memory(cwd);

    assert!(!sm.is_persisted());
    assert!(!sm.session_id().is_empty());

    sm.append_message(&rab::agent::types::AgentMessage::user("test"));

    let ctx = sm.build_session_context();
    assert_eq!(ctx.messages.len(), 1);
    assert_eq!(ctx.messages[0].content, "test");

    // No file should be created
    assert!(sm.session_file().is_none());
}

#[tokio::test]
async fn test_agent_loop_persists_messages_to_session() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    let mut session = SessionManager::create(&cwd, Some(&sessions_dir));

    // Run agent loop — this simulates what the TUI does on submit
    let prompt = rab::agent::types::AgentMessage::user("run this");
    let history = session.build_session_context().messages;

    let config = LoopConfig {
        model: "test".to_string(),
        system_prompt: "test".to_string(),
        tools: vec![],
        agent_tools: &empty_tools(),
        extensions: &empty_extensions(),
    };

    let provider = NoopProvider;
    let mut events = Vec::new();
    let new_messages =
        rab::agent::run_agent_loop(vec![prompt], history, &config, &provider, &mut |e| {
            events.push(e)
        })
        .await
        .unwrap();

    // Persist new messages (simulating what TUI's AgentEnd handler does)
    for msg in &new_messages {
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

    let entries = rab::agent::session::load_entries_from_file(file_path);
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
    session.append_message(&rab::agent::types::AgentMessage::user("previous question"));
    session.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "previous answer".to_string();
        m
    });

    let file_path = session.session_file().unwrap().to_path_buf();
    let original_count = session.entries().len();
    drop(session);

    // Reopen and run agent loop with history
    let mut session = SessionManager::open(&file_path, Some(&sessions_dir), None);
    let history = session.build_session_context().messages;
    assert_eq!(history.len(), 2, "History should have 2 messages");

    let prompt = rab::agent::types::AgentMessage::user("new question");

    let config = LoopConfig {
        model: "test".to_string(),
        system_prompt: "test".to_string(),
        tools: vec![],
        agent_tools: &empty_tools(),
        extensions: &empty_extensions(),
    };

    let provider = NoopProvider;
    let new_messages =
        rab::agent::run_agent_loop(vec![prompt], history, &config, &provider, &mut |_| {})
            .await
            .unwrap();

    // Persist — simulating TUI AgentEnd
    for msg in &new_messages {
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
    assert_eq!(ctx.messages[0].content, "previous question");
    assert_eq!(ctx.messages[1].content, "previous answer");
    assert_eq!(ctx.messages[2].content, "new question");
}

#[tokio::test]
async fn test_session_label_and_custom() {
    let tmp = TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    let mut sm = SessionManager::create(&cwd, Some(&sessions_dir));
    let msg_id = sm.append_message(&rab::agent::types::AgentMessage::user("important"));
    sm.append_message(&{
        let mut m = rab::agent::types::AgentMessage::user("dummy");
        m.role = rab::agent::types::Role::Assistant;
        m.content = "ack".to_string();
        m
    });

    // Set label
    sm.append_label_change(&msg_id, Some("critical"));
    assert_eq!(sm.label(&msg_id), Some("critical"));

    // Add custom entry
    sm.append_custom_entry("my_ext", serde_json::json!({"key": "val"}));

    // Branch summary
    sm.append_branch_summary("root", "Abandoned path");

    let entries = sm.entries();
    let has_label = entries
        .iter()
        .any(|e| matches!(e, rab::agent::session::SessionEntry::Label(_)));
    assert!(has_label);

    let has_custom = entries
        .iter()
        .any(|e| matches!(e, rab::agent::session::SessionEntry::Custom(_)));
    assert!(has_custom);

    let has_summary = entries
        .iter()
        .any(|e| matches!(e, rab::agent::session::SessionEntry::BranchSummary(_)));
    assert!(has_summary);
}
