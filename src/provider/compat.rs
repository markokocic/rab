//! Rich compatibility flags matching pi's OpenAICompletionsCompat structure.
//! Deserialized from the `compat` field in models.json, then serialized into
//! `ModelConfig::headers["_rab_compat"]` for our custom provider to read.

use serde::{Deserialize, Serialize};

/// Thinking format strategies (maps pi's `thinkingFormat`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RabThinkingFormat {
    #[default]
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "openrouter")]
    OpenRouter,
    #[serde(rename = "deepseek")]
    DeepSeek,
    #[serde(rename = "together")]
    Together,
    #[serde(rename = "zai")]
    Zai,
    #[serde(rename = "qwen")]
    Qwen,
    #[serde(rename = "chat-template")]
    ChatTemplate,
    #[serde(rename = "qwen-chat-template")]
    QwenChatTemplate,
    #[serde(rename = "string-thinking")]
    StringThinking,
    #[serde(rename = "ant-ling")]
    AntLing,
}

/// Max tokens field name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RabMaxTokensField {
    #[serde(rename = "max_tokens")]
    MaxTokens,
    #[default]
    #[serde(rename = "max_completion_tokens")]
    MaxCompletionTokens,
}

/// Rich compatibility flags, matching pi's `OpenAICompletionsCompat` schema.
///
/// All fields are optional — defaults are resolved at use time in the provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RabOpenAiCompat {
    /// Whether the provider supports the `store` parameter (OpenAI-only).
    pub supports_store: bool,
    /// Whether the provider supports the `developer` role vs `system`.
    pub supports_developer_role: bool,
    /// Whether the provider supports `reasoning_effort`.
    pub supports_reasoning_effort: bool,
    /// Whether the provider supports `thinking: { type }` control.
    pub supports_thinking_control: bool,
    /// Whether usage data is available in streaming responses.
    pub supports_usage_in_streaming: bool,
    /// Which field name to use for max tokens.
    pub max_tokens_field: RabMaxTokensField,
    /// Whether tool results must include a `name` field.
    pub requires_tool_result_name: bool,
    /// Whether an assistant message must be inserted after tool results.
    pub requires_assistant_after_tool_result: bool,
    /// Whether thinking blocks must be converted to text with `<thinking>` tags.
    pub requires_thinking_as_text: bool,
    /// Whether replayed assistant messages must include `reasoning_content`.
    pub requires_reasoning_content_on_assistant_messages: bool,
    /// How thinking/reasoning is formatted in the API.
    pub thinking_format: RabThinkingFormat,
    /// Whether the provider supports the `strict` field in tool definitions.
    pub supports_strict_mode: bool,
    /// Whether the provider supports long cache retention.
    pub supports_long_cache_retention: bool,
}

impl Default for RabOpenAiCompat {
    fn default() -> Self {
        Self {
            supports_store: true,
            supports_developer_role: true,
            supports_reasoning_effort: true,
            supports_thinking_control: false,
            supports_usage_in_streaming: true,
            max_tokens_field: RabMaxTokensField::MaxCompletionTokens,
            requires_tool_result_name: false,
            requires_assistant_after_tool_result: false,
            requires_thinking_as_text: false,
            requires_reasoning_content_on_assistant_messages: false,
            thinking_format: RabThinkingFormat::OpenAi,
            supports_strict_mode: true,
            supports_long_cache_retention: true,
        }
    }
}
