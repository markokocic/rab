//! Print (non-interactive) mode for rab.
//!
//! Runs a single prompt and prints the response to stdout, with tool
//! execution progress reported on stderr.
//!
//! Matches pi's `packages/coding-agent/src/modes/print-mode.ts`.

use std::io::Write;

use crate::provider::compat::RabOpenAiCompat;
use yoagent::types::AgentTool;

/// Run a single-turn agent session in print mode.
///
/// Dispatches to the appropriate yoagent provider based on the model's
/// API protocol, streams text to stdout and thinking/progress to stderr.
pub async fn run_print_mode(
    message: String,
    api_key: String,
    mc: yoagent::provider::model::ModelConfig,
    rab_compat: RabOpenAiCompat,
    system_prompt: String,
    agent_tools: Vec<Box<dyn AgentTool>>,
    agent_session: &mut crate::agent::AgentSession,
) -> anyhow::Result<()> {
    use yoagent::provider::model::ApiProtocol;

    let agent = match mc.api {
        ApiProtocol::OpenAiCompletions => yoagent::agent::Agent::from_provider(
            crate::provider::openai_compat::RabOpenAiCompatProvider::new(rab_compat),
            mc.clone(),
        ),
        ApiProtocol::AnthropicMessages => yoagent::agent::Agent::from_provider(
            crate::provider::anthropic::RabAnthropicProvider,
            mc.clone(),
        ),
        ApiProtocol::OpenAiResponses => yoagent::agent::Agent::from_config(mc.clone()),
        ApiProtocol::GoogleGenerativeAi => yoagent::agent::Agent::from_config(mc.clone()),
        _ => yoagent::agent::Agent::from_config(mc.clone()),
    };
    let mut agent = agent
        .with_api_key(&api_key)
        .with_system_prompt(&system_prompt)
        .with_thinking(yoagent::types::ThinkingLevel::High)
        .with_tools(agent_tools)
        .with_context_config(yoagent::context::ContextConfig::from_context_window(
            mc.context_window,
        ))
        .with_execution_limits(yoagent::context::ExecutionLimits {
            max_total_tokens: usize::MAX,
            max_turns: usize::MAX,
            max_duration: std::time::Duration::from_secs(u64::MAX),
        });

    let (yo_tx, mut yo_rx) = tokio::sync::mpsc::unbounded_channel();
    let msg_for_agent = message.clone();

    // Spawn agent loop (it blocks until done, sending events to yo_tx).
    let agent_handle = tokio::spawn(async move {
        agent.prompt_with_sender(msg_for_agent, yo_tx).await;
    });

    // Persist user prompt via AgentSession
    let rab_prompt = crate::agent::types::user_message(&message);
    agent_session.send_user_message_obj(&rab_prompt);

    let mut thinking_prefix_printed = false;
    const PRINT_MODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    // Process events from yoagent with a timeout.
    loop {
        let event = tokio::time::timeout(PRINT_MODE_TIMEOUT, yo_rx.recv()).await;

        let event = match event {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                agent_handle.abort();
                eprintln!(
                    "{}{}",
                    colored::Colorize::red("✗ "),
                    colored::Colorize::red(
                        "Print mode timed out after 120s — the provider may have hung."
                    )
                );
                break;
            }
        };

        agent_session.on_agent_event(&event);

        match &event {
            yoagent::types::AgentEvent::MessageUpdate { delta, .. } => {
                use yoagent::types::StreamDelta;
                match delta {
                    StreamDelta::Text { delta } => {
                        print!("{}", delta);
                        let _ = std::io::stdout().flush();
                    }
                    StreamDelta::Thinking { delta } => {
                        if !thinking_prefix_printed {
                            eprint!("{}", colored::Colorize::dimmed("… "));
                            thinking_prefix_printed = true;
                        }
                        eprint!("{}", colored::Colorize::dimmed(delta.as_str()));
                        let _ = std::io::stderr().flush();
                    }
                    _ => {}
                }
            }
            yoagent::types::AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                eprintln!(
                    "\n{} {} {}",
                    colored::Colorize::dimmed("⚙"),
                    colored::Colorize::bold(tool_name.as_str()),
                    colored::Colorize::dimmed(
                        serde_json::to_string(args).unwrap_or_default().as_str()
                    )
                );
                thinking_prefix_printed = false;
            }
            yoagent::types::AgentEvent::ToolExecutionEnd {
                result, is_error, ..
            } => {
                let content: String = result
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let yoagent::types::Content::Text { text } = c {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if *is_error {
                    eprintln!(
                        "{} {}",
                        colored::Colorize::red("✗"),
                        colored::Colorize::red(content.as_str())
                    );
                } else {
                    let truncated: String = content.chars().take(500).collect();
                    eprintln!(
                        "{} {}",
                        colored::Colorize::dimmed("✓"),
                        colored::Colorize::dimmed(truncated.as_str())
                    );
                    if content.len() > 500 {
                        eprintln!("{}", colored::Colorize::dimmed("... (truncated)"));
                    }
                }
            }
            yoagent::types::AgentEvent::ProgressMessage {
                text, tool_name, ..
            } => {
                if tool_name.is_empty() {
                    eprint!("{}", text);
                } else {
                    print!("{}", text);
                }
                let _ = std::io::stdout().flush();
            }
            yoagent::types::AgentEvent::AgentEnd { .. } => {
                eprintln!();
            }
            yoagent::types::AgentEvent::MessageEnd { message } => {
                if let Some(err) = crate::agent::types::message_error(message) {
                    let msg = if err.is_empty() {
                        "Provider error: The agent encountered an issue and stopped."
                    } else {
                        err
                    };
                    eprintln!(
                        "{}{}",
                        colored::Colorize::red("✗ "),
                        colored::Colorize::red(msg)
                    );
                } else if crate::agent::types::message_is_system_stop(message) {
                    let text = crate::agent::types::message_text(message);
                    eprintln!(
                        "{}{}",
                        colored::Colorize::red("✗ "),
                        colored::Colorize::red(text.as_str())
                    );
                } else if let Some(text) = crate::agent::types::message_extension_text(message) {
                    eprintln!(
                        "{}{}",
                        colored::Colorize::dimmed("· "),
                        colored::Colorize::dimmed(text.as_str())
                    );
                }
            }
            yoagent::types::AgentEvent::InputRejected { reason } => {
                eprintln!(
                    "{}{}",
                    colored::Colorize::yellow("! "),
                    colored::Colorize::yellow(reason.as_str())
                );
            }
            _ => {}
        }
    }

    // Run auto-compaction if needed
    match agent_session.check_auto_compact().await {
        Ok(true) => eprintln!("{}", colored::Colorize::dimmed("✓ Compaction completed")),
        Ok(false) => {}
        Err(e) => eprintln!(
            "{}",
            colored::Colorize::yellow(format!("Auto-compaction skipped: {}", e).as_str())
        ),
    }

    Ok(())
}
