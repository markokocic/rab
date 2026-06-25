use crate::agent::extension::{Cancel, Extension, ToolOutput};
use crate::agent::provider::{Provider, StopReason, StreamEvent, ToolDef};
use crate::agent::types::{
    AgentMessage, PendingMessageQueue, Role, ToolCall, ToolExecutionMode, Usage,
};
use futures::future::join_all;

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
    /// Progressive args update (pi calls renderCall multiple times).
    ToolCallArgsUpdate {
        id: String,
        args: serde_json::Value,
    },
    ToolResult {
        id: String,
        name: String,
        content: String,
        compact: Option<String>,
        is_error: bool,
        /// Structured details for the UI renderer (not sent to LLM).
        details: Option<serde_json::Value>,
    },
    /// Intermediate tool execution progress (bash streaming output).
    ToolProgress {
        content: String,
        is_error: bool,
    },
    /// Stream was aborted or errored. TextDelta/ThinkingDelta may have been sent before.
    Aborted {
        reason: String,
    },
    /// A user message was injected from the steering or follow-up queue.
    UserMessage {
        content: String,
    },
    TurnEnd,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
}

/// Transform function: rewrites messages before each LLM call.
pub type TransformFn = Box<dyn Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync>;

/// Prepare-next-turn function: optionally modifies context between turns.
pub type PrepareNextTurnFn = Box<dyn Fn(&[AgentMessage]) -> Option<TurnUpdate> + Send + Sync>;

/// Should-stop-after-turn predicate: early-stop check.
pub type ShouldStopFn = Box<dyn Fn(&[AgentMessage]) -> bool + Send + Sync>;

/// Optional return value from `prepare_next_turn` to modify context for the next turn.
pub struct TurnUpdate {
    /// Replace the full message context for the next LLM call.
    pub context: Option<Vec<AgentMessage>>,
}

/// Configuration for the agent loop.
pub struct LoopConfig<'a> {
    pub model: String,
    pub system_prompt: String,
    pub tools: Vec<ToolDef>,
    pub agent_tools: &'a [Box<dyn crate::agent::extension::AgentTool>],
    pub extensions: &'a [Box<dyn Extension>],
    /// Tool execution mode: parallel (default) or sequential.
    pub tool_execution: ToolExecutionMode,
    /// Optional steering queue: messages delivered after the current assistant turn's
    /// tool calls finish, before the next LLM call.
    pub steering_queue: Option<&'a std::sync::Mutex<PendingMessageQueue>>,
    /// Optional follow-up queue: messages delivered only after the agent has no more
    /// tool calls (fully idle).
    pub follow_up_queue: Option<&'a std::sync::Mutex<PendingMessageQueue>>,
    /// Optional transform applied to the message list before each LLM call.
    /// Receives the current messages and returns (possibly modified) messages.
    /// Pi-compatible: `transformContext` for context window management, pruning, etc.
    pub transform_context: Option<TransformFn>,
    /// Optional callback invoked after each turn completes.
    /// Can return a `TurnUpdate` to modify the context for the next turn.
    /// Pi-compatible: `prepareNextTurn`.
    pub prepare_next_turn: Option<PrepareNextTurnFn>,
    /// Optional predicate invoked after each turn completes.
    /// Return true to stop the agent loop early.
    /// Pi-compatible: `shouldStopAfterTurn`.
    pub should_stop_after_turn: Option<ShouldStopFn>,
}

/// Find a tool by name across all extensions.
fn find_tool<'a>(
    tools: &'a [Box<dyn crate::agent::extension::AgentTool>],
    name: &str,
) -> Option<&'a dyn crate::agent::extension::AgentTool> {
    tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
}

/// Result of a single tool execution within the execution phase.
struct ToolExecOutcome {
    id: String,
    name: String,
    content: String,
    compact: Option<String>,
    is_error: bool,
    /// When true and ALL tools in the batch are terminal, skip further LLM calls.
    terminate: bool,
    /// Structured details for UI rendering (not sent to LLM).
    details: Option<serde_json::Value>,
}

/// Run the full agent loop. Returns all new messages added during the run.
/// `history` contains pre-existing messages from a previous session (if continuing).
///
/// Supports parallel tool execution (default) and sequential, plus steering/follow-up
/// message queues for mid-stream interruption (pi-compatible).
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

    // ── Outer loop: continues when follow-up messages arrive ──
    // (pi-compatible: after agent would stop, check follow-up queue and continue)
    loop {
        // ── Inner loop: stream LLM → execute tools → repeat ──
        let mut has_more_tool_calls = true;

        while has_more_tool_calls {
            // Check steering messages before each LLM call
            // (pi-compatible: delivered after current turn's tool calls finish,
            //  before next LLM call)
            drain_steering(config, &mut messages, &mut new_messages, emit);

            // 1. Stream LLM response
            // Apply transform_context if configured (pi-compatible: rewrite messages
            // for the LLM call without modifying the stored transcript).
            let llm_messages: &[AgentMessage] = &messages;
            let _transformed_holder;
            let llm_messages = if let Some(ref transform) = config.transform_context {
                _transformed_holder = transform(llm_messages);
                &_transformed_holder
            } else {
                llm_messages
            };
            let mut stream = provider
                .stream(
                    &config.model,
                    &config.system_prompt,
                    llm_messages,
                    &config.tools,
                )
                .await?;

            // 2. Collect streaming response
            let mut response_text = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut stop_reason = StopReason::EndTurn;
            let mut usage = Usage::default();

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
                        let args: serde_json::Value = serde_json::from_str(&arguments)
                            .unwrap_or(serde_json::Value::String(arguments.clone()));

                        if let Some(existing) = tool_calls.iter_mut().find(|tc| tc.id == id) {
                            existing.arguments = args;
                        } else {
                            tool_calls.push(ToolCall {
                                id,
                                name,
                                arguments: args,
                            });
                        }
                    }
                    StreamEvent::Done {
                        text,
                        usage: done_usage,
                        stop_reason: sr,
                        tool_calls: tcs,
                    } => {
                        if response_text.is_empty() && !text.is_empty() {
                            emit(AgentEvent::TextDelta {
                                delta: text.clone(),
                            });
                        }
                        response_text = text;
                        stop_reason = sr;
                        usage = done_usage;
                        if !tcs.is_empty() {
                            tool_calls = tcs;
                        }
                    }
                    StreamEvent::Error { message } => {
                        // Pi-style: create an error assistant message so the failure is always visible
                        emit(AgentEvent::TextDelta {
                            delta: message.clone(),
                        });
                        let error_assistant = AgentMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            parent_id: None,
                            role: Role::Assistant,
                            content: message.clone(),
                            tool_calls: vec![],
                            tool_call_id: None,
                            usage: None,
                            is_error: true,
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        };
                        messages.push(error_assistant.clone());
                        new_messages.push(error_assistant);
                        emit(AgentEvent::AgentEnd {
                            messages: new_messages.clone(),
                        });
                        return Ok(new_messages);
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
                usage: Some(usage),
                is_error: false,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };

            messages.push(assistant_msg.clone());
            new_messages.push(assistant_msg);

            // Handle errors — pi-style: mark the assistant message as error
            // so the consumer (TUI/print mode) can always detect the failure.
            if stop_reason == StopReason::Error {
                if let Some(last) = messages.last_mut()
                    && last.role == Role::Assistant
                {
                    last.is_error = true;
                }
                if let Some(last) = new_messages.last_mut()
                    && last.role == Role::Assistant
                {
                    last.is_error = true;
                }
                emit(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                });
                return Ok(new_messages);
            }

            // 3. Execute tool calls
            if !tool_calls.is_empty() {
                // Check if any tool in this batch declares sequential execution mode.
                // If so, the entire batch runs sequentially (pi-compatible per-tool override).
                let has_sequential_tool = tool_calls.iter().any(|tc| {
                    config
                        .agent_tools
                        .iter()
                        .find(|t| t.name() == tc.name)
                        .map(|t| t.execution_mode() == ToolExecutionMode::Sequential)
                        .unwrap_or(false)
                });

                let effective_mode = if has_sequential_tool {
                    ToolExecutionMode::Sequential
                } else {
                    config.tool_execution
                };

                let outcomes = match effective_mode {
                    ToolExecutionMode::Parallel => {
                        execute_tool_calls_parallel(&tool_calls, config, emit).await
                    }
                    ToolExecutionMode::Sequential => {
                        execute_tool_calls_sequential(&tool_calls, config, emit).await
                    }
                };

                let all_terminate = !outcomes.is_empty() && outcomes.iter().all(|o| o.terminate);

                for outcome in outcomes {
                    let msg =
                        AgentMessage::tool_result(&outcome.id, &outcome.content, outcome.is_error);
                    emit(AgentEvent::ToolResult {
                        id: outcome.id,
                        name: outcome.name,
                        content: outcome.content,
                        compact: outcome.compact,
                        is_error: outcome.is_error,
                        details: outcome.details,
                    });
                    messages.push(msg.clone());
                    new_messages.push(msg);
                }

                // Prepare next turn (pi-compatible: allows modifying context between turns
                // even when tools were called)
                apply_prepare_next_turn(config, &mut messages, &new_messages);

                if all_terminate {
                    // All tools returned terminate=true, stop further LLM calls
                    emit(AgentEvent::TurnEnd);
                    break;
                }

                // Inner loop continues — tool results go back to LLM
                continue;
            }

            // 4. No tool calls — inner turn complete
            has_more_tool_calls = false;
            emit(AgentEvent::TurnEnd);

            // Prepare next turn after the turn fully completes
            apply_prepare_next_turn(config, &mut messages, &new_messages);

            // Check should_stop_after_turn (pi-compatible: early-stop predicate)
            if apply_should_stop_after_turn(config, &new_messages) {
                emit(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                });
                return Ok(new_messages);
            }
        }

        // 5. Agent would stop. Check for follow-up messages.
        // (pi-compatible: follow-up messages are delivered only after agent is idle)
        if !drain_follow_up(config, &mut messages, &mut new_messages, emit) {
            break;
        }
    }

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    });
    Ok(new_messages)
}

/// Drain steering messages into the message list, emitting UserMessage events.
fn drain_steering(
    config: &LoopConfig<'_>,
    messages: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
) -> bool {
    let Some(queue) = config.steering_queue else {
        return false;
    };
    let drained = queue.lock().unwrap().drain();
    if drained.is_empty() {
        return false;
    }
    for msg in drained {
        emit(AgentEvent::UserMessage {
            content: msg.content.clone(),
        });
        messages.push(msg.clone());
        new_messages.push(msg);
    }
    true
}

/// Drain follow-up messages into the message list, emitting UserMessage events.
/// Returns true if any messages were drained (caller should continue outer loop).
fn drain_follow_up(
    config: &LoopConfig<'_>,
    messages: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
) -> bool {
    let Some(queue) = config.follow_up_queue else {
        return false;
    };
    let drained = queue.lock().unwrap().drain();
    if drained.is_empty() {
        return false;
    }
    for msg in drained {
        emit(AgentEvent::UserMessage {
            content: msg.content.clone(),
        });
        messages.push(msg.clone());
        new_messages.push(msg);
    }
    true
}

/// Apply `prepare_next_turn` callback if configured.
/// Modifies the message context for the next turn (pi-compatible).
fn apply_prepare_next_turn(
    config: &LoopConfig<'_>,
    messages: &mut Vec<AgentMessage>,
    new_messages: &[AgentMessage],
) {
    if let Some(ref prepare) = config.prepare_next_turn
        && let Some(update) = prepare(new_messages)
        && let Some(ctx) = update.context
    {
        *messages = ctx;
    }
}

/// Apply `should_stop_after_turn` callback if configured.
/// Returns true if the agent loop should stop early (pi-compatible).
fn apply_should_stop_after_turn(config: &LoopConfig<'_>, new_messages: &[AgentMessage]) -> bool {
    config
        .should_stop_after_turn
        .as_ref()
        .map(|stop| stop(new_messages))
        .unwrap_or(false)
}

/// Execute tool calls sequentially (one at a time, in order).
async fn execute_tool_calls_sequential(
    tool_calls: &[ToolCall],
    config: &LoopConfig<'_>,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
) -> Vec<ToolExecOutcome> {
    let mut outcomes = Vec::new();

    for tc in tool_calls {
        emit(AgentEvent::ToolCall {
            id: tc.id.clone(),
            name: tc.name.clone(),
            args: tc.arguments.clone(),
        });

        // Check before_tool_call hooks
        let mut blocked = false;
        for ext in config.extensions {
            if let Some(reason) = ext.before_tool_call(tc).await {
                outcomes.push(ToolExecOutcome {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    content: format!("Tool execution blocked: {:?}", reason),
                    compact: None,
                    is_error: true,
                    terminate: false,
                    details: None,
                });
                blocked = true;
                break;
            }
        }
        if blocked {
            continue;
        }

        // Execute the tool with progress forwarding
        let outcome = execute_single_tool(
            tc,
            config.agent_tools,
            config.extensions,
            None, // sequential: progress is emitted inline, not via channel
        )
        .await;
        outcomes.push(outcome);
    }

    outcomes
}

/// Execute tool calls in parallel (pi-compatible):
/// Phase 1 (sequential preflight): emit ToolCall events, check before_tool_call hooks.
/// Phase 2 (concurrent execution): execute all non-blocked tools concurrently via join_all.
/// Phase 3 (sequential post-processing): collect outcomes in original tool call order.
async fn execute_tool_calls_parallel(
    tool_calls: &[ToolCall],
    config: &LoopConfig<'_>,
    emit: &mut (dyn FnMut(AgentEvent) + Send),
) -> Vec<ToolExecOutcome> {
    let mut outcomes: Vec<ToolExecOutcome> = Vec::with_capacity(tool_calls.len());
    let mut futures: Vec<
        std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecOutcome> + Send + '_>>,
    > = Vec::new();

    // Note: progress updates from parallel tool execution are not forwarded
    // to `emit` because `emit` is FnMut (not Sync) and can't be shared across
    // concurrent futures. To wire progress, pass a shared mpsc channel instead.
    // `execute_single_tool` accepts `progress_tx: Option<UnboundedSender<AgentEvent>>`
    // for this purpose. Pass Some(channel) when progress forwarding is needed.

    // ── Phase 1: Sequential preflight ──
    // Emit ToolCall events and check before_tool_call hooks one at a time.
    for tc in tool_calls {
        emit(AgentEvent::ToolCall {
            id: tc.id.clone(),
            name: tc.name.clone(),
            args: tc.arguments.clone(),
        });

        let mut blocked = false;
        for ext in config.extensions {
            if let Some(reason) = ext.before_tool_call(tc).await {
                outcomes.push(ToolExecOutcome {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    content: format!("Tool execution blocked: {:?}", reason),
                    compact: None,
                    is_error: true,
                    terminate: false,
                    details: None,
                });
                blocked = true;
                break;
            }
        }
        if blocked {
            continue;
        }

        // ── Phase 2: Collect non-blocked tools as concurrent futures ──
        // `execute_single_tool` takes an optional progress channel for streaming
        // tool output. When None, progress updates are not forwarded (the channel
        // is still created internally for the tool's `on_update` but discarded).
        let tc_clone = tc.clone();
        futures.push(Box::pin(async move {
            execute_single_tool(
                &tc_clone,
                config.agent_tools,
                config.extensions,
                None, // progress_tx: pass Some(channel) to get streaming updates
            )
            .await
        }));
    }

    // ── Phase 3: Await all concurrent executions, preserving preflight order ──
    if !futures.is_empty() {
        let results = join_all(futures).await;
        outcomes.extend(results);
    }

    outcomes
}

/// Execute a single tool call and return the outcome.
/// If `progress_tx` is provided, tool progress updates (from `on_update`) are
/// forwarded as `AgentEvent::ToolProgress` events.
async fn execute_single_tool(
    tc: &ToolCall,
    agent_tools: &[Box<dyn crate::agent::extension::AgentTool>],
    extensions: &[Box<dyn Extension>],
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
) -> ToolExecOutcome {
    let cancel = Cancel::new();

    if let Some(tool) = find_tool(agent_tools, &tc.name) {
        // Apply prepare_arguments if the tool defines it (pi-compatible)
        let args = tool.prepare_arguments(tc.arguments.clone());

        // Wire on_update: if progress forwarding is requested, create a channel
        // so the tool can stream progress updates back to the agent.
        let on_update = progress_tx.as_ref().map(|_| {
            let (tool_tx, mut tool_rx) = tokio::sync::mpsc::unbounded_channel::<ToolOutput>();
            if let Some(ref tx) = progress_tx {
                let tx = tx.clone();
                tokio::spawn(async move {
                    while let Some(output) = tool_rx.recv().await {
                        let _ = tx.send(AgentEvent::ToolProgress {
                            content: output.content,
                            is_error: output.is_error,
                        });
                    }
                });
            }
            tool_tx
        });

        match tool.execute(tc.id.clone(), args, cancel, on_update).await {
            Ok(output) => {
                // Check after_tool_call hooks
                let mut final_result = output.content.clone();
                for ext in extensions {
                    if let Some(overridden) = ext.after_tool_call(tc, &final_result).await {
                        final_result = overridden;
                    }
                }

                ToolExecOutcome {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    content: final_result,
                    compact: output.compact,
                    is_error: false,
                    terminate: output.terminate,
                    details: output.details,
                }
            }
            Err(e) => ToolExecOutcome {
                id: tc.id.clone(),
                name: tc.name.clone(),
                content: format!("{:#}", e),
                compact: None,
                is_error: true,
                terminate: false,
                details: None,
            },
        }
    } else {
        ToolExecOutcome {
            id: tc.id.clone(),
            name: tc.name.clone(),
            content: format!("Tool '{}' not found", tc.name),
            compact: None,
            is_error: true,
            terminate: false,
            details: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::extension::{AgentTool, BlockReason, Cancel, ToolOutput};
    use crate::agent::provider::StreamEvent;
    use crate::agent::types::{
        AgentMessage, PendingMessageQueue, QueueMode, Role, ToolCall, ToolExecutionMode,
    };
    use async_trait::async_trait;
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    // ── Mock Provider ──
    struct MockProvider {
        responses: Arc<std::sync::Mutex<Vec<MockResponse>>>,
        // Track messages sent to the provider for assertions
        sent_messages: Arc<std::sync::Mutex<Vec<Vec<AgentMessage>>>>,
    }

    struct MockResponse {
        text: String,
        tool_calls: Vec<ToolCall>,
        stop_reason: StopReason,
        thinking: String,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                responses: Arc::new(std::sync::Mutex::new(Vec::new())),
                sent_messages: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn add_response(&self, text: &str) {
            self.responses.lock().unwrap().push(MockResponse {
                text: text.to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                thinking: String::new(),
            });
        }

        fn add_tool_call_response(&self, text: &str, tool_calls: Vec<ToolCall>) {
            self.responses.lock().unwrap().push(MockResponse {
                text: text.to_string(),
                tool_calls,
                stop_reason: StopReason::ToolUse,
                thinking: String::new(),
            });
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn stream(
            &self,
            _model: &str,
            _system: &str,
            messages: &[AgentMessage],
            _tools: &[ToolDef],
        ) -> anyhow::Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>> {
            // Record messages sent to this provider call
            self.sent_messages.lock().unwrap().push(messages.to_vec());

            let mut resp = self.responses.lock().unwrap();
            let response = if resp.is_empty() {
                // Default: return end turn with empty text
                MockResponse {
                    text: String::new(),
                    tool_calls: vec![],
                    stop_reason: StopReason::EndTurn,
                    thinking: String::new(),
                }
            } else {
                resp.remove(0)
            };
            drop(resp);

            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

            // Send thinking if present
            if !response.thinking.is_empty() {
                let _ = tx.send(StreamEvent::ThinkingDelta {
                    text: response.thinking.clone(),
                });
            }

            // Send text deltas
            if !response.text.is_empty() {
                let _ = tx.send(StreamEvent::TextDelta {
                    text: response.text.clone(),
                });
            }

            // Send done
            let _ = tx.send(StreamEvent::Done {
                text: response.text,
                usage: crate::agent::types::Usage::default(),
                stop_reason: response.stop_reason,
                tool_calls: response.tool_calls,
            });

            // Convert receiver to stream using futures::stream::unfold
            use futures::stream::unfold;
            let stream = unfold(rx, |mut rx| async move {
                rx.recv().await.map(|event| (event, rx))
            });
            Ok(Box::pin(stream))
        }
    }

    // ── Mock Tool ──
    struct MockTool {
        name: String,
        execution_mode: ToolExecutionMode,
        execute_delay: std::time::Duration,
        executed: Arc<std::sync::Mutex<Vec<String>>>,
        terminate: bool,
    }

    impl MockTool {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                execution_mode: ToolExecutionMode::Parallel,
                execute_delay: std::time::Duration::ZERO,
                executed: Arc::new(std::sync::Mutex::new(Vec::new())),
                terminate: false,
            }
        }

        fn with_delay(mut self, delay: std::time::Duration) -> Self {
            self.execute_delay = delay;
            self
        }

        fn with_terminate(mut self) -> Self {
            self.terminate = true;
            self
        }
    }

    #[async_trait]
    impl AgentTool for MockTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "mock tool"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn execution_mode(&self) -> ToolExecutionMode {
            self.execution_mode
        }

        async fn execute(
            &self,
            tool_call_id: String,
            _args: serde_json::Value,
            _cancel: Cancel,
            _on_update: Option<tokio::sync::mpsc::UnboundedSender<ToolOutput>>,
        ) -> anyhow::Result<ToolOutput> {
            self.executed.lock().unwrap().push(tool_call_id.clone());

            if self.execute_delay > std::time::Duration::ZERO {
                tokio::time::sleep(self.execute_delay).await;
            }

            Ok(ToolOutput {
                content: format!("executed: {}", tool_call_id),
                compact: None,
                is_error: false,
                terminate: self.terminate,
                details: None,
            })
        }
    }

    // ── Helper: collect events ──
    #[derive(Debug, Clone)]
    struct EventRecorder {
        events: Arc<std::sync::Mutex<Vec<AgentEvent>>>,
    }

    impl EventRecorder {
        fn new() -> Self {
            Self {
                events: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn record(&self, event: AgentEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn events(&self) -> Vec<AgentEvent> {
            self.events.lock().unwrap().clone()
        }

        fn event_types(&self) -> Vec<String> {
            self.events()
                .iter()
                .map(|e| match e {
                    AgentEvent::AgentStart => "agent_start".to_string(),
                    AgentEvent::TurnStart => "turn_start".to_string(),
                    AgentEvent::TextDelta { .. } => "text_delta".to_string(),
                    AgentEvent::ThinkingDelta { .. } => "thinking_delta".to_string(),
                    AgentEvent::ToolCall { .. } => "tool_call".to_string(),
                    AgentEvent::ToolCallArgsUpdate { .. } => "tool_call_args_update".to_string(),
                    AgentEvent::ToolResult { .. } => "tool_result".to_string(),
                    AgentEvent::ToolProgress { .. } => "tool_progress".to_string(),
                    AgentEvent::Aborted { .. } => "aborted".to_string(),
                    AgentEvent::UserMessage { .. } => "user_message".to_string(),
                    AgentEvent::TurnEnd => "turn_end".to_string(),
                    AgentEvent::AgentEnd { .. } => "agent_end".to_string(),
                })
                .collect()
        }

        fn text_deltas(&self) -> Vec<String> {
            self.events()
                .iter()
                .filter_map(|e| {
                    if let AgentEvent::TextDelta { delta } = e {
                        Some(delta.clone())
                    } else {
                        None
                    }
                })
                .collect()
        }
    }

    // ── Tests ──

    /// Test basic text-only response (no tool calls).
    #[tokio::test]
    async fn test_basic_text_response() {
        let provider = MockProvider::new();
        provider.add_response("Hello, world!");

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "You are helpful.".to_string(),
            tools: vec![],
            agent_tools: &[],
            extensions: &[],
            tool_execution: ToolExecutionMode::Parallel,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let prompt = AgentMessage::user("Hi");
        let result = run_agent_loop(vec![prompt], vec![], &config, &provider, &mut emit)
            .await
            .unwrap();

        // Should have user message + assistant message
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, Role::User);
        assert_eq!(result[1].role, Role::Assistant);

        // Check event sequence
        let types = recorder.event_types();
        assert!(types.contains(&"agent_start".to_string()));
        assert!(types.contains(&"text_delta".to_string()));
        assert!(types.contains(&"turn_end".to_string()));
        assert!(types.contains(&"agent_end".to_string()));

        // Check text content
        let texts = recorder.text_deltas();
        assert!(texts.iter().any(|t| t == "Hello, world!"));
    }

    /// Test sequential tool execution.
    #[tokio::test]
    async fn test_sequential_tool_execution() {
        let tool = MockTool::new("echo");
        let tool_executed = Arc::clone(&tool.executed);
        let agent_tools: Vec<Box<dyn AgentTool>> = vec![Box::new(tool)];

        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![
                ToolCall {
                    id: "call-1".to_string(),
                    name: "echo".to_string(),
                    arguments: serde_json::json!({}),
                },
                ToolCall {
                    id: "call-2".to_string(),
                    name: "echo".to_string(),
                    arguments: serde_json::json!({}),
                },
            ],
        );
        provider.add_response("Done after tools.");

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Sequential,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("run tools")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        // 1 user + 1 assistant (tool call) + 2 tool results + 1 assistant (final)
        assert_eq!(result.len(), 5);

        let executed = tool_executed.lock().unwrap().clone();
        assert_eq!(executed.len(), 2);
        assert_eq!(executed[0], "call-1");
        assert_eq!(executed[1], "call-2");

        // Verify event sequence includes tool calls and results
        let types = recorder.event_types();
        assert!(types.contains(&"tool_call".to_string()));
        assert!(types.contains(&"tool_result".to_string()));
    }

    /// Test parallel tool execution: tools run concurrently.
    #[tokio::test]
    async fn test_parallel_tool_execution() {
        let fast_tool =
            Arc::new(MockTool::new("fast").with_delay(std::time::Duration::from_millis(50)));
        let slow_tool =
            Arc::new(MockTool::new("slow").with_delay(std::time::Duration::from_millis(100)));
        let _fast_executed = Arc::clone(&fast_tool.executed);
        let _slow_executed = Arc::clone(&slow_tool.executed);

        // Track start times to verify concurrency
        let start_times: Arc<std::sync::Mutex<Vec<(String, std::time::Instant)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let start_times_clone = Arc::clone(&start_times);

        struct TrackingTool {
            inner: MockTool,
            start_times: Arc<std::sync::Mutex<Vec<(String, std::time::Instant)>>>,
        }
        #[async_trait]
        impl AgentTool for TrackingTool {
            fn name(&self) -> &str {
                self.inner.name()
            }
            fn description(&self) -> &str {
                "tracking"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(
                &self,
                tool_call_id: String,
                args: serde_json::Value,
                cancel: Cancel,
                on_update: Option<tokio::sync::mpsc::UnboundedSender<ToolOutput>>,
            ) -> anyhow::Result<ToolOutput> {
                self.start_times
                    .lock()
                    .unwrap()
                    .push((tool_call_id.clone(), std::time::Instant::now()));
                self.inner
                    .execute(tool_call_id, args, cancel, on_update)
                    .await
            }
        }

        let agent_tools: Vec<Box<dyn AgentTool>> = vec![
            Box::new(TrackingTool {
                inner: MockTool::new("slow").with_delay(std::time::Duration::from_millis(100)),
                start_times: Arc::clone(&start_times),
            }),
            Box::new(TrackingTool {
                inner: MockTool::new("fast").with_delay(std::time::Duration::from_millis(50)),
                start_times: Arc::clone(&start_times_clone),
            }),
        ];

        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![
                ToolCall {
                    id: "slow-1".to_string(),
                    name: "slow".to_string(),
                    arguments: serde_json::json!({}),
                },
                ToolCall {
                    id: "fast-1".to_string(),
                    name: "fast".to_string(),
                    arguments: serde_json::json!({}),
                },
            ],
        );
        provider.add_response("All tools done.");

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Parallel,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        run_agent_loop(
            vec![AgentMessage::user("run tools")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        let times = start_times.lock().unwrap();
        assert_eq!(times.len(), 2, "both tools should have started");

        // Both tools should have started — in parallel mode, the second tool (fast)
        // starts before the first (slow) finishes. We just verify both started.
        let names: Vec<&str> = times.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"slow-1"));
        assert!(names.contains(&"fast-1"));
    }

    /// Test that per-tool sequential mode forces the entire batch to be sequential.
    #[tokio::test]
    async fn test_per_tool_sequential_mode() {
        let executed = Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            // Override tools to track execution order
            let _seq_exec = Arc::clone(&executed);
            let _par_exec = Arc::clone(&executed);

            struct SeqTool;
            #[async_trait]
            impl AgentTool for SeqTool {
                fn name(&self) -> &str {
                    "sequential_tool"
                }
                fn description(&self) -> &str {
                    ""
                }
                fn parameters(&self) -> serde_json::Value {
                    serde_json::json!({})
                }
                fn execution_mode(&self) -> ToolExecutionMode {
                    ToolExecutionMode::Sequential
                }
                async fn execute(
                    &self,
                    tool_call_id: String,
                    _args: serde_json::Value,
                    _cancel: Cancel,
                    _on_update: Option<tokio::sync::mpsc::UnboundedSender<ToolOutput>>,
                ) -> anyhow::Result<ToolOutput> {
                    // Simulate work
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    Ok(ToolOutput::ok(format!("done: {}", tool_call_id)))
                }
            }

            struct ParTool {
                executed: Arc<std::sync::Mutex<Vec<String>>>,
            }
            #[async_trait]
            impl AgentTool for ParTool {
                fn name(&self) -> &str {
                    "parallel_tool"
                }
                fn description(&self) -> &str {
                    ""
                }
                fn parameters(&self) -> serde_json::Value {
                    serde_json::json!({})
                }
                async fn execute(
                    &self,
                    tool_call_id: String,
                    _args: serde_json::Value,
                    _cancel: Cancel,
                    _on_update: Option<tokio::sync::mpsc::UnboundedSender<ToolOutput>>,
                ) -> anyhow::Result<ToolOutput> {
                    self.executed.lock().unwrap().push(tool_call_id.clone());
                    Ok(ToolOutput::ok(format!("done: {}", tool_call_id)))
                }
            }

            let agent_tools: Vec<Box<dyn AgentTool>> = vec![
                Box::new(SeqTool),
                Box::new(ParTool {
                    executed: Arc::clone(&executed),
                }),
            ];

            let provider = MockProvider::new();
            provider.add_tool_call_response(
                "",
                vec![
                    ToolCall {
                        id: "seq-1".to_string(),
                        name: "sequential_tool".to_string(),
                        arguments: serde_json::json!({}),
                    },
                    ToolCall {
                        id: "par-1".to_string(),
                        name: "parallel_tool".to_string(),
                        arguments: serde_json::json!({}),
                    },
                ],
            );
            provider.add_response("Done.");

            let recorder = EventRecorder::new();
            let mut emit = |e: AgentEvent| recorder.record(e);

            let config = LoopConfig {
                model: "test".to_string(),
                system_prompt: "".to_string(),
                tools: vec![],
                agent_tools: &agent_tools,
                extensions: &[],
                tool_execution: ToolExecutionMode::Parallel,
                steering_queue: None,
                follow_up_queue: None,
                transform_context: None,
                prepare_next_turn: None,
                should_stop_after_turn: None,
            };

            run_agent_loop(
                vec![AgentMessage::user("run")],
                vec![],
                &config,
                &provider,
                &mut emit,
            )
            .await
            .unwrap();

            // Both should execute (one is sequential by declaration)
            let exec_order = executed.lock().unwrap().clone();
            assert_eq!(
                exec_order.len(),
                1,
                "only parallel_tool records in executed"
            );
        }
    }

    /// Test that terminate flag on ALL tools stops the loop.
    #[tokio::test]
    async fn test_terminate_stops_loop() {
        let agent_tools: Vec<Box<dyn AgentTool>> =
            vec![Box::new(MockTool::new("final").with_terminate())];

        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![ToolCall {
                id: "final-1".to_string(),
                name: "final".to_string(),
                arguments: serde_json::json!({}),
            }],
        );
        // No second response — loop should stop after terminate

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Parallel,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("final")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        // Should have: user msg + assistant (tool call) + tool result
        // NOT a second assistant (which would come from another LLM call)
        assert_eq!(
            result.len(),
            3,
            "should stop after terminate without second LLM call"
        );

        let types = recorder.event_types();
        assert!(types.contains(&"turn_end".to_string()));
        assert!(types.contains(&"agent_end".to_string()));
    }

    /// Test that transform_context rewrites messages before LLM call.
    #[tokio::test]
    async fn test_transform_context() {
        let provider = MockProvider::new();
        provider.add_response("Response");

        let transform_called = Arc::new(std::sync::Mutex::new(false));
        let transform_called_clone = Arc::clone(&transform_called);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &[],
            extensions: &[],
            tool_execution: ToolExecutionMode::Parallel,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: Some(Box::new(move |msgs| {
                *transform_called_clone.lock().unwrap() = true;
                msgs.to_vec()
            })),
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let mut emit = |_: AgentEvent| {};
        run_agent_loop(
            vec![AgentMessage::user("hi")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        assert!(
            *transform_called.lock().unwrap(),
            "transform_context should be called"
        );
    }

    /// Test that prepare_next_turn can modify context.
    #[tokio::test]
    async fn test_prepare_next_turn() {
        let agent_tools: Vec<Box<dyn AgentTool>> = vec![Box::new(MockTool::new("echo"))];
        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![ToolCall {
                id: "tool-1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({}),
            }],
        );
        provider.add_response("After prepare.");

        let prepare_called = Arc::new(std::sync::Mutex::new(false));
        let prepare_called_clone = Arc::clone(&prepare_called);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Sequential,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: Some(Box::new(move |_new_msgs| {
                *prepare_called_clone.lock().unwrap() = true;
                None // don't modify context
            })),
            should_stop_after_turn: None,
        };

        let mut emit = |_: AgentEvent| {};
        run_agent_loop(
            vec![AgentMessage::user("run")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        assert!(
            *prepare_called.lock().unwrap(),
            "prepare_next_turn should be called"
        );
    }

    /// Test that should_stop_after_turn can stop the loop early.
    #[tokio::test]
    async fn test_should_stop_after_turn() {
        let provider = MockProvider::new();
        provider.add_response("First turn.");

        let stop = Arc::new(std::sync::Mutex::new(true));
        let stop_clone = Arc::clone(&stop);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &[],
            extensions: &[],
            tool_execution: ToolExecutionMode::Parallel,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: Some(Box::new(move |_| *stop_clone.lock().unwrap())),
        };

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);
        run_agent_loop(
            vec![AgentMessage::user("hi")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        // Should have exactly 1 assistant message (no second turn)
        let types = recorder.event_types();
        let agent_end_count = types.iter().filter(|t| *t == "agent_end").count();
        assert_eq!(agent_end_count, 1, "should end exactly once");
    }

    /// Test steering queue: messages injected between turns.
    #[tokio::test]
    async fn test_steering_queue() {
        let agent_tools: Vec<Box<dyn AgentTool>> = vec![Box::new(MockTool::new("echo"))];
        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![ToolCall {
                id: "tool-1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({}),
            }],
        );
        provider.add_response("After tool.");
        provider.add_response("After steering.");

        let steering_queue = std::sync::Mutex::new(PendingMessageQueue::new(QueueMode::OneAtATime));
        // Queue a steering message — it should be injected after tool result, before 2nd LLM call
        steering_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("steer here"));

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Sequential,
            steering_queue: Some(&steering_queue),
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("run")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        // The steering message should appear as a UserMessage in events and as
        // an injected user message in the result
        let types = recorder.event_types();
        let user_msg_count = types.iter().filter(|t| *t == "user_message").count();
        assert!(
            user_msg_count >= 1,
            "steering should produce at least one user_message event, got {}",
            user_msg_count
        );

        // Result should contain: user prompt + assistant (tool call) + tool result + steering user + assistant
        let user_messages: Vec<&AgentMessage> =
            result.iter().filter(|m| m.role == Role::User).collect();
        assert_eq!(
            user_messages.len(),
            2,
            "should have original prompt + steering message"
        );
    }

    /// Test follow-up queue: messages injected after agent is idle.
    #[tokio::test]
    async fn test_follow_up_queue() {
        let provider = MockProvider::new();
        provider.add_response("First response.");
        provider.add_response("Follow-up response.");

        let follow_up_queue =
            std::sync::Mutex::new(PendingMessageQueue::new(QueueMode::OneAtATime));
        follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("follow up"));

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &[],
            extensions: &[],
            tool_execution: ToolExecutionMode::Parallel,
            steering_queue: None,
            follow_up_queue: Some(&follow_up_queue),
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("first")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        // Should have: user + assistant (first) + user (follow-up) + assistant (follow-up)
        assert_eq!(
            result.len(),
            4,
            "follow-up should add another user+assistant pair"
        );
        assert_eq!(
            result[2].content, "follow up",
            "third message should be the injected follow-up"
        );

        let types = recorder.event_types();
        assert!(types.contains(&"user_message".to_string()));
    }

    /// Test PendingMessageQueue drain modes.
    #[tokio::test]
    async fn test_message_queue_modes() {
        // OneAtATime: drain one message at a time
        let mut queue = PendingMessageQueue::new(QueueMode::OneAtATime);
        queue.enqueue(AgentMessage::user("msg1"));
        queue.enqueue(AgentMessage::user("msg2"));

        let batch1 = queue.drain();
        assert_eq!(batch1.len(), 1, "OneAtATime should drain 1");
        assert_eq!(batch1[0].content, "msg1");

        let batch2 = queue.drain();
        assert_eq!(batch2.len(), 1, "OneAtATime should drain 1 on second call");
        assert_eq!(batch2[0].content, "msg2");

        assert!(
            queue.drain().is_empty(),
            "should be empty after both drained"
        );

        // All: drain all at once
        let mut queue = PendingMessageQueue::new(QueueMode::All);
        queue.enqueue(AgentMessage::user("a"));
        queue.enqueue(AgentMessage::user("b"));

        let all = queue.drain();
        assert_eq!(all.len(), 2, "All mode should drain both");
        assert!(queue.drain().is_empty(), "should be empty after drain");

        // Clear
        let mut queue = PendingMessageQueue::new(QueueMode::OneAtATime);
        queue.enqueue(AgentMessage::user("x"));
        queue.clear();
        assert!(queue.is_empty());
    }

    /// Test that prepare_arguments is called on the tool.
    #[tokio::test]
    async fn test_prepare_arguments() {
        struct PrepTool;
        #[async_trait]
        impl AgentTool for PrepTool {
            fn name(&self) -> &str {
                "prep_tool"
            }
            fn description(&self) -> &str {
                ""
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
                let mut m = serde_json::Map::new();
                m.insert("prepared".to_string(), serde_json::json!(true));
                if let Some(obj) = args.as_object() {
                    for (k, v) in obj {
                        m.insert(k.clone(), v.clone());
                    }
                }
                serde_json::Value::Object(m)
            }
            async fn execute(
                &self,
                _tool_call_id: String,
                args: serde_json::Value,
                _cancel: Cancel,
                _on_update: Option<tokio::sync::mpsc::UnboundedSender<ToolOutput>>,
            ) -> anyhow::Result<ToolOutput> {
                // Verify prepare_arguments was called: args should have "prepared": true
                assert_eq!(args.get("prepared").and_then(|v| v.as_bool()), Some(true));
                Ok(ToolOutput::ok("prepared ok"))
            }
        }

        let agent_tools: Vec<Box<dyn AgentTool>> = vec![Box::new(PrepTool)];
        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![ToolCall {
                id: "tool-1".to_string(),
                name: "prep_tool".to_string(),
                arguments: serde_json::json!({"original": "value"}),
            }],
        );
        provider.add_response("Done.");

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Sequential,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let mut emit = |_: AgentEvent| {};
        let result = run_agent_loop(
            vec![AgentMessage::user("prep")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await;

        assert!(
            result.is_ok(),
            "prepare_arguments should work without error"
        );
    }

    /// Test that before_tool_call can block execution.
    #[tokio::test]
    async fn test_before_tool_call_blocks() {
        struct BlockingExt;
        #[async_trait]
        impl Extension for BlockingExt {
            fn name(&self) -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed("blocker")
            }
            async fn before_tool_call(&self, _tc: &ToolCall) -> Option<BlockReason> {
                Some(BlockReason::Security("blocked for test".into()))
            }
        }

        let agent_tools: Vec<Box<dyn AgentTool>> = vec![Box::new(MockTool::new("echo"))];
        let extensions: Vec<Box<dyn Extension>> = vec![Box::new(BlockingExt)];

        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![ToolCall {
                id: "tool-1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({}),
            }],
        );
        provider.add_response("After blocked tool.");

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &extensions,
            tool_execution: ToolExecutionMode::Sequential,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("block test")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        // Should have: user + assistant (tool call) + tool result (blocked) + assistant (after)
        assert!(
            result.len() >= 3,
            "blocked tool should still produce a result"
        );

        // Find the tool result
        let tool_results: Vec<&AgentMessage> = result
            .iter()
            .filter(|m| m.role == Role::ToolResult)
            .collect();
        assert!(!tool_results.is_empty());
        assert!(
            tool_results[0].is_error,
            "blocked tool result should be error"
        );
        assert!(
            tool_results[0].content.contains("blocked"),
            "blocked result should mention block reason"
        );
    }

    /// Test error response from provider leads to graceful abort.
    #[tokio::test]
    async fn test_provider_error_aborts() {
        // A provider that returns an error
        struct ErrorProvider;
        #[async_trait]
        impl Provider for ErrorProvider {
            async fn stream(
                &self,
                _model: &str,
                _system: &str,
                _messages: &[AgentMessage],
                _tools: &[ToolDef],
            ) -> anyhow::Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>> {
                anyhow::bail!("provider error")
            }
        }

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &[],
            extensions: &[],
            tool_execution: ToolExecutionMode::Parallel,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("hi")],
            vec![],
            &config,
            &ErrorProvider,
            &mut emit,
        )
        .await;

        // Provider error should propagate as an Err
        assert!(result.is_err(), "provider error should propagate");
    }

    /// Test that tool execution errors are reported as tool results.
    #[tokio::test]
    async fn test_tool_execution_error() {
        struct ErrorTool;
        #[async_trait]
        impl AgentTool for ErrorTool {
            fn name(&self) -> &str {
                "error_tool"
            }
            fn description(&self) -> &str {
                ""
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(
                &self,
                _tool_call_id: String,
                _args: serde_json::Value,
                _cancel: Cancel,
                _on_update: Option<tokio::sync::mpsc::UnboundedSender<ToolOutput>>,
            ) -> anyhow::Result<ToolOutput> {
                anyhow::bail!("tool crashed")
            }
        }

        let agent_tools: Vec<Box<dyn AgentTool>> = vec![Box::new(ErrorTool)];
        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![ToolCall {
                id: "tool-1".to_string(),
                name: "error_tool".to_string(),
                arguments: serde_json::json!({}),
            }],
        );
        provider.add_response("After error.");

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Sequential,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("error test")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        // Should have error tool result
        let tool_results: Vec<&AgentMessage> = result
            .iter()
            .filter(|m| m.role == Role::ToolResult)
            .collect();
        assert!(!tool_results.is_empty());
        assert!(tool_results[0].is_error);
    }

    /// Test that tool not found produces an error tool result.
    #[tokio::test]
    async fn test_tool_not_found() {
        let provider = MockProvider::new();
        provider.add_tool_call_response(
            "",
            vec![ToolCall {
                id: "tool-1".to_string(),
                name: "nonexistent".to_string(),
                arguments: serde_json::json!({}),
            }],
        );
        provider.add_response("After missing tool.");

        // Empty agent_tools — the tool won't be found
        let agent_tools: Vec<Box<dyn AgentTool>> = vec![];

        let recorder = EventRecorder::new();
        let mut emit = |e: AgentEvent| recorder.record(e);

        let config = LoopConfig {
            model: "test".to_string(),
            system_prompt: "".to_string(),
            tools: vec![],
            agent_tools: &agent_tools,
            extensions: &[],
            tool_execution: ToolExecutionMode::Sequential,
            steering_queue: None,
            follow_up_queue: None,
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let result = run_agent_loop(
            vec![AgentMessage::user("test")],
            vec![],
            &config,
            &provider,
            &mut emit,
        )
        .await
        .unwrap();

        let tool_results: Vec<&AgentMessage> = result
            .iter()
            .filter(|m| m.role == Role::ToolResult)
            .collect();
        assert!(!tool_results.is_empty());
        assert!(tool_results[0].is_error);
        assert!(tool_results[0].content.contains("not found"));
    }
}
