use std::time::Instant;

use crate::agent::provider::{Provider, StreamEvent, ToolDef};
use crate::agent::types::{AgentMessage, Role, ToolCall};
use crate::auth::AuthStorage;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ContentPart, MessageContent, ReasoningEffort, Tool,
    ToolCall as GenaiToolCall, ToolResponse,
};
use genai::resolver::{AuthData, AuthResolver};
use std::pin::Pin;
use std::sync::RwLock;

/// Build a reqwest::Client that uses webpki-roots (embedded Mozilla CA list)
/// instead of rustls-platform-verifier, which panics on Android/Termux
/// because it requires JNI initialization.
fn build_reqwest_client() -> reqwest::Client {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls_config)
        .timeout(std::time::Duration::from_secs(300)) // overall request timeout
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build reqwest client")
}

pub struct GenaiProvider {
    client: genai::Client,
    model_prefix: String,
    reasoning_effort: RwLock<Option<ReasoningEffort>>,
}

impl GenaiProvider {
    pub fn new(auth: &AuthStorage, thinking_level: Option<&str>) -> anyhow::Result<Self> {
        let api_key = auth
            .api_key("opencode-go")
            .ok_or_else(|| anyhow::anyhow!("No API key found for opencode_go in auth.json"))?;

        let auth_resolver = AuthResolver::from_resolver_fn(move |_model_iden: genai::ModelIden| {
            Ok(Some(AuthData::from_single(api_key.clone())))
        });

        let reqwest_client = build_reqwest_client();
        let client = genai::Client::builder()
            .with_reqwest(reqwest_client)
            .with_auth_resolver(auth_resolver)
            .build();

        // Reasoning effort mapping:
        // - When thinking_level is None (not configured), default to High (highest
        //   commonly supported value across OpenAI-compatible providers).
        // - When "off" or "none", set to None so the parameter is omitted entirely
        //   ("none" is not widely supported and causes 400 errors with DeepSeek).
        // - Values beyond "high" ("xhigh", "max") are clamped to "high" since few
        //   providers support them.
        // - "minimal" maps to "low" for the same reason.
        // - Any unrecognized value also defaults to High.
        let reasoning_effort = match thinking_level {
            Some("off" | "none") => None,
            Some("minimal" | "low") => Some(ReasoningEffort::Low),
            Some("medium") => Some(ReasoningEffort::Medium),
            _ => Some(ReasoningEffort::High), // None, xhigh, max, or unknown → highest commonly supported
        };

        Ok(Self {
            client,
            model_prefix: "opencode_go::".into(),
            reasoning_effort: RwLock::new(reasoning_effort),
        })
    }

    fn full_model(&self, model: &str) -> String {
        if model.contains("::") {
            model.to_string()
        } else {
            format!("{}{}", self.model_prefix, model)
        }
    }

    /// Convert a thinking level string to ReasoningEffort.
    fn thinking_level_to_effort(level: Option<&str>) -> Option<ReasoningEffort> {
        match level {
            Some("off" | "none") => None,
            Some("minimal" | "low") => Some(ReasoningEffort::Low),
            Some("medium") => Some(ReasoningEffort::Medium),
            _ => Some(ReasoningEffort::High), // None, xhigh, max, or unknown
        }
    }

    fn convert_messages(messages: &[AgentMessage]) -> Vec<ChatMessage> {
        messages
            .iter()
            .map(|m| match m.role {
                Role::User => ChatMessage::user(&m.content),
                Role::Assistant => {
                    let mut parts: Vec<ContentPart> = Vec::new();

                    // Include text content if present (supports models that emit both
                    // text and tool calls in the same assistant turn)
                    if !m.content.is_empty() {
                        parts.push(ContentPart::from_text(&m.content));
                    }

                    for tc in &m.tool_calls {
                        parts.push(ContentPart::ToolCall(GenaiToolCall {
                            call_id: tc.id.clone(),
                            fn_name: tc.name.clone(),
                            fn_arguments: tc.arguments.clone(),
                            thought_signatures: None,
                        }));
                    }

                    ChatMessage::assistant(MessageContent::from_parts(parts))
                }
                Role::ToolResult => ChatMessage::from(ToolResponse::new(
                    m.tool_call_id.clone().unwrap_or_default(),
                    &m.content,
                )),
            })
            .collect()
    }

    fn convert_tools(tools: &[ToolDef]) -> Vec<Tool> {
        tools
            .iter()
            .map(|t| {
                Tool::new(&t.name)
                    .with_description(&t.description)
                    .with_schema(t.parameters.clone())
            })
            .collect()
    }
}

#[async_trait]
impl Provider for GenaiProvider {
    async fn stream(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
        tools: &[ToolDef],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>> {
        let full_model = self.full_model(model);
        let chat_messages = Self::convert_messages(messages);
        let genai_tools = Self::convert_tools(tools);

        let mut req = ChatRequest::new(chat_messages).with_system(system_prompt);
        if !genai_tools.is_empty() {
            req = req.with_tools(genai_tools);
        }

        let mut options = ChatOptions::default()
            .with_capture_usage(true)
            .with_capture_content(true)
            .with_capture_tool_calls(true);

        if let Ok(guard) = self.reasoning_effort.read()
            && let Some(ref effort) = *guard
        {
            options = options.with_reasoning_effort(effort.clone());
        }

        // Pi-compatible streaming timeout configuration:
        // - Initial response (exec_chat_stream): 30s timeout.
        //   The genai crate creates the request builder but the actual HTTP
        //   request is lazily sent when the stream is first polled. However,
        //   adapter dispatch and auth resolution happen here, so a timeout
        //   prevents hangs at this stage too (matching pi's approach of
        //   having timeouts at every boundary).
        let genai_response = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.client
                .exec_chat_stream(&full_model, req, Some(&options)),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Initial chat stream request timed out after 30s — provider did not respond"
            )
        })?
        .map_err(|e| anyhow::anyhow!("{:#}", e))?;

        let mut genai_stream = genai_response.stream;

        // Pi-compatible streaming timeout configuration:
        // - First-event timeout: 20s (matching pi's DEFAULT_SSE_HEADER_TIMEOUT_MS)
        //   Prevents zero-event SSE streams from leaving callers stuck on "Working...".
        // - Per-chunk idle timeout: 60s between subsequent events.
        //   If the stream stalls mid-response (no tokens, no done, no error),
        //   this ensures we eventually error out instead of hanging forever.
        //   Pi's SSE path has no per-chunk timeout, but its WebSocket path uses
        //   a configurable idle timeout (default 300s). We use 60s as a reasonable
        //   safety net that still gives providers ample time between events.
        const FIRST_EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
        const CHUNK_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

        let stream = async_stream::stream! {
            // When the last yielded event arrived (or None if nothing yielded yet).
            // We track this separately from raw stream events because some providers
            // send keep-alive chunks (e.g. ThoughtSignatureChunk) that should NOT
            // reset the idle timer. Only yielded events count as progress.
            let mut last_yield: Option<Instant> = None;
            let started = Instant::now();

            loop {
                // Quick poll (500ms) so we detect stream end / next event
                // promptly while still giving the runtime breathing room.
                let result = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    genai_stream.next(),
                ).await;

                match result {
                    Ok(Some(Ok(event))) => {
                        let mut yielded = false;
                        match event {
                            genai::chat::ChatStreamEvent::Start => {},
                            genai::chat::ChatStreamEvent::Chunk(chunk) => {
                                yielded = true;
                                yield StreamEvent::TextDelta { text: chunk.content };
                            }
                            genai::chat::ChatStreamEvent::ReasoningChunk(chunk) => {
                                yielded = true;
                                yield StreamEvent::ThinkingDelta { text: chunk.content };
                            }
                            // ThoughtSignatureChunk is a keep-alive that the
                            // provider sends while still thinking. It does NOT
                            // count as progress — if only these arrive we still
                            // time out after FIRST_EVENT_TIMEOUT (20s).
                            genai::chat::ChatStreamEvent::ThoughtSignatureChunk(_) => {},
                            genai::chat::ChatStreamEvent::ToolCallChunk(tool_chunk) => {
                                yielded = true;
                                let tc = &tool_chunk.tool_call;
                                yield StreamEvent::ToolCall {
                                    id: tc.call_id.clone(),
                                    name: tc.fn_name.clone(),
                                    arguments: serde_json::to_string(&tc.fn_arguments)
                                        .unwrap_or_default(),
                                };
                            }
                            genai::chat::ChatStreamEvent::End(end) => {
                                yielded = true;
                                let text = end.captured_first_text().unwrap_or("").to_string();
                                let tool_calls: Vec<ToolCall> = end
                                    .captured_tool_calls()
                                    .into_iter()
                                    .flatten()
                                    .map(|tc| ToolCall {
                                        id: tc.call_id.clone(),
                                        name: tc.fn_name.clone(),
                                        arguments: tc.fn_arguments.clone(),
                                    })
                                    .collect();

                                let usage = crate::agent::types::Usage {
                                    input_tokens: end.captured_usage.as_ref()
                                        .and_then(|u| u.prompt_tokens),
                                    output_tokens: end.captured_usage.as_ref()
                                        .and_then(|u| u.completion_tokens),
                                    cache_tokens: end.captured_usage.as_ref()
                                        .and_then(|u| u.prompt_tokens_details.as_ref()
                                            .and_then(|d| d.cached_tokens)),
                                    cache_write_tokens: end.captured_usage.as_ref()
                                        .and_then(|u| u.prompt_tokens_details.as_ref()
                                            .and_then(|d| d.cache_creation_tokens)),
                                    cost_total: None,
                                };

                                let stop_reason = match &end.captured_stop_reason {
                                    Some(genai::chat::StopReason::Completed(_)) => crate::agent::provider::StopReason::EndTurn,
                                    Some(genai::chat::StopReason::ToolCall(_)) => crate::agent::provider::StopReason::ToolUse,
                                    Some(genai::chat::StopReason::MaxTokens(_)) => crate::agent::provider::StopReason::MaxTokens,
                                    _ => crate::agent::provider::StopReason::EndTurn,
                                };

                                yield StreamEvent::Done {
                                    text,
                                    usage,
                                    stop_reason,
                                    tool_calls,
                                };
                            }
                        }
                        if yielded {
                            last_yield = Some(Instant::now());
                        }
                    }
                    Ok(Some(Err(e))) => {
                        last_yield = Some(Instant::now());
                        yield StreamEvent::Error {
                            message: format!("{:#}", e),
                        };
                    }
                    Ok(None) => {
                        // Stream ended cleanly (no more events)
                        break;
                    }
                    Err(_elapsed) => {
                        // 500ms wire poll timed out — no raw event arrived.
                        // Check the idle timeout against last meaningful yield.
                        let idle = last_yield
                            .map(|t| t.elapsed())
                            .unwrap_or_else(|| started.elapsed());
                        // First-event timeout: 20s (matching pi's
                        // DEFAULT_SSE_HEADER_TIMEOUT_MS). Prevents zero-event
                        // streams from leaving callers stuck on "Working...".
                        // Per-chunk idle timeout: 60s between yielded events.
                        // If only keep-alive chunks arrive, this still fires.
                        // Pi's SSE path has no per-chunk timeout, but its
                        // WebSocket path uses a configurable idle timeout
                        // (default 300s). We use 60s as a reasonable safety net.
                        let limit = if last_yield.is_none() {
                            FIRST_EVENT_TIMEOUT
                        } else {
                            CHUNK_IDLE_TIMEOUT
                        };
                        if idle >= limit {
                            yield StreamEvent::Error {
                                message: format!(
                                    "No response from provider after {}s — connection may be stalled",
                                    idle.as_secs(),
                                ),
                            };
                            break;
                        }
                        // idle < limit: poll expired without data but within
                        // the allowed window — loop again.
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    fn set_reasoning_effort(&self, level: Option<&str>) {
        let effort = Self::thinking_level_to_effort(level);
        if let Ok(mut guard) = self.reasoning_effort.write() {
            *guard = effort;
        }
    }
}
