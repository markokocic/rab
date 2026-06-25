//! Phase 1 evaluation: test yoagent with the opencode_go / DeepSeek v4 provider.
//!
//! What this test verifies:
//!   1. yoagent compiles alongside rab's existing deps (reqwest 0.12 + 0.13 coexist)
//!   2. TLS works on Android/Termux via native-tls/OpenSSL
//!   3. opencode-go → deepseek-v4-flash responds with streaming text+thinking
//!
//! Run:  cargo test --test yoagent_eval_test -- --nocapture

use std::io::Write;
use std::path::PathBuf;

// ── Hardcoded to the highest available level ──
const THINKING_LEVEL: yoagent::types::ThinkingLevel = yoagent::types::ThinkingLevel::High;

/// Read the opencode-go API key from ~/.rab/agent/auth.json
fn read_opencode_key() -> Option<String> {
    let home = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let path = home.join(".rab").join("agent").join("auth.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    parsed
        .get("opencode-go")
        .and_then(|v| v.get("key"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[tokio::test]
async fn test_yoagent_basic_stream() {
    // ── Auth ──
    let api_key =
        read_opencode_key().expect("No opencode-go API key found in ~/.rab/agent/auth.json");
    eprintln!("✓ API key loaded ({} chars)", api_key.len());

    // ── Model config ──
    // Matches genai's OpenCodeGoAdapter:
    //   Base URL : https://opencode.ai/zen/go/v1/
    //   Endpoint : /chat/completions (OpenAI protocol — deepseek-v4-flash doesn't start with "minimax-")
    //   Auth     : Bearer <api-key>
    let model_config = yoagent::provider::model::ModelConfig::openai_compat(
        "https://opencode.ai/zen/go/v1",
        "deepseek-v4-flash",
        "opencode-go",
        yoagent::provider::model::OpenAiCompat::deepseek(),
    );

    // ── Agent ──
    let mut agent = yoagent::agent::Agent::new(yoagent::provider::OpenAiCompatProvider)
        .with_model("deepseek-v4-flash")
        .with_api_key(&api_key)
        .with_model_config(model_config)
        .with_system_prompt("You are a helpful assistant. Answer concisely.")
        .with_thinking(THINKING_LEVEL)
        .with_max_tokens(256)
        .without_context_management();

    eprintln!("✓ Agent ready, sending prompt...");

    // ── Stream ──
    let mut rx = agent
        .prompt("What is the capital of France? Answer in one word.")
        .await;

    let mut full_text = String::new();
    let mut saw_start = false;
    let mut saw_end = false;

    while let Some(event) = rx.recv().await {
        match event {
            yoagent::types::AgentEvent::AgentStart => {
                saw_start = true;
                eprintln!("→ AgentStart");
            }
            yoagent::types::AgentEvent::MessageUpdate {
                delta: yoagent::types::StreamDelta::Text { delta },
                ..
            } => {
                full_text.push_str(&delta);
                print!("{delta}");
                let _ = std::io::stdout().flush();
            }
            yoagent::types::AgentEvent::MessageUpdate {
                delta: yoagent::types::StreamDelta::Thinking { delta },
                ..
            } => {
                let preview = &delta[..delta.len().min(60)];
                eprint!("[thinking: {preview}]");
                let _ = std::io::stderr().flush();
            }
            yoagent::types::AgentEvent::AgentEnd { .. } => {
                saw_end = true;
                eprintln!("\n→ AgentEnd");
            }
            yoagent::types::AgentEvent::TurnEnd { .. } => {
                eprintln!("→ TurnEnd");
            }
            _ => {} // ignore others for this test
        }
    }

    agent.finish().await;

    // ── Assertions ──
    assert!(saw_start, "Should have received AgentStart");
    assert!(saw_end, "Should have received AgentEnd");
    assert!(!full_text.is_empty(), "Should have received text response");
    assert!(
        full_text.to_lowercase().contains("paris"),
        "Response should mention Paris, got: {full_text}"
    );

    // Log message summary
    let messages = agent.messages();
    eprintln!("\n=== Messages ({}) ===", messages.len());
    for msg in messages {
        if let yoagent::types::AgentMessage::Llm(llm) = msg {
            eprintln!("  role={}  content_len={}", llm.role(), {
                if let yoagent::types::Message::Assistant { content, .. } = llm {
                    content.len()
                } else {
                    0
                }
            });
        }
    }

    eprintln!("\n✓ PASSED — response: {full_text}");
}
