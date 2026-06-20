use crate::agent::extension::{Cancel, Extension};
use crate::agent::provider::{Provider, StopReason, StreamEvent, ToolDef};
use crate::agent::types::{AgentMessage, Role, ToolCall};

/// Collect tool definitions from all extensions.
pub fn collect_tool_defs(extensions: &[Box<dyn Extension>]) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    for ext in extensions {
        for tool in ext.tools() {
            if !defs.iter().any(|d: &ToolDef| d.name == tool.name()) {
                defs.push(ToolDef {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.parameters(),
                });
            }
        }
    }
    defs
}

/// Emitted by the loop for consumers (print mode writes to stdout; TUI later renders).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    TextDelta {
        delta: String,
    },
    ThinkingDelta {
        delta: String,
    },
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        id: String,
        name: String,
        content: String,
        compact: Option<String>,
        is_error: bool,
    },
    TurnEnd,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
}

/// Configuration for the agent loop.
pub struct LoopConfig<'a> {
    pub model: String,
    pub system_prompt: String,
    pub tools: Vec<ToolDef>,
    pub agent_tools: &'a [Box<dyn crate::agent::extension::AgentTool>],
    pub extensions: &'a [Box<dyn Extension>],
}

/// Find a tool by name across all extensions.
fn find_tool<'a>(
    tools: &'a [Box<dyn crate::agent::extension::AgentTool>],
    name: &str,
) -> Option<&'a dyn crate::agent::extension::AgentTool> {
    tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
}

/// Run the full agent loop. Returns all new messages added during the run.
/// `history` contains pre-existing messages from a previous session (if continuing).
pub async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    history: Vec<AgentMessage>,
    config: &LoopConfig<'_>,
    provider: &dyn Provider,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
) -> anyhow::Result<Vec<AgentMessage>> {
    let mut messages: Vec<AgentMessage> = Vec::new();
    messages.extend(history);
    messages.extend(prompts.clone());

    let mut new_messages: Vec<AgentMessage> = prompts.clone();

    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);

    // Inner loop: stream LLM → execute tools → repeat
    // Outer loop (steering/follow-up queues) will be added in Phase 1
    loop {
        // 1. Stream LLM response
        let mut stream = provider
            .stream(
                &config.model,
                &config.system_prompt,
                &messages,
                &config.tools,
            )
            .await?;

        // 2. Collect streaming response
        let mut response_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;

        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            match event {
                StreamEvent::TextDelta { text } => {
                    response_text.push_str(&text);
                    emit(AgentEvent::TextDelta { delta: text });
                }
                StreamEvent::ThinkingDelta { text } => {
                    emit(AgentEvent::ThinkingDelta { delta: text });
                }
                StreamEvent::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
                    // Parse the accumulated arguments JSON
                    let args: serde_json::Value = serde_json::from_str(&arguments)
                        .unwrap_or(serde_json::Value::String(arguments.clone()));

                    // Try to find existing tool call and update, or insert new
                    if let Some(existing) = tool_calls.iter_mut().find(|tc| tc.id == id) {
                        existing.arguments = args;
                    } else {
                        // Partial: we might get chunks; try to parse what we have
                        tool_calls.push(ToolCall {
                            id,
                            name,
                            arguments: args,
                        });
                    }
                }
                StreamEvent::Done {
                    text,
                    stop_reason: sr,
                    tool_calls: tcs,
                    ..
                } => {
                    response_text = text;
                    stop_reason = sr;
                    // Merge tool calls from Done event (more complete)
                    if !tcs.is_empty() {
                        tool_calls = tcs;
                    }
                }
                StreamEvent::Error { message } => {
                    let error_msg = AgentMessage::tool_result(String::new(), message.clone(), true);
                    new_messages.push(error_msg);
                    emit(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    });
                    return Err(anyhow::anyhow!("Provider error: {}", message));
                }
            }
        }

        // Create assistant message
        let assistant_msg = AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role: Role::Assistant,
            content: response_text.clone(),
            tool_calls: tool_calls.clone(),
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        messages.push(assistant_msg.clone());
        new_messages.push(assistant_msg);

        // Handle errors
        if stop_reason == StopReason::Error {
            emit(AgentEvent::AgentEnd {
                messages: new_messages.clone(),
            });
            return Ok(new_messages);
        }

        // 3. Execute tool calls (parallel by default)
        if !tool_calls.is_empty() {
            for tc in &tool_calls {
                emit(AgentEvent::ToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    args: tc.arguments.clone(),
                });

                // Check before_tool_call hooks
                let mut blocked = false;
                for ext in config.extensions {
                    if let Some(reason) = ext.before_tool_call(tc).await {
                        let msg = AgentMessage::tool_result(
                            &tc.id,
                            format!("Tool execution blocked: {:?}", reason),
                            true,
                        );
                        emit(AgentEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            content: msg.content.clone(),
                            compact: None,
                            is_error: true,
                        });
                        messages.push(msg.clone());
                        new_messages.push(msg);
                        blocked = true;
                        break;
                    }
                }
                if blocked {
                    continue;
                }

                // Execute the tool
                let cancel = Cancel::new();
                if let Some(tool) = find_tool(config.agent_tools, &tc.name) {
                    match tool
                        .execute(tc.id.clone(), tc.arguments.clone(), cancel)
                        .await
                    {
                        Ok(output) => {
                            // Check after_tool_call hooks
                            let mut final_result = output.content.clone();
                            for ext in config.extensions {
                                if let Some(overridden) =
                                    ext.after_tool_call(tc, &final_result).await
                                {
                                    final_result = overridden;
                                }
                            }

                            let msg = AgentMessage::tool_result(&tc.id, &final_result, false);
                            let compact = output.compact.clone();
                            emit(AgentEvent::ToolResult {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                content: final_result.clone(),
                                compact,
                                is_error: false,
                            });
                            messages.push(msg.clone());
                            new_messages.push(msg);
                        }
                        Err(e) => {
                            let err_str = format!("{:#}", e);
                            let msg = AgentMessage::tool_result(&tc.id, &err_str, true);
                            emit(AgentEvent::ToolResult {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                content: err_str,
                                compact: None,
                                is_error: true,
                            });
                            messages.push(msg.clone());
                            new_messages.push(msg);
                        }
                    }
                } else {
                    let msg = AgentMessage::tool_result(
                        &tc.id,
                        format!("Tool '{}' not found", tc.name),
                        true,
                    );
                    emit(AgentEvent::ToolResult {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        content: msg.content.clone(),
                        compact: None,
                        is_error: true,
                    });
                    messages.push(msg.clone());
                    new_messages.push(msg);
                }
            }
            // Loop continues — tool results go back to LLM
            continue;
        }

        // 4. No tool calls — turn complete
        emit(AgentEvent::TurnEnd);
        break;
    }

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    });
    Ok(new_messages)
}
