//! Rich compatibility flags matching pi's OpenAICompletionsCompat structure.
//! Deserialized from the `compat` field in models.json and passed directly
//! to `RabOpenAiCompatProvider`.

use serde::{Deserialize, Serialize};
use yoagent::provider::model::OpenAiCompat;

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
    /// Whether replayed assistant messages must include `reasoning_content`.
    pub requires_reasoning_content_on_assistant_messages: bool,
    /// How thinking/reasoning is formatted in the API.
    pub thinking_format: RabThinkingFormat,
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
            requires_reasoning_content_on_assistant_messages: false,
            thinking_format: RabThinkingFormat::OpenAi,
        }
    }
}

impl From<&OpenAiCompat> for RabOpenAiCompat {
    fn from(c: &OpenAiCompat) -> Self {
        use yoagent::provider::model::MaxTokensField;
        let max_tokens_field = match c.max_tokens_field {
            MaxTokensField::MaxTokens => RabMaxTokensField::MaxTokens,
            MaxTokensField::MaxCompletionTokens => RabMaxTokensField::MaxCompletionTokens,
        };
        use yoagent::provider::model::ThinkingFormat;
        let thinking_format = match c.thinking_format {
            ThinkingFormat::OpenAi | ThinkingFormat::Xai => RabThinkingFormat::OpenAi,
            ThinkingFormat::Qwen => RabThinkingFormat::Qwen,
        };
        Self {
            supports_store: c.supports_store,
            supports_developer_role: c.supports_developer_role,
            supports_reasoning_effort: c.supports_reasoning_effort,
            supports_thinking_control: c.supports_thinking_control,
            supports_usage_in_streaming: c.supports_usage_in_streaming,
            max_tokens_field,
            requires_tool_result_name: c.requires_tool_result_name,
            requires_assistant_after_tool_result: c.requires_assistant_after_tool_result,
            thinking_format,
            ..Default::default()
        }
    }
}
