# rab Architecture

A lightweight, extensible Rust coding agent inspired by [pi-coding-agent](https://pi.dev).
rab delegates the core agent loop, types, and provider abstraction to the **yoagent** crate,
providing the session layer, TUI, built-in tools, slash commands, file search tools (grep/find/ls),
file mutation queue, lifecycle management, and a **custom provider layer** with a model registry
and rich OpenAI-compatible streaming support.

---

## Layered architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                          rab (EPL-2.0)                               │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │               main.rs (manual arg parsing)                    │   │
│  │  arg parsing, env reading, session init,                     │   │
│  │  mode dispatch (print / interactive), extension gating,      │   │
│  │  context file loading, skills, auth, provider resolution     │   │
│  │  subcommand: rab update-models                               │   │
│  └────────────────────┬─────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │              AgentSession (agent_session.rs)                  │   │
│  │  Primary entry point; owns Session + config (cwd, dir,       │   │
│  │  persist, lazy write). Factory methods: create, open, etc.  │   │
│  │  - Event-driven message persistence (crash-safe)             │   │
│  │  - Model/thinking/tool change detection & recording          │   │
│  │  - Auto/manual compaction (compaction.rs)                    │   │
│  │  - Branch summarization (branch_summary.rs)                  │   │
│  │  - Branch navigation (set_branch)                            │   │
│  │  - Pi-compatible persist_message_end pattern                 │   │
│  └──────────┬───────────────────────────────────────────────────┘   │
│             │                                                       │
│  ┌──────────▼───────────────────────────────────────────────────┐   │
│  │              Session (agent/session/mod.rs + model.rs)        │   │
│  │  High-level API wrapping SessionStorage. Pi-compatible:      │   │
│  │  append_*, build_context, move_to, metadata.                │   │
│  │  SessionError, SessionTreeNode, SessionManager (internal)   │   │
│  └──────────┬───────────────────────────────────────────────────┘   │
│             │                                                       │
│  ┌──────────▼───────────────────────────────────────────────────┐   │
│  │              SessionStorage (agent/session/storage.rs)        │   │
│  │  Low-level CRUD: leaf mgmt, labels, path queries.            │   │
│  │  Impls: InMemorySessionStorage, JsonlSessionStorage          │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐      │
│  │builtin│ │  tui/  │ │commands│ │extens-│ │settings│ │ auth  │      │
│  │read   │ │ agent/ │ │.rs     │ │ions/  │ │.rs     │ │.rs    │      │
│  │write  │ │ ui/    │ │22 slash│ ┌──────────────┐ │~/.rab/ │ │API    │
│  │edit   │ │screen  │ │commands│ │file_search (3)│ │settings│ │keys,  │
│  │bash   │ │editor  │ │        │ │mcp/ (6 mods,  │ │AGENTS  │ │OAuth  │
│  │file_  │ │list    │ │        │ │ 2K lines)     │ │.md     │ │       │
│  │mutation│ └───────┘ │        │ │AGENTS.md       │ │skills  │ │       │
│  │_queue │            │        │ │skills          │ │        │ │       │
│  │cancel │            │        │ │prompts/        │ │        │ │       │
│  └──┬────┘            │        │ │prompt_templ.rs │ │        │ │       │
│     │                 │        │ └──────────────┘ └───────┘ └───────┘      │
│     │                 │        │                                     │
│     │     impl Extension trait + yoagent::types::AgentTool          │
│     │                                                               │
│  ┌──▼──────────────────────────────────────────────────────────┐   │
│  │              agent/extension.rs (Extension trait)             │   │
│  │  pub trait Extension: Send + Sync {                          │   │
│  │    fn name(&self) -> Cow<'static, str>;                      │   │
│  │    fn tools(&self) -> Vec<ToolDefinition>;                   │   │
│  │    fn commands(&self) -> Vec<SlashCommand>;                  │   │
│  │    fn skills(&self) -> SkillSet;                             │   │
│  │  }                                                           │   │
│  │  ToolDefinition wraps AgentTool with: snippet, guidelines,   │   │
│  │  prepare_arguments, before_tool_call hook, after_tool_call   │   │
│  │  hook, before_compact hook, and bundled ToolRenderer.        │   │
│  │  validate_tool_arguments() + coerce_with_json_schema()       │   │
│  │  Builtin + user extensions share this trait                  │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │              rab Provider Layer (src/provider/)               │   │
│  │                                                              │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           ProviderRegistry (mod.rs)                   │   │   │
│  │  │  Loads built-in + user models.json → ProviderEntry[]  │   │   │
│  │  │  resolve(model_id, preferred_provider) → ResolvedModel│   │   │
│  │  │  list_models(), provider_for_model(), count_providers │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           models.rs (models.json parsing)             │   │   │
│  │  │  ProviderEntry, ModelDef parsing                     │   │   │
│  │  │  load_builtin() / load_user() / merge()              │   │   │
│  │  │  ApiProtocol conversion, CostConfig, compat          │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           openai_compat.rs (RabOpenAiCompatProvider)  │   │   │
│  │  │  Custom streaming provider replacing yoagent's       │   │   │
│  │  │  OpenAiCompatProvider with richer compat handling:   │   │   │
│  │  │  - DeepSeek thinking: { type } format                │   │   │
│  │  │  - reasoning_content on replayed assistant messages  │   │   │
│  │  │  - Configurable max_tokens field name                │   │   │
│  │  │  - All pi OpenAICompletionsCompat flags              │   │   │
│  │  │  - Thinking Level → reasoning_effort mapping         │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           compat.rs (RabOpenAiCompat)                │   │   │
│  │  │  Rich compat flags stored as JSON in model headers:  │   │   │
│  │  │  supports_store, supports_developer_role,            │   │   │
│  │  │  supports_reasoning_effort, supports_thinking_control│   │   │
│  │  │  supports_usage_in_streaming, max_tokens_field,      │   │   │
│  │  │  requires_tool_result_name,                          │   │   │
│  │  │  requires_assistant_after_tool_result,               │   │   │
│  │  │  requires_thinking_as_text,                          │   │   │
│  │  │  requires_reasoning_content_on_assistant_messages,   │   │   │
│  │  │  thinking_format (OpenAi/OpenRouter/DeepSeek/...,    │   │   │
│  │  │  supports_strict_mode, supports_long_cache_retention │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           RabAnthropicProvider (anthropic.rs)         │   │   │
│  │  │  Custom Anthropic Messages API provider that uses    │   │   │
│  │  │  model_config.base_url and model_config.headers —     │   │   │
│  │  │  unlike yoagent's hardcoded AnthropicProvider.       │   │   │
│  │  │  Enables GitHub Copilot (and other proxies) to serve │   │   │
│  │  │  Anthropic-format models through their own endpoints. │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           OAuth device flow (oauth/)                  │   │   │
│  │  │  Generic OAuth provider trait + registry matching   │   │   │
│  │  │  pi's OAuthProviderInterface.                       │   │   │
│  │  │  - device_code.rs: RFC 8628 device code flow poller │   │   │
│  │  │  - github_copilot.rs: GitHub Copilot OAuth with     │   │   │
│  │  │    model fetch and API key derivation               │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           update.rs (rab update-models subcommand)    │   │   │
│  │  │  Fetches https://models.dev/api.json                │   │   │
│  │  │  Applies pi-style corrections (DeepSeek, Qwen, etc) │   │   │
│  │  │  Writes src/provider/models.json                    │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │                                                              │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │                   yoagent 0.8.4 (MIT)                        │   │
│  │                                                              │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::types                             │   │   │
│  │  │  AgentMessage, Message (User/Assistant/ToolResult),   │   │   │
│  │  │  Content, AgentTool, AgentEvent, Usage, etc.          │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::provider                          │   │   │
│  │  │  Provider trait + StreamProvider trait               │   │   │
│  │  │  OpenAiCompatProvider (fallback), AnthropicProvider,  │   │   │
│  │  │  OpenAiResponsesProvider, GoogleProvider             │   │   │
│  │  │  ModelConfig, CostConfig, ApiProtocol, ThinkingFormat│   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::agent                             │   │   │
│  │  │  Agent struct, run_agent_loop(),                     │   │   │
│  │  │  text/tool streaming, event emission                 │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::skills                            │   │   │
│  │  │  Skill type, frontmatter parsing                     │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │                                                              │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │  Provider backends                                           │   │
│  │  OpenCode Go (opencode.ai/zen/go/v1) — default               │   │
│  │  OpenCode (opencode.ai/zen/v1)                               │   │
│  │  Anthropic — auto-detected by model API config               │   │
│  │  OpenAI — auto-detected by model API config                  │   │
│  │  Google / Ollama — auto-detected by model API config         │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  TUI (src/tui/ + src/agent/ui/) — 55+ modules, ~700+ tests  │   │
│  │  Direct Rust port on crossterm 0.29                          │   │
│  │  Image (Kitty protocol), TerminalColors (OSC 11 detection),  │   │
│  │  TreeSelector, ConfirmOverlay, LoginDialog, OAuthSelector,   │   │
│  │  ScopedModelsSelector                                         │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

## Key architectural decisions

- **yoagent is the core dependency**, not genai. rab delegates the agent loop,
  provider abstraction, and message types to yoagent. rab provides the session layer,
  TUI, built-in tools, file search tools, slash commands, lifecycle management,
  and a custom provider layer on top.

- **Custom provider layer over yoagent** — rab has its own `ProviderRegistry` that
  loads a built-in model catalog (`src/provider/models.json`) merged with user
  overrides (`~/.rab/agent/models.json`). On top of yoagent's providers, rab also
  provides:
  - `RabOpenAiCompatProvider` — custom streaming provider that handles DeepSeek
    thinking format, `reasoning_content`, configurable `max_tokens_field`, and all
    pi `OpenAICompletionsCompat` flags stored in model config headers.
  - `RabAnthropicProvider` — custom Anthropic Messages API provider that respects
    `model_config.base_url` and `model_config.headers`, unlike yoagent's hardcoded
    `AnthropicProvider`. Enables GitHub Copilot (and other proxies) to serve
    Anthropic-format models through their own endpoints.

- **`rab update-models` subcommand** — fetches `https://models.dev/api.json`,
  applies pi-style corrections (DeepSeek, Qwen, Grok, Kimi), and writes
  `src/provider/models.json`. All-or-nothing: any error aborts before writing.

- **Multi-protocol agent selection** — `main.rs` resolves the model via
  `ProviderRegistry`, then selects the appropriate yoagent provider based on
  `ApiProtocol`:
  - `OpenAiCompletions` → `RabOpenAiCompatProvider` (rich compat)
  - `AnthropicMessages` → `RabAnthropicProvider` (custom, respects base_url)
  - `OpenAiResponses` → `yoagent::provider::OpenAiResponsesProvider`
  - `GoogleGenerativeAi` → `yoagent::provider::GoogleProvider`
  - Fallback → `yoagent::provider::OpenAiCompatProvider`

- **One extension mechanism** — built-in tools and user extensions use the same
  `Extension` trait. No separate tool registration path. All tools, commands,
  renderers, and skills go through `Extension`.

- **ToolDefinition wraps every tool** — each `AgentTool` is wrapped in a
  `ToolDefinition` that carries prompt snippet metadata, guidelines, argument
  preparation hooks (`prepare_arguments`), `before_tool_call` and
  `after_tool_call` hooks (pi-compatible), `before_compact` hook, and automatic
  JSON Schema argument coercion + validation.

- **Pluggable operations** — every built-in tool (read, write, edit, bash,
  grep, find, ls) delegates filesystem/shell operations through a trait
  (e.g. `ReadOperations`, `BashOperations`, `GrepOperations`, `FindOperations`,
  `LsOperations`), making it possible to replace local execution with remote (SSH) execution.

- **OAuth support** — `src/provider/oauth/` implements pi's `OAuthProviderInterface`
  with device code flow (RFC 8628) for headless authentication. The GitHub Copilot
  OAuth provider fetches available models after login and auto-enables them.

- **Agent loop lives in yoagent** — rab has no `loop.rs`. yoagent's `Agent`
  struct handles streaming, tool execution, and event emission. rab subscribes
  to events via `AgentEvent` for persistence and UI updates. A fresh `Agent`
  is created per turn (new agent loop per user message), using yoagent's
  native `follow_up()` for mid-turn message queuing and `steer()` for
  turn-level message injection.

- **Types from yoagent** — `AgentMessage`, `Message`, `Content`, `AgentTool`
  are all re-exported from `yoagent::types`. rab's `types.rs` is a thin shim
  with helper functions only (no rab-specific enums).

- **File mutation queue** — concurrent file writes/edits to the same file are
  serialized via `with_file_mutation_queue()` so the model can issue multiple
  sequential edits to the same file without races.

## Pi component mapping

| pi component | rab equivalent | Status |
|---|---|---|
| `pi-tui` (terminal UI, components, editor) | `src/tui/` + `src/agent/ui/` | ✅ Complete — 55+ modules, ~700+ tests. Direct Rust port on crossterm 0.29. Includes Image (Kitty), TerminalColors (OSC 11), TreeSelector, ConfirmOverlay, LoginDialog, OAuthSelector, ScopedModelsSelector. |
| `pi-agent-core` (agent loop, session, compaction, skills) | Delegated to **yoagent** (agent loop, types, provider, skills) + rab's `AgentSession` (session lifecycle, compaction, branching) | ✅ Agent loop in yoagent (`yoagent::agent::Agent`). ✅ Session in `session/model.rs` (~3200+ lines). ✅ SessionStorage in `session/storage.rs` (~660 lines). ✅ Compaction in `compaction.rs` (~1140 lines). ✅ Branch summarization in `branch_summary.rs` (~440 lines). ✅ Skills loaded via `yoagent::skills::SkillSet`. |
| `coding-agent` (CLI, extensions, tools, settings, commands) | `main.rs`, `builtin/`, `extensions/`, `settings.rs`, `auth.rs`, `commands.rs`, `prompt_templates.rs`, `export.rs` | ✅ Tools (read/write/edit/bash/grep/find/ls), settings, auth, CLI done. ✅ 22+ slash commands including `/export`, `/import`, `/share`. ✅ Prompt templates as `/name` commands. ✅ Extension trait with tools, commands, renderers, skills, hooks. |
| `GrepTool`, `FindTool`, `LsTool` (pi agent tools) | `src/extensions/file_search.rs` | ✅ grep (ripgrep/grep fallback), find (fd/find fallback), ls — all with pluggable operations. |
| provider registry + model catalog | `src/provider/` | ✅ `ProviderRegistry` loading built-in + user models.json. ✅ `RabOpenAiCompatProvider` for rich OpenAI-compatible streaming. ✅ `RabAnthropicProvider` for custom Anthropic API. ✅ OAuth support (device code flow, GitHub Copilot). ✅ `rab update-models` subcommand. |
| MCP adapter (pi-mcp-adapter) | `src/extensions/mcp/` (6 modules, ~2040 lines) | ✅ Proxy `mcp` tool, direct tool adapters, SSE-aware HTTP transport, config loading (global+project merge), server lifecycle (lazy connect, idle timeout), persistent metadata cache, tool renderers. |
| provider | `yoagent::provider::*` + `rab::provider::RabOpenAiCompatProvider` + `rab::provider::RabAnthropicProvider` | ✅ Multi-protocol: RabOpenAiCompatProvider (OpenAiCompletions), RabAnthropicProvider (AnthropicMessages), OpenAiResponsesProvider, GoogleProvider. Auto-detection by model config's `ApiProtocol`. |
| `beforeToolCall` / `afterToolCall` | `ToolDefinition.before_tool_call` / `.after_tool_call` | ✅ Per-tool hooks for blocking/preprocessing/postprocessing |
| `validateToolArguments` | `extension::validate_tool_arguments()` | ✅ Full JSON Schema validation with pi-compatible error paths |
| Argument coercion | `extension::coerce_with_json_schema()` | ✅ Type coercion matching pi's `Value.Convert` + `coerceWithJsonSchema` |
| Theme system | `src/agent/ui/theme.rs` | ✅ JSON theme system with resolution, fallback, detection (715 lines) |
| Resource loading (AGENTS.md/CLAUDE.md) | `src/agent/context_files.rs` | ✅ AGENTS.md/CLAUDE.md discovery, `<project_context>` wrapping |
| Skills | `yoagent::skills::SkillSet` + `SystemPromptBuilder.skills()` | ✅ Skill loading, frontmatter, prompt formatting, /skill:name expansion |
| Image support (Kitty protocol) | `src/tui/components/image.rs` + markdown.rs hyperlinks | ✅ Image display via Kitty protocol with dedicated Image component. Input (clipboard paste) TBD. |
| Config files | `~/.rab/` | ✅ Same schema as pi. Auth at `~/.rab/agent/auth.json`. |
| Footer data (git branch, extensions) | `src/agent/footer_data_provider.rs` | ✅ Git branch resolution (worktree/reftable support), extension statuses, provider count |
| File mutation queue | `src/builtin/file_mutation_queue.rs` | ✅ Per-file serialization using tokio::sync::Notify, same pattern as pi |
| MCP extension | `src/extensions/mcp/` | ✅ Proxy `mcp` tool, direct tools, config loading, server lifecycle, cache, renderers |
| Export/Import | `src/builtin/export.rs` | ✅ `/export` (HTML/JSONL), `/import`, `/share` with embedded template assets |
| Prompt templates | `src/agent/prompt_templates.rs` | ✅ `/name` commands from `.md` files, frontmatter, placeholder expansion |
| Path utilities | `src/paths.rs` | ✅ Canonicalization, resolution, display (cross-platform) |
| OAuth | `src/provider/oauth/` | ✅ Device code flow (RFC 8628), GitHub Copilot provider, credential storage |
| WASM plugin system | Not started | ⬜ Phase 2 |

---

## Core type system (`src/agent/types.rs`)

A thin shim over `yoagent::types`. Provides helper functions and rab-specific enums.

### yoagent types (re-exported)

```rust
// From yoagent::types:
pub use yoagent::types::{AgentMessage, Content, Message};
// AgentMessage is an enum:
//   AgentMessage::Llm(Message) — user, assistant, tool_result
//   AgentMessage::Extension(...) — extension-specific data
// Message::User { content, timestamp, ... }
// Message::Assistant { content, model, provider, usage, error_message, ... }
// Message::ToolResult { tool_call_id, tool_name, content, is_error, ... }
// Content::Text { text }
// Content::ToolCall { id, name, arguments }
// Content::Thinking { text, signature }
```

### Helper functions

```rust
pub fn content_text(content: &[Content]) -> String;       // text parts joined
pub fn content_tool_calls(content: &[Content]) -> Vec<(id, name, args)>;
pub fn message_text(msg: &AgentMessage) -> String;
pub fn message_is_user(msg: &AgentMessage) -> bool;
pub fn message_is_assistant(msg: &AgentMessage) -> bool;
pub fn message_is_tool_result(msg: &AgentMessage) -> bool;
pub fn message_is_error(msg: &AgentMessage) -> bool;
pub fn message_tool_call_id(msg: &AgentMessage) -> Option<&str>;
pub fn message_usage(msg: &AgentMessage) -> Option<Usage>;
pub fn message_error(msg: &AgentMessage) -> Option<&str>;
pub fn message_tool_call_count(msg: &AgentMessage) -> usize;
pub fn user_message(text: &str) -> AgentMessage;
pub fn assistant_message(text: &str) -> AgentMessage;
pub fn tool_result_message(tool_call_id: &str, text: String, is_error: bool) -> AgentMessage;
```

---

## Agent lifecycle (`src/agent/agent_session.rs`)

The `AgentSession` struct is the primary entry point for session management.
It owns a `Session` directly (not `SessionManager`) plus app-level config.
Factory methods (`create`, `open`, `in_memory`, etc.) replace the
`SessionManager::create()` pattern.

```rust
pub struct AgentSession {
    session: crate::agent::session::Session,
    session_dir: PathBuf,
    cwd: PathBuf,
    persist: bool,
    flushed: bool,                           // lazy-write state
    last_model: Option<(String, String)>,
    last_thinking_level: String,
    last_active_tools: Option<Vec<String>>,
    persisted_message_ids: HashSet<String>,
    persisted_tool_call_ids: HashSet<String>,
    compaction_settings: CompactionSettings,
    context_window: u64,
    model_name: String,
    compaction_api_key: Option<String>,
    model_config: Option<yoagent::provider::model::ModelConfig>,
    thinking_level: yoagent::types::ThinkingLevel,
    extensions: Vec<Box<dyn Extension>>,
    event_listeners: Vec<CompactionEventCallback>,
    overflow_recovery_attempted: bool,
}
```

### Responsibilities

1. **Factory methods** — `AgentSession::create()`, `.open()`, `.in_memory()`,
   `.continue_recent()`, `.fork_from()` — create sessions directly.

2. **Lazy write** — `ensure_flushed()` migrates from in-memory to file-backed
   storage on the first assistant message (pi-compatible deferred persistence).

3. **Event-driven persistence** — `handle_yo_event()` persists tool results
   immediately (crash-safe). `on_agent_end()` persists remaining messages.

4. **Model/thinking/tool change tracking** — `on_model_change()`,
   `on_thinking_level_change()`, `on_active_tools_change()` append metadata
   entries only when values differ from last known (diff-based).

5. **Auto-compaction** — `check_auto_compact()` runs after the agent finishes
   a turn. Calls `compact()` to generate a summary.

6. **Manual compaction** — `run_manual_compact()` for `/compact`.

7. **Branch summarization** — `summarize_branch_navigation()` summarises
   abandoned branches. `set_branch()` moves the leaf pointer.

8. **New session** — `new_session()` creates a fresh in-memory session.

### Typical usage in print mode

```rust
let mut agent_session = AgentSession::create(&cwd, session_dir.as_deref());
agent_session.set_compaction_config(api_key, &model, context_window, Some(model_config));

// Submit user message
let msg = user_message("list .rs files");
agent_session.send_user_message_obj(&msg);

// Spawn yoagent agent loop
let agent = yoagent::agent::Agent::new(RabOpenAiCompatProvider)  // or AnthropicProvider, etc.
    .with_model(&model)
    .with_api_key(&api_key)
    .with_model_config(model_config)
    .with_system_prompt(&system_prompt)
    .with_tools(agent_tools);

agent.prompt_with_sender(msg_text, tx).await;

// Process events — AgentSession persists tool results immediately
while let Some(event) = rx.recv().await {
    agent_session.on_agent_event(&event);
    // Update UI ...
}

// AgentEnd persists remaining messages
agent_session.check_auto_compact().await;
```

---

## Session layer (`src/agent/session/`) — `mod.rs` + `model.rs`, ~3200+ lines

Pi-compatible three-layer architecture:

```
AgentSession → Session → SessionStorage
                (high-level)  (low-level CRUD)
```

### Session (high-level wrapper)

Wraps `Box<dyn SessionStorage>` and provides pi-compatible entry construction,
context building, and branch navigation. All `append_*` methods generate typed
`SessionEntry` values with auto-generated IDs, parent chains, and timestamps.

```rust
pub struct Session {
    storage: Box<dyn SessionStorage>,
}
```

Key methods: `append_message()`, `append_model_change()`,
`append_thinking_level_change()`, `append_compaction()`,
`append_branch_summary()`, `append_label_change()`, `append_custom_entry()`,
`append_custom_message_entry()`, `build_context()`, `move_to()`,
`get_branch()`, `get_entries()`, `get_entry()`, `get_leaf_id()`,
`get_label()`, `get_session_name()`, `find_entries()`, `session_id()`,
`session_file()`, `session_name()`, `metadata()`, `set_leaf_id()`.

### SessionManager (internal helper)

`SessionManager` still exists as an internal helper behind the scenes.
It provides factory methods (`create`, `open`, `in_memory`, etc.) and
lazy-write tracking that `AgentSession` uses during construction.
Not in the public API path — `main.rs` and `App` use `AgentSession` directly.

### Format

JSONL file, one object per line. Same format as pi's sessions.

```jsonl
{"type":"session","version":3,"id":"01J...","cwd":"/home/user/project",...}
{"type":"message","id":"m1","parentId":null,"role":"user","content":"list .rs files","timestamp":"..."}
{"type":"model_change","id":"mc1","parentId":"m1","provider":"opencode_go","model_id":"deepseek-v4-pro","timestamp":"..."}
{"type":"message","id":"m2","parentId":"mc1","role":"assistant","content":"Found 3 files...","timestamp":"..."}
{"type":"message","id":"m3","parentId":"m2","role":"toolResult","toolCallId":"tool_01","content":"src/main.rs\n",...}
{"type":"compaction","id":"c1","parentId":"...","summary":"...","firstKeptEntryId":"...","tokensBefore":45000,...}
{"type":"branch_summary","id":"bs1","parentId":"...","fromId":"m10","summary":"Explored refactoring...",...}
```

### Entry types

```rust
pub enum SessionEntry {
    Message(MessageEntry),                          // conversation messages
    ThinkingLevelChange(ThinkingLevelChangeEntry),  // metadata
    ModelChange(ModelChangeEntry),                  // metadata
    ActiveToolsChange(ActiveToolsChangeEntry),      // metadata
    Compaction(CompactionEntry),                    // compaction summary
    BranchSummary(BranchSummaryEntry),              // branch navigation summary
    SessionInfo(SessionInfoEntry),                  // session name
    Label(LabelEntry),                              // tree node labels
    Custom(CustomEntry),                            // extension data
    CustomMessage(CustomMessageEntry),              // extension messages
    Leaf(LeafEntry),                                // tree leaf pointer
}
```

### Versioning

Current session version: **3**. Each entry has a unique `id` and optional
`parentId`, forming a tree. Branching writes a persistent `LeafEntry`
(pi-compatible durability — leaf position survives crashes).

---

## Session storage (`src/agent/session/storage.rs`) — ~660 lines

Pi-compatible `SessionStorage` trait with full CRUD operations.
Both implementations fully own their state (entries, by_id, labels, leaf_id).

### SessionStorage trait

```rust
pub trait SessionStorage: Send {
    fn metadata(&self) -> SessionMetadata;
    fn get_leaf_id(&self) -> Option<String>;
    fn set_leaf_id(&mut self, leaf_id: Option<&str>) -> Result<(), String>;
    fn create_entry_id(&self) -> String;
    fn append_entry(&mut self, entry: SessionEntry) -> Result<(), String>;
    fn get_entry(&self, id: &str) -> Option<SessionEntry>;
    fn find_entries(&self, type_name: &str) -> Vec<SessionEntry>;
    fn get_label(&self, id: &str) -> Option<String>;
    fn get_path_to_root(&self, leaf_id: Option<&str>) -> Result<Vec<SessionEntry>, String>;
    fn get_entries(&self) -> Vec<SessionEntry>;
    fn path(&self) -> Option<&Path>;
}
```

### Implementations

- **`JsonlSessionStorage`** — file-backed. Loads all entries into memory on
  open, persists on every `append_entry()` / `set_leaf_id()`. Leaf entries
  are written as `type: "leaf"` JSONL lines (pi-compatible).
- **`InMemorySessionStorage`** — fully in-memory. Used for `--no-session` mode
  and as initial storage before the first lazy flush.

### Leaf persistence

Branch navigation (`move_to` / `set_branch`) writes a persistent `LeafEntry`
with `type: "leaf"` and `targetId`, matching pi's durability model. The leaf
position survives crashes and is restored when the session file is reopened.

---

## Session repo (`src/agent/session/repo.rs`)

Higher-level session lifecycle management:

```rust
pub trait SessionRepo {
    fn list(&self, session_dir: &Path, filter_cwd: Option<&Path>,
            progress: Option<&dyn Fn(usize, usize)>) -> Vec<SessionInfo>;
    fn list_all(&self, progress: Option<&dyn Fn(usize, usize)>) -> Vec<SessionInfo>;
    fn delete(&self, path: &Path) -> io::Result<()>;
    fn fork(&self, source_path: &Path, target_dir: &Path,
            entry_id: Option<&str>, position: Option<&str>) -> io::Result<String>;
    fn load_info(&self, path: &Path) -> Option<SessionInfo>;
}
```

`DefaultSessionRepo` provides progress-aware, concurrent listing with
cwd filtering for the session picker UI.

---

## Compaction (`src/agent/compaction.rs`) — ~1140 lines ✅ IMPLEMENTED

When the conversation approaches the model's context window, older messages
are summarized to free space. Ported from pi's compaction algorithm.

### Algorithm

1. **Prepare** (`prepare_compaction()`) — Find the previous compaction
   boundary, scan entries, detect the cut point based on `keep_recent_tokens`.
   Handles split-turn detection (user message vs. mid-turn cut).

2. **Check threshold** (`should_compact()`) — If total tokens exceed
   `context_window - reserve_tokens`, compaction triggers.

3. **Summarize** (`compact()`) — Send older messages to the provider with a
   structured summarization prompt. Supports incremental updates (previous
   summary in `<previous-summary>` tags). Handles turn-prefix summarization
   for split turns.

4. **Replace** — Append a `CompactionEntry` to the session. The entry
   contains the summary, `first_kept_entry_id`, token count, and file
   operation details (readFiles/modifiedFiles).

### Settings

```rust
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,       // tokens reserved for system + response
    pub keep_recent_tokens: u64,   // newest tokens always kept
}
```

Defaults: enabled, 16K reserve, 20K keep recent. Configurable via settings
(`autoCompact`, `compactReserveTokens`, `compactKeepRecentTokens`).

### Manual trigger

Via `/compact` slash command → `AgentSession::run_manual_compact()` →
`compact()` → append `CompactionEntry`.

### Shared summarization helper

`summarize_text()` (shared with `branch_summary.rs`) calls the provider
with a non-streaming text completion (no tools, low temperature) to
generate summaries.

---

## Branch summarization (`src/agent/branch_summary.rs`) — ~440 lines ✅ IMPLEMENTED

When the user navigates to a different branch in the session tree, the
abandoned branch is summarized so context is preserved.

### Algorithm

1. **Collect entries** (`collect_entries_for_branch_summary()`) — Walk from
   `old_leaf_id` back to the common ancestor with `target_id`. Produces
   chronological list of abandoned entries.

2. **Prepare messages** (`prepare_branch_entries()`) — Filter entries,
   respecting token budget. Tool results are skipped (context in assistant's
   tool calls). Compaction/branch summary entries are prioritized.

3. **Generate summary** (`generate_branch_summary()`) — Call the provider
   with a structured prompt (Goal / Progress / Key Decisions / Next Steps).
   Append a `BranchSummaryEntry` with file operation details (readFiles,
   modifiedFiles extracted from tool calls).

---

## Provider layer (`src/provider/`)

### ProviderRegistry (`mod.rs`)

The provider registry loads a built-in model catalog from `src/provider/models.json`
and overlays user overrides from `~/.rab/agent/models.json`. It resolves model IDs
to `ResolvedModel` structs containing the `ModelConfig` (base URL, API protocol,
compat flags, pricing, context window) and the API key.

```rust
pub struct ProviderRegistry {
    entries: Vec<ProviderEntry>,
    auth_storage: crate::auth::AuthStorage,
}

impl ProviderRegistry {
    pub fn load(agent_dir: &Path) -> anyhow::Result<Self>;
    pub fn reload(&mut self, agent_dir: &Path) -> anyhow::Result<()>;
    pub fn resolve(&self, model_id: &str, preferred_provider: Option<&str>)
        -> anyhow::Result<ResolvedModel>;
    pub fn list_models(&self) -> Vec<String>;
    pub fn provider_for_model(&self, model_id: &str, preferred_provider: Option<&str>)
        -> Option<String>;
    pub fn api_key_for_provider(&self, provider_id: &str) -> Option<String>;
    pub fn count_providers(&self) -> usize;
}
```

### Model resolution

Resolution order:
1. If `preferred_provider` is set, that provider is checked first
2. Otherwise returns the first provider that has the given model ID
3. API key resolved from: `auth.json` → environment variable → empty string

### models.rs — models.json parsing

Parses the built-in and user `models.json` files. Each provider entry contains:

```json
{
  "providers": {
    "opencode-go": {
      "name": "OpenCode Zen Go",
      "baseUrl": "https://opencode.ai/zen/go",
      "api": "openai-completions",
      "env": { "apiKey": "OPENCODE_API_KEY" },
      "models": [
        {
          "id": "deepseek-v4-flash",
          "name": "DeepSeek V4 Flash",
          "api": "openai-completions",
          "reasoning": true,
          "input": ["text"],
          "cost": { "input": 0.15, "output": 0.6, "cacheRead": 0.0, "cacheWrite": 0.0 },
          "contextWindow": 128000,
          "maxTokens": 16384,
          "compat": {
            "supportsStore": false,
            "supportsDeveloperRole": false,
            "maxTokensField": "max_tokens",
            "requiresReasoningContentOnAssistantMessages": true,
            "thinkingFormat": "deepseek"
          }
        }
      ]
    }
  }
}
```

Supported `api` types: `openai-completions`, `anthropic-messages`,
`openai-responses`, `google-generative-ai`, `google-vertex`,
`bedrock-converse-stream`, `azure-openai-responses`.

User-provided providers with the same `id` replace built-in entries entirely (merge semantics).

### RabOpenAiCompatProvider (`openai_compat.rs`)

Custom streaming provider implementing `yoagent::provider::traits::StreamProvider`.
Replaces yoagent's `OpenAiCompatProvider` with richer compat handling:

- **Thinking format**: supports `reasoning_content` in delta chunks for DeepSeek,
  OpenRouter, Together, ZAI, Qwen, and other providers
- **Thinking control**: DeepSeek uses `thinking: { type: "enabled" | "disabled" }`
  instead of `reasoning_effort`
- **`requires_reasoning_content_on_assistant_messages`**: for providers like DeepSeek
  that need `reasoning_content` on replayed assistant messages
- **Configurable `max_tokens_field`**: `max_tokens` vs `max_completion_tokens`
- **`requires_assistant_after_tool_result`**: inserts empty assistant message after
  tool results for providers that require it
- **`requires_tool_result_name`**: includes `name` field in tool result messages
- **Full SSE streaming**: parses OpenAI-compatible SSE chunks, handles
  `reasoning_content` delta, tool call deltas, usage info

Request body construction accounts for all compat flags:
- `developer` vs `system` role
- `thinking: { type }` vs `reasoning_effort`
- `max_tokens` vs `max_completion_tokens`
- Tool definition format with `strict` mode (default: true)

### RabOpenAiCompat (`compat.rs`)

Rich compatibility flags matching pi's `OpenAICompletionsCompat` schema,
stored as JSON in `ModelConfig::headers["_rab_compat"]`:

```rust
pub struct RabOpenAiCompat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_thinking_control: bool,
    pub supports_usage_in_streaming: bool,
    pub max_tokens_field: RabMaxTokensField,         // MaxTokens | MaxCompletionTokens
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub requires_thinking_as_text: bool,
    pub requires_reasoning_content_on_assistant_messages: bool,
    pub thinking_format: RabThinkingFormat,          // OpenAi | OpenRouter | DeepSeek | Together | Zai | Qwen | ChatTemplate | QwenChatTemplate | StringThinking | AntLing
    pub supports_strict_mode: bool,
    pub supports_long_cache_retention: bool,
}
```

### RabAnthropicProvider (`anthropic.rs`)

Custom Anthropic Messages API provider that uses `model_config.base_url`
and forwards `model_config.headers` — unlike yoagent's `AnthropicProvider`
which hardcodes `https://api.anthropic.com` and ignores headers.
This allows GitHub Copilot (and other proxies) to serve Anthropic-format
models through their own endpoints.

### OAuth (`oauth/`)

Generic OAuth provider trait and registry matching pi's `OAuthProviderInterface`.

| Module | Description |
|--------|-------------|
| `oauth/mod.rs` | `OAuthProvider` trait, `OAuthCredentials`, `DeviceCodeInfo`, registry |
| `oauth/device_code.rs` | RFC 8628 device code flow poller with timeout, slow-down handling, cancellation |
| `oauth/github_copilot.rs` | GitHub Copilot OAuth: device code login, model fetch, auto-enable |

### update.rs — `rab update-models` subcommand

Fetches `https://models.dev/api.json`, processes target providers
(currently `opencode` and `opencode-go`), applies pi-style corrections:

| Model | Correction |
|-------|-----------|
| `deepseek-v4*` | `requiresReasoningContentOnAssistantMessages: true`, `thinkingFormat: deepseek`, `supportsReasoningEffort: false`, `thinkingLevelMap` with `high`/`max` mapping |
| `kimi-k2.6` | `thinkingFormat: deepseek`, `supportsReasoningEffort: false` |
| `kimi-k2.5` | `supportsLongCacheRetention: false` |
| `minimax-m2.7` | `supportsLongCacheRetention: false` |
| `grok-build-0.1` | `supportsReasoningEffort: false`, `thinkingLevelMap` limiting levels |
| `qwen3*` (opencode-go) | `thinkingFormat: qwen` |

---

## Extension trait (`src/agent/extension.rs`) — ~1100 lines

All capability — built-in or user-provided — comes through the same trait.
Supports pi-compatible `beforeToolCall` / `afterToolCall` hooks, `before_compact` hook,
argument type coercion via JSON Schema, and full schema validation.

```rust
#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> Cow<'static, str>;
    fn tools(&self) -> Vec<ToolDefinition> { vec![] }
    fn commands(&self) -> Vec<SlashCommand> { vec![] }
    fn skills(&self) -> yoagent::skills::SkillSet { yoagent::skills::SkillSet::empty() }
}
```

### Supporting types

```rust
pub struct ToolDefinition {
    pub tool: Box<dyn yoagent::types::AgentTool>,
    pub snippet: &'static str,
    pub guidelines: &'static [&'static str],
    pub prepare_arguments: Option<fn(serde_json::Value) -> Result<serde_json::Value, String>>,
    pub before_tool_call: Option<fn(&serde_json::Value) -> Option<BeforeToolCallResult>>,
    pub after_tool_call: Option<fn(&yoagent::types::ToolResult, bool) -> Option<AfterToolCallResult>>,
}

pub struct BeforeToolCallResult { pub block: bool, pub reason: String }
pub struct AfterToolCallResult { pub content: Option<Vec<Content>>, pub details: Option<Value>, pub is_error: Option<bool> }
pub struct BeforeCompactResult { pub cancel: bool, pub summary: Option<String>, pub details: Option<Value> }

pub struct AutocompleteItem { pub value: String, pub label: String, pub description: Option<String> }

pub trait CommandHandler: Send + Sync {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult>;
    fn argument_completions(&self, prefix: &str) -> Vec<AutocompleteItem>;
}

pub enum CommandResult { Info(String), Quit, ModelChanged(String), ShowHelp, Reloaded,
    NewSession, SessionSwitched, SessionInfo, OpenSessionSelector, SessionNamed,
    OpenSettings, ScopedModels, ExportSession, ImportSession, ShareSession,
    CopyLastMessage, ShowChangelog, ForkSession, CloneSession, SessionTree,
    TrustDecision, Login, Logout, CompactSession, ... }

pub struct SlashCommand { pub name: String, pub description: String, pub handler: Box<dyn CommandHandler> }

pub trait ToolRenderer: Send + Sync {
    fn render_call(&self, args: &Value, width: usize, theme: &dyn Theme, ctx: &ToolRenderContext) -> Vec<String>;
    fn render_result(&self, content: &str, width: usize, theme: &dyn Theme, ctx: &ToolRenderContext) -> Vec<String>;
    fn render_self(&self) -> bool;
    fn render_bg_key(&self) -> Option<&'static str>;
}

pub struct ToolRenderContext {
    pub expanded: bool,
    pub args_complete: bool,
    pub is_partial: bool,
    pub is_error: bool,
    pub cwd: String,
    pub duration_secs: Option<f64>,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub was_truncated: bool,
    pub full_output_path: Option<String>,
    pub file_path: Option<String>,
    pub expand_key: String,
    pub details: Option<serde_json::Value>,
    pub invalidate: Option<UnboundedSender<()>>,
}
```

### Argument coercion and validation

Every tool call goes through `ToolDefinition::execute()`:

1. **`prepare_arguments`** — custom per-tool pre-processing
2. **`coerce_with_json_schema()`** — recursive type coercion for common LLM mistakes
3. **`validate_tool_arguments()`** — full JSON Schema validation with pi-compatible error paths
4. **`before_tool_call`** — optional hook that can block execution
5. **Execute** — call inner `AgentTool::execute()`
6. **`after_tool_call`** — optional hook that can modify the result

At startup, extensions are collected from builtins + file_search + mcp:

```rust
let extensions: Vec<Box<dyn Extension>> = vec![
    Box::new(CommandsExtension::new(available_models)),
    Box::new(ReadExtension::new(cwd)),
    Box::new(WriteExtension::new(cwd)),
    Box::new(EditExtension::new(cwd)),
    Box::new(BashExtension::new(cwd)),
    Box::new(FileSearchExtension::new(cwd)),  // grep, find, ls
    Box::new(McpExtension::from_cwd(&cwd)),   // MCP tools
];
```

Extension gating is done via `is_extension_active()` which checks
`settings.tools` (whitelist) and `settings.exclude_tools` (blacklist).
Core extensions (commands, read, write, edit, bash, mcp) are always active
when no whitelist is set. Grep/find/ls are opt-in and bundled as a single
`FileSearchExtension`, activated if any of `"grep"`, `"find"`, or `"ls"`
is whitelisted in `settings.tools`.

---

## Built-in extensions (`builtin/`)

### export — `/export`, `/import`, `/share`

Pi-compatible session export/import with embedded template assets:

| Command | Description |
|---------|-------------|
| `/export [path]` | Export session to HTML (default) or JSONL (`.jsonl` extension) |
| `/import <path>` | Import and resume a session from a JSONL file |
| `/share` | Share as secret GitHub gist (requires `gh` CLI) |

Template assets (`template.html`, `template.css`, `template.js`, `marked.min.js`,
`highlight.min.js`) are embedded via `include_bytes!` at compile time.

### commands — ✅ 22 slash commands

| Command | Result | Description |
|---------|--------|-------------|
| `/quit` | `Quit` | Graceful shutdown |
| `/model <name>` | `ModelChanged(name)` / `Info` | Switch model; no args lists available |
| `/settings` | `OpenSettings` | Open settings menu overlay |
| `/scoped-models` | `ScopedModels` | Enable/disable models for Ctrl+P cycling |
| `/export [path]` | `ExportSession { path }` | Export session (HTML or .jsonl) |
| `/import <path>` | `ImportSession { path }` | Import and resume a session |
| `/share` | `ShareSession` | Share as secret GitHub gist |
| `/copy` | `CopyLastMessage` | Copy last assistant message to clipboard |
| `/name <name>` | `SessionNamed { name }` | Set session display name |
| `/session` | `SessionInfo { ... }` | Show session info and stats |
| `/changelog` | `ShowChangelog` | Show changelog entries |
| `/hotkeys` | `ShowHelp` | Show keyboard shortcuts |
| `/fork [msg-id]` | `ForkSession { message_id }` | Fork from a previous message |
| `/clone` | `CloneSession` | Duplicate the current session |
| `/tree` | `SessionTree` | Navigate session tree |
| `/trust <decision>` | `TrustDecision { decision }` | Save project trust decision |
| `/login [provider]` | `Login { provider }` | Configure provider auth |
| `/logout [provider]` | `Logout { provider }` | Remove provider auth |
| `/new` | `NewSession` | Clear conversation |
| `/compact` | `CompactSession` | Manually compact session context |
| `/resume` | `OpenSessionSelector` | Open session selector |
| `/reload` | `Reloaded` | Reload keybindings, extensions, skills, prompts, themes |

### Built-in tools

| Tool | Key features |
|------|-------------|
| **read** | Path resolution, line numbers, 50KB truncation. Image support (base64 data URL). Pluggable `ReadOperations` trait. |
| **write** | Temp file + atomic rename, parent dir creation. `prepare_write_args` for type coercion. Preview rendering with syntax highlighting (partial+full). |
| **edit** | Exact-match search/replace, error on zero/multiple matches. Diff rendering with intra-line character-level inverse. Pluggable `EditOperations` trait. File mutation queue for serialized concurrent edits. |
| **bash** | `sh -c <command>`, configurable timeout, streaming via ToolProgress. Last 2000 lines / 50KB truncation. Pluggable `BashOperations` trait, command prefix, custom shell path. |

### File search extension (`src/extensions/file_search.rs`)

| Tool | Key features |
|------|-------------|
| **grep** | Uses ripgrep (`rg`) with `--json` output, falls back to `grep`. Respects .gitignore. Options: pattern, path, glob, ignoreCase, literal, context, limit (default 100). Pluggable `GrepOperations`. Shared `run_shell_command()` helper. |
| **find** | Uses `fd` (rust rewrite of find) with glob matching and .gitignore awareness, falls back to `find -name`. Options: pattern, path, limit (default 1000). Pluggable `FindOperations`. Shared `run_shell_command()` helper. |
| **ls** | Directory listing with `/` suffix for directories, dotfiles included. Options: path, limit (default 500). Pluggable `LsOperations`. Pure Rust via `std::fs::read_dir`. |

### File mutation queue (`src/builtin/file_mutation_queue.rs`)

Serializes concurrent file mutations targeting the same file path.
Different files run in parallel. Uses `tokio::sync::Notify` stored in a
global `LazyLock<Mutex<HashMap>>`. Operations chain through `Notify`
signals (each operation waits for the previous one's signal, then stores
its own for the next). Cleanup removes stale entries.

```rust
pub async fn with_file_mutation_queue<T, E, F, Fut>(
    file_path: &str,
    cwd: &Path,
    f: F,
) -> Result<T, E>
```

---

## Auth (`src/auth.rs`)

API key and OAuth credential storage, loaded from `~/.rab/agent/auth.json`.

```rust
pub enum AuthCredential {
    ApiKey { key: String },
    Oauth { access: String, refresh: Option<String>, expires: Option<i64>, enterprise_url: Option<String> },
}

pub struct AuthStorage(HashMap<String, AuthCredential>);
```

Load order: `~/.rab/agent/auth.json`. Supports multiple providers (api_key
or oauth). Methods: `api_key(provider)`, `oauth_token(provider)`.

---

## Footer data provider (`src/agent/footer_data_provider.rs`)

Provides git branch, extension statuses, and provider count to the Footer
on a **pull** basis. Owned by the App behind `Rc<RefCell<>>`.

Git branch resolution:
1. Walk up from `cwd` looking for `.git`
2. If `.git` is a file → worktree: parse `gitdir:` path, find HEAD
3. If `.git` is a directory → regular repo: find HEAD
4. Read HEAD file; if `ref: refs/heads/.invalid` → fall back to git
5. Otherwise treat as detached HEAD

Reftable repos are detected via the `.invalid` sentinel and fall back to
`git symbolic-ref --short HEAD`.

```rust
pub struct FooterDataProvider {
    cwd: PathBuf,
    git_branch: Option<String>,
    extension_statuses: BTreeMap<String, String>,
    available_provider_count: usize,
}
```

---

## Path utilities (`src/paths.rs`) — ~250 lines

Centralized path handling matching pi's `packages/coding-agent/src/utils/paths.ts`:

- `canonicalize()` — canonicalize with Windows `\\\\?` prefix stripping
- `resolve_path()` — resolve relative/absolute paths, expand `~`
- `display_path()` — display path relative to cwd or home

## Settings (`src/agent/settings.rs`) — ~800 lines

Same file names and format as pi, under `~/.rab/agent/`.

### Config files

| Pi path | rab path | Status |
|---|---|---|
| `~/.pi/agent/settings.json` | `~/.rab/agent/settings.json` | ✅ |
| `.pi/settings.json` | `.rab/settings.json` | ✅ (project-local overrides) |
| `~/.pi/agent/auth.json` | `~/.rab/agent/auth.json` | ✅ |
| `~/.pi/agent/models.json` | `~/.rab/agent/models.json` | ✅ (user overrides, merged with built-in) |
| `~/.pi/agent/AGENTS.md` | `~/.rab/agent/AGENTS.md` | ✅ |
| `AGENTS.md` / `CLAUDE.md` | `AGENTS.md` / `CLAUDE.md` | ✅ (project + ancestor walk) |
| `~/.pi/agent/keybindings.json` | `~/.rab/keybindings.json` | ✅ |
| `~/.pi/agent/sessions/` | `~/.rab/sessions/` | ✅ |
| `~/.pi/agent/skills/` | `~/.rab/skills/` | ✅ |
| `~/.pi/agent/themes/` | `~/.rab/themes/` + embedded | ✅ |

### `settings.json` format

```json
{
    "defaultModel": "deepseek-v4-flash",
    "defaultThinkingLevel": "high",
    "defaultProvider": "opencode_go",
    "tools": ["read", "write", "edit", "bash"],
    "excludeTools": [],
    "theme": "dark",
    "verbose": false,
    "hideThinkingBlock": true,
    "collapseToolOutput": true,
    "autoCompact": true,
    "compactReserveTokens": 16384,
    "compactKeepRecentTokens": 20000
}
```

Load order: global `~/.rab/agent/settings.json`, then project `.rab/settings.json`
overlays. CLI flags (`--model`, `--thinking`, `--no-context-files`, etc.) take
precedence over both. Settings modifications during a session (Ctrl+T, Ctrl+O,
Ctrl+Shift+C toggles) are tracked via `modified_fields` and only written
back for fields that were explicitly changed (preserving project-level overrides).

---

## System prompt (`src/agent/system_prompt.rs`) — ~400 lines

Built via `SystemPromptBuilder`:

1. **Default prompt** — tool descriptions, response format, tool guidelines,
   available tools from `ToolDefinition.snippet` / `.guidelines`.
2. **Custom SYSTEM.md** — `~/.rab/agent/SYSTEM.md` (global) or `.rab/SYSTEM.md`
   (project). Replaces the default prompt.
3. **APPEND_SYSTEM.md** — appended after all prompts.
4. **Context files** — AGENTS.md/CLAUDE.md walked from cwd to root,
   wrapped in `<project_instructions path="...">` tags.
5. **Skills** — available skills listed as `<available_skills>` XML.
6. **Date and cwd** — `Current date: YYYY-MM-DD`, `Current working directory: /path`.

Skills from extensions are merged via `skill_set.merge(ext.skills())`.

---

## Prompt templates (`src/agent/prompt_templates.rs`) — ~570 lines

Loads `.md` files from `~/.rab/agent/prompts/` (global) and `.rab/prompts/` (project)
and registers them as `/name` commands. Follows pi's prompt template system:

- Filename (minus `.md`) becomes the `/name` command
- Frontmatter supports `description` and `argument-hint`
- Body supports `$1`, `$2`, `$@`, `$ARGUMENTS`, `${N:-default}`, `${@:N}`, `${@:N:L}`
- Later entries override earlier ones on name conflict

## CLI (`main.rs`) — manual arg parsing

```
rab [MESSAGE]...

Subcommands:
  rab update-models          Fetch and update built-in model catalog

Session:
  -c, --continue             Continue most recent session in cwd
  -r, --resume               Open interactive session picker
  --session PATH             Open specific session file (path or partial ID)
  --session-id ID            Create/open session with explicit ID
  --fork PATH                Fork a session from another session file
  --export PATH              Export session and exit (⚠ not yet implemented)
  --no-session               Ephemeral, don't save
  -n, --name <name>          Set session name

Model:
  --model MODEL              Model name (default: from settings.json)

Context:
  -nc, --no-context-files    Skip AGENTS.md/CLAUDE.md loading
  --system-prompt <text>     Replace default system prompt
  --append-system-prompt <text>  Append to system prompt

Other:
  --session-dir <path>       Session storage directory override
```

Session resolution supports both paths and partial UUID prefixes.
`--fork` resolves the session file, validates no conflicts with `--session-id`,
and calls `AgentSession::fork_from()`.

Extension gating: `is_extension_active()` checks `settings.tools` whitelist
and `settings.exclude_tools` blacklist. Core extensions (commands, read,
write, edit, bash, mcp) are always active when no whitelist is set. File
search tools (grep, find, ls) are opt-in via the bundled `FileSearchExtension`.

---

## Run modes

### Print mode

```
$ rab "What does git status do?"
Shows the current state of the working directory and staging area...
```

```
$ cat README.md | rab "Summarize this"
```

Streams the response to stdout. Thinking blocks shown dimmed on stderr.
Tool calls and results shown prefixed with colored indicators. Has a 120s
timeout to prevent hanging on stuck providers. Uses a simple event loop:
`yoagent::agent::Agent::prompt_with_sender()` → process `AgentEvent` stream →
`agent_session.on_agent_event()` for persistence.

Provider selection in print mode matches interactive mode — the model is
resolved through `ProviderRegistry`, then the appropriate provider is chosen
based on `ApiProtocol`:

```rust
let agent = match mc.api {
    ApiProtocol::OpenAiCompletions =>
        yoagent::agent::Agent::new(RabOpenAiCompatProvider),
    ApiProtocol::AnthropicMessages =>
        yoagent::agent::Agent::new(RabAnthropicProvider),
    ApiProtocol::OpenAiResponses =>
        yoagent::agent::Agent::new(yoagent::provider::OpenAiResponsesProvider),
    ApiProtocol::GoogleGenerativeAi =>
        yoagent::agent::Agent::new(yoagent::provider::GoogleProvider),
    _ => yoagent::agent::Agent::new(yoagent::provider::OpenAiCompatProvider),
};
```

### Interactive (TUI) mode

Same agent loop, different sink: `App` in `src/agent/ui/` subscribes to the
agent event stream and renders to a pi-tui-style main-screen TUI instead of
stdout. Uses `yoagent::agent::Agent::prompt_with_sender()` with event channels.

---

## TUI (`src/tui/` + `src/agent/ui/`) — 55+ modules, ~700+ tests

The TUI library is a direct Rust port of pi's `@earendil-works/pi-tui`.

### Core (`src/tui/`)

| Module | Description |
|--------|-------------|
| `tui_core.rs` | `TUI` struct — event loop, render loop, overlay system, screen diff |
| `component.rs` | `Component` trait, `Size`, `RenderContext` |
| `container.rs` | Container layout — vertical, centered, flex-grow, children |
| `focusable.rs` | Focus management — `Focusable` trait, focus ring |
| `screen.rs` | Screen diff renderer — line-by-line comparison with cursor markers |
| `overlay.rs` | Overlay system — show/hide/composite, anchor-based positioning |
| `terminal.rs` | Terminal abstraction — `TerminalTrait`, `ProcessTerminal`, raw mode, cursor hide/show, synchronized output, `TerminalEvent::Key`/`Paste`/`Resize` |
| `keys.rs` | Key event handling — `key_event_to_id()`, 30+ action IDs, kitty protocol key encoding |
| `keybindings.rs` | JSON keybinding loading from `~/.rab/keybindings.json`, merge, resolution, 50+ action constants |
| `theme.rs` | Theme trait + default JSON theme loader |
| `fuzzy.rs` | Fuzzy matching for autocomplete |
| `autocomplete.rs` | Editor autocomplete popup — completions, rendering, keyboard navigation |
| `kill_ring.rs` | Kill ring for editor cut/copy/paste |
| `undo_stack.rs` | Undo/redo for editor |
| `word_nav.rs` | Word-boundary navigation utilities |
| `visual_truncate.rs` | Shared `truncate_to_visual_lines()` utility |
| `terminal_colors.rs` | Terminal color scheme detection — parses OSC 11 responses |
| `util.rs` | Shared utilities |

### Components (`src/tui/components/`)

| Module | Description |
|--------|-------------|
| `editor.rs` | Multi-line editor — word-wrap, undo stack, kill ring, paste markers, bracketed paste, history recall, character jump, sticky column, border_color, autocomplete |
| `markdown.rs` | comrak-based renderer with syntax highlighting, tables, code blocks, Kitty hyperlinks |
| `diff.rs` | Unified diff with colored +/lines and intra-line character-level inverse |
| `image.rs` | Inline image via Kitty terminal protocol, fallback to text summary |
| `box.rs` | `Box` component with render cache, borders, backgrounds |
| `text.rs` | `Text` / `TruncatedText` with RefCell cache |
| `spacer.rs` | Vertical spacer |
| `select_list.rs` | Two-column selectable list with prefix filter |
| `loader.rs` | Animated loader with timer, colors, abort support |
| `dynamic_lines.rs` | Dynamically-sized section component |
| `rc_ref_cell_component.rs` | `RcRefCellComponent` bridge for shared ownership components |

### App layer (`src/agent/ui/`)

| Module | Description |
|--------|-------------|
| `app.rs` | Main `App` struct — event handler, agent loop management, message queuing, compose_ui, bang commands (!/!!), skills expansion |
| `chat_editor.rs` | `ChatEditor` wrapper — input processing, slash command dispatch, /skill:name expansion. `InputAction` enum with `Handled`, `Escape`, `Clear`, `Exit`, `ThinkingCycle`, `ModelSelector`, `ModelCycleForward`, `ModelCycleBackward`, `ToggleThinking`, `ToolsExpand`, `EditorExternal`, `Help`, `Submit`, `FollowUp`, `Dequeue`, `CompactToggle` |
| `theme.rs` | `RabTheme` — JSON theme resolution, fallback, color detection, 130+ theme keys |
| `working.rs` | `WorkingIndicator` — timer-based working animation |
| `model_selector.rs` | `ModelSelector` — Ctrl+P model cycling with scoped-models support |
| `footer.rs` | `Footer` — cwd, git branch, token usage, model, auto-compact indicator |
| `help.rs` | Help overlay with keybinding display |
| `render_utils.rs` | Shared rendering utilities |
| `components/header.rs` | Header — "rab" logo, keybinding hints |
| `components/footer_component.rs` | Footer component |
| `components/editor_component.rs` | Editor component with border_color |
| `components/user_message.rs` | User message component (box + markdown) |
| `components/assistant_message.rs` | Streaming assistant message with thinking blocks |
| `components/tool_messages.rs` | Tool execution components (read, write, edit, bash, grep, find, ls) with dedicated renderers |
| `components/info_message.rs` | Info message component (dim text) |
| `components/session_picker.rs` | Session selector overlay |
| `components/tree_selector.rs` | Full-screen session tree navigation with filtering, folding, labels |
| `components/confirm_overlay.rs` | Generic confirmation dialog with yes/no |
| `components/login_dialog.rs` | Login dialog for OAuth flows (prompt, device code, auth URL) |
| `components/oauth_selector.rs` | Provider selector with search and auth status display |
| `components/scoped_models_selector.rs` | Enable/disable models for Ctrl+P cycling |
| `components/mod.rs` | Component re-exports |

### Layout (component tree)

```
Terminal (no alternate screen):
TUI.root (Container):
  ├── HeaderComponent ("rab" logo, keybinding hints)
  ├── chat_container (RefContainer)
  │   ├── UserMessageComponent
  │   ├── ToolExecComponent (read/write/edit/bash/grep/find/ls)
  │   ├── RcRefCellComponent → AssistantMessageComponent
  │   ├── InfoMessageComponent
  │   └── Spacer(1) between each
  ├── pending_section (DynamicLines — streaming text/thinking)
  ├── status_section (DynamicLines — transient status)
  ├── queued_section (DynamicLines — ◷ queued messages)
  ├── working_section (DynamicLines — ⠋ Working...)
  ├── EditorComponent (border color: thinking level / bash mode)
  └── FooterComponent (cwd, git branch, token usage, model, auto-compact)
```

### Message queuing and follow-up

When the user submits a message while streaming, it is queued via yoagent's
native `follow_up()` method. The agent loop's outer loop picks it up when the
current turn finishes — no concurrent agent loops. Queued messages appear
between chat and editor. On `AgentEnd`, the next queued message is
auto-submitted.

- **Follow-up** (`app.message.followUp`, default Alt+Enter) — queues a message
  during streaming without aborting the current turn.
- **Dequeue** (`app.message.dequeue`, default Alt+Up) — restores the queued
  message back to the editor.
- **Interrupt** (`app.interrupt`, default Ctrl+C) — aborts streaming and
  restores any queued messages to the editor.

### Keybinding actions

Keybinding actions are defined as constants in `src/tui/keybindings.rs`:

| Action constant | ID string | Default binding |
|-----------------|-----------|----------------|
| `ACTION_APP_ESCAPE` | `app.escape` | Escape |
| `ACTION_APP_CLEAR` | `app.clear` | Ctrl+C |
| `ACTION_APP_INTERRUPT` | `app.interrupt` | Ctrl+C (when streaming) |
| `ACTION_APP_EXIT` | `app.exit` | Ctrl+D |
| `ACTION_APP_SUSPEND` | `app.suspend` | Ctrl+Z |
| `ACTION_APP_THINKING_CYCLE` | `app.thinking.cycle` | Shift+Tab |
| `ACTION_APP_MODEL_SELECTOR` | `app.model.select` | Ctrl+L |
| `ACTION_APP_MODEL_CYCLE_FORWARD` | `app.model.cycleForward` | Ctrl+P |
| `ACTION_APP_MODEL_CYCLE_BACKWARD` | `app.model.cycleBackward` | Shift+Ctrl+P |
| `ACTION_APP_TOGGLE_THINKING` | `app.thinking.toggle` | Ctrl+T |
| `ACTION_APP_TOOLS_EXPAND` | `app.tools.expand` | Ctrl+O |
| `ACTION_APP_EDITOR_EXTERNAL` | `app.editor.external` | Ctrl+E |
| `ACTION_APP_HELP` | `app.help` | Ctrl+H |
| `ACTION_APP_HISTORY_UP` | `app.historyUp` | Ctrl+Up |
| `ACTION_APP_HISTORY_DOWN` | `app.historyDown` | Ctrl+Down |
| `ACTION_APP_MESSAGE_FOLLOW_UP` | `app.message.followUp` | Alt+Enter |
| `ACTION_APP_MESSAGE_DEQUEUE` | `app.message.dequeue` | Alt+Up |
| `ACTION_APP_COMPACT_TOGGLE` | `app.compact.toggle` | Shift+Ctrl+C |
| `ACTION_APP_SESSION_NEW` | `app.session.new` | Ctrl+N |
| `ACTION_APP_SESSION_TREE` | `app.session.tree` | Ctrl+G |
| `ACTION_APP_SESSION_FORK` | `app.session.fork` | Ctrl+F |
| `ACTION_APP_SESSION_RESUME` | `app.session.resume` | Ctrl+R |
| `ACTION_INPUT_SUBMIT` | `tui.input.submit` | Enter |
| `ACTION_INPUT_TAB` | `tui.input.tab` | Tab |
| `ACTION_INPUT_NEW_LINE` | `tui.input.newLine` | Alt+Enter (when idle) |
| `ACTION_INPUT_COPY` | `tui.input.copy` | Ctrl+Shift+C |

All bindings are customizable via `~/.rab/keybindings.json`.

### Transient status

Toggle messages (Ctrl+T thinking, Ctrl+O tool output, model switch, interrupt)
use `status_section` that appears for one frame then clears.

### Bang commands

- `!command` — run command, persist output to session as tool result
- `!!command` — run command, don't persist output, useful for viewing help/man

---

## Storage layout (`~/.rab/`)

```
~/.rab/
├── agent/
│   ├── settings.json          # global settings
│   ├── auth.json              # API keys and OAuth credentials
│   ├── models.json            # user provider/model overrides (merged with built-in)
│   ├── SYSTEM.md              # custom system prompt (full override)
│   ├── APPEND_SYSTEM.md       # appended to system prompt
│   ├── AGENTS.md              # global context file
│   ├── mcp.json               # MCP server configuration
│   └── prompts/               # prompt templates (.md files)
├── models.json                # ⬜ deprecated, use agent/models.json
├── keybindings.json           # custom keybindings
├── extensions/                # ⬜ WASM plugins (Phase 2)
├── skills/                    # agent skills (SKILL.md files)
├── themes/                    # TUI themes
└── sessions/
    └── <cwd-hash>/            # one directory per project
        ├── 01J...abc.jsonl
        └── 01J...def.jsonl

./
├── .rab/
│   ├── settings.json          # project-local overrides
│   ├── SYSTEM.md              # project-local system prompt
│   └── APPEND_SYSTEM.md       # project-local append prompt
├── AGENTS.md                  # project context (also walks parent dirs)
└── CLAUDE.md                  # alias for AGENTS.md
```

---

## Dependency tree

```
rab (EPL-2.0)
├── yoagent 0.8.4         (MIT)        - agent loop, provider, types, skills
│   └── reqwest           (Apache 2.0) - HTTP client (inside yoagent)
├── tokio 1               (MIT)        - async runtime
├── tokio-util 0.7        (MIT)        - CancellationToken
├── serde + serde_json 1  (MIT)        - JSON serialization
├── uuid 1                (MIT)        - message/session IDs
├── chrono 0.4            (MIT)        - timestamps
├── directories 6         (MIT)        - XDG paths
├── anyhow 1              (MIT)        - error handling
├── futures 0.3           (MIT)        - StreamExt
├── async-trait 0.1       (MIT)        - trait async fn
├── colored 3             (MPL-2.0)    - terminal colors
├── crossterm 0.29        (MIT)        - terminal I/O
├── unicode-segmentation 1 (MIT)       - grapheme-aware cursor movement
├── unicode-width 0.2     (MIT)        - character display width
├── unicode-normalization 0.1 (MIT)    - Unicode normalization
├── comrak 0.52           (MIT)        - markdown parsing (GFM)
├── syntect 5.3           (MIT)        - syntax highlighting (optional feature)
├── base64 0.22           (MIT)        - image data URL encoding
├── async-stream 0.3      (MIT)        - async stream generation
├── regex 1.12            (MIT)        - regex
├── reqwest 0.12          (MIT/Apache 2.0) - HTTP client (rustls-tls, socks)
├── reqwest-eventsource 0.6 (Apache 2.0) - SSE streaming for OpenAI API
├── tracing 0.1           (MIT)        - diagnostic logging
├── openssl-sys 0.9       (MIT)        - vendored OpenSSL
├── libc 0.2              (MIT)        - system calls
├── url 2.5               (MIT)        - URL parsing
├── fs2 0.4               (MIT)        - cross-platform file locks
# wasmtime 26+ (Phase 2, Apache 2.0)
# notify 7    (Phase 2, CC0-1.0)
# rmcp 1      (Phase 2, MIT)
```

No GPL dependencies. All permissive (MIT / Apache 2.0 / MPL-2.0), fully
compatible with EPL-2.0. Phase 2 dependencies (wasmtime, notify, rmcp) are
gated behind Cargo features: `plugins` and `mcp`. MVP compiles without them.

---

## Phase 2

### WASM plugin system — ⬜ NOT STARTED

Planned: WASM components via wasmtime, loaded from `~/.rab/extensions/`.
Same `Extension` trait used by builtins — plugins implement it via WIT
bindings. Hot reload via file watcher.

### MCP adapter — ✅ IMPLEMENTED

The `src/extensions/mcp/` module (6 modules, ~2040 lines) provides a full MCP
adapter matching pi-mcp-adapter's architecture. Configured via `~/.rab/agent/mcp.json`
(global) and `.rab/mcp.json` (project-local overrides).

#### Key components:

| Module | Description |
|--------|-------------|
| `mcp/mod.rs` | `McpExtension` struct, proxy `mcp` tool, `McpDirectTool` adapter |
| `mcp/config.rs` | Config loading + merging (global + project), server config hashing |
| `mcp/server.rs` | `ServerManager` — lazy connection, idle timeout, keep-alive, SSE-aware HTTP transport |
| `mcp/types.rs` | `McpConfig`, `ServerEntry`, `McpSettings`, `MetadataCache`, tool name formatting |
| `mcp/cache.rs` | Persistent metadata cache (`mcp-cache.json`) for fast startup |
| `mcp/renderer.rs` | `McpToolRenderer` and `McpProxyToolRenderer` for TUI rendering |

#### Dual architecture:
- **Proxy tool** (`mcp` gateway) — status, list, search, describe, connect, call, auth actions
- **Direct tools** — servers with `directTools` enabled register individual tools as native `AgentTool`s

#### Server lifecycle:
- `lazy` (default) — connects on first use, disconnects after idle timeout
- `eager` — connects at agent startup
- `keep-alive` — never disconnects

### models.json — ✅ IMPLEMENTED

Custom provider/model definitions are now supported via `~/.rab/agent/models.json`,
merged with the built-in catalog (`src/provider/models.json`, ~1300 lines).
User entries with the same provider ID replace built-in entries entirely.
The `rab update-models` subcommand fetches the latest models from `models.dev`
and updates the built-in catalog.

### User extensions (compile-time)

Already possible today — implement the `Extension` trait and register in
`main.rs`. No dynamic loading yet.

---

## Open questions

- **Image paste in TUI** — clipboard integration differs per platform
  (wl-paste, pbpaste, PowerShell). Kitty protocol covers display; input TBD.
- **Command deny-list** — bash tool currently runs anything. A deny-list or
  sandbox (bubblewrap, landlock) should be configurable.
- **Provider fallback** — if the primary provider fails, should rab retry
  with another? yoagent handles basic retry; full fallback chain TBD.
- **Multi-model cycling** — Ctrl+P model switching uses a static list.
  A full model registry with metadata (context window, costs) is future work.
