# rab Architecture

A lightweight, extensible Rust coding agent inspired by [pi-coding-agent](https://pi.dev).
rab delegates the core agent loop, types, and provider abstraction to the **yoagent** crate
(MIT, published crate 0.13.1), providing the session layer, TUI, built-in tools, slash commands,
file search tools (grep/find/ls), file mutation queue, lifecycle management, and a
**custom provider layer** with a model registry and rich OpenAI-compatible streaming support.

---

## Layered architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                          rab (EPL-2.0)                               │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │               main.rs (thin entry point)                     │   │
│  │  subcommand: rab generate-models                            │   │
│  │  delegates to rab::cli::run                                 │   │
│  └────────────────────┬─────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │              rab::cli (src/cli/)                              │   │
│  │                                                              │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           args.rs                                     │   │   │
│  │  │  CliArgs struct, parse_args, get_agent_dir,          │   │   │
│  │  │  load_system_md/load_append_system_md                │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           run.rs                                     │   │   │
│  │  │  Startup orchestration: validate_flag_conflicts,     │   │   │
│  │  │  build_session, build_builtin_extension,             │   │   │
│  │  │  build_extensions, build_tools_and_snippets,         │   │   │
│  │  │  register_hooks, load_skills, load_prompt_templates, │   │   │
│  │  │  run_interactive, run_print                          │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           print_mode.rs                              │   │   │
│  │  │  Agent loop for non-interactive mode (stdin/stdout) │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           session.rs                                 │   │   │
│  │  │  Session resolution helper (resolve_session_arg)    │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │                                                              │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │              AgentSession (agent_session.rs)                  │   │
│  │  Primary entry point; owns Session directly (not via         │   │
│  │  SessionManager). Factory methods: create, open, etc.       │   │
│  │  - Event-driven message persistence                          │   │
│  │  - Model/thinking/tool change detection & recording          │   │
│  │  - Auto/manual compaction (compaction.rs)                    │   │
│  │  - Branch summarization (branch_summary.rs)                  │   │
│  │  - Branch navigation (set_branch)                            │   │
│  │  - Pi-compatible persist_message_end pattern                 │   │
│  │  - Compaction cancellation (Cancel token)                    │   │
│  │  - Provider registry for per-message cost config             │   │
│  └──────────┬───────────────────────────────────────────────────┘   │
│             │                                                       │
│  ┌──────────▼───────────────────────────────────────────────────┐   │
│  │              Session (agent/session.rs)                       │   │
│  │  Simplified: wraps yoagent::Session directly                 │   │
│  │  - Costs stored as ExtensionMessage entries in JSONL stream  │   │
│  │  - Metadata as ExtensionMessage (model_change, compaction,   │   │
│  │    thinking_level_change, etc.)                              │   │
│  │  - No SessionStorage trait, no InMemory/Jsonl distinction    │   │
│  │  - No SessionManager, no lazy write                          │   │
│  │  - ~1424 lines                                               │   │
│  │                                                              │   │
│  │  Format (yoagent JSONL):                                     │   │
│  │    Line 1: metadata JSON (id, cwd, createdAt, name, ...)    │   │
│  │    Lines 2+: yoagent JSONL entries (append-friendly)        │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐      │
│  │builtin│ │  tui/ │ │commands│ │extens-│ │settings│ │ auth  │      │
│  │read   │ │ agent/ │ │.rs     │ │ions/  │ │.rs     │ │.rs    │      │
│  │write  │ │ ui/    │ │22 slash│ ┌──────────────┐ │~/.rab/ │ │API    │
│  │edit   │ │screen  │ │commands│ │file_search (3)│ │settings│ │keys,  │
│  │bash   │ │editor  │ │+ /ext. │ │mcp/ (6 mods)  │ │AGENTS  │ │OAuth  │
│  |file_  | │list    │ │/nextTurn│ │tree_sitter/   │ │.md     │ │       │
│  │mutation│ └───────┘ │        │ │AGENTS.md       │ │skills  │ │       │
│  │_queue │            │        │ │skills          │ │        │ │       │
│  │cancel │            │        │ │prompts/        │ │        │ │       │
│  └──┬────┘            │        │ │prompt_templ.rs │ │        │ │       │
│     │                 │        │ └──────────────┘ └───────┘ └───────┘      │
│     │                 │        │                                     │
│     │     impl Extension trait + yoagent::types::AgentTool          │
│     │                                                               │
│  ┌──▼──────────────────────────────────────────────────────────┐   │
│  │              src/extension/ (Extension trait + types)         │   │
│  │  Split from the old agent/extension.rs into 5 modules:       │   │
│  │                                                              │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           traits.rs — pub trait Extension              │   │   │
│  │  │  ExtensionDefault (Builtin/Enabled/Disabled)          │   │   │
│  │  │  is_extension_enabled()                               │   │   │
│  │  │  ToolRenderer trait (pi-compatible render_call/       │   │   │
│  │  │    render_result/render_self)                         │   │   │
│  │  │  Lifecycle: on_reload, on_session_shutdown,           │   │   │
│  │  │    on_session_start                                   │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           types.rs — ToolDefinition, SlashCommand,    │   │   │
│  │  │  CommandResult, HookRegistration, BeforeHook/AfterHook│   │   │
│  │  │  AfterToolCallResult, BeforeToolCallResult, Cancel    │   │   │
│  │  │  ToolRenderContext, AutocompleteItem                  │   │   │
│  │  │  ToolDefinition::execute() with coercion + validation │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           hooks.rs — Global extension hook registry   │   │   │
│  │  │  register_tool_hooks(), clear_tool_hooks(),           │   │   │
│  │  │  run_before_hooks(), run_after_hooks()                │   │   │
│  │  │  Global RwLock<HashMap<tool_name, ToolHookSet>>       │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           coerce.rs — coerce_with_json_schema(),      │   │   │
│  │  │  validate_tool_arguments(), ValidationError           │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │                                                              │   │
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
│  │  │  list_models(), list_providers(), count_providers     │   │   │
│  │  │  list_authenticated_model_ids()                       │   │   │
│  │  │  provider_has_auth(), auth_status_for_provider()      │   │   │
│  │  │  list_model_provider_tuples()                         │   │   │
│  │  │  ResolvedModel carries: ModelConfig, api_key,         │   │   │
│  │  │    rab_compat (RabOpenAiCompat), thinking_map         │   │   │
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
│  │  │  Rich compat flags parsed from model config:         │   │   │
│  │  │  supports_store, supports_developer_role,            │   │   │
│  │  │  supports_reasoning_effort, supports_thinking_control│   │   │
│  │  │  supports_usage_in_streaming, max_tokens_field,      │   │   │
│  │  │  requires_tool_result_name,                          │   │   │
│  │  │  requires_assistant_after_tool_result,               │   │   │
│  │  │  requires_reasoning_content_on_assistant_messages,   │   │   │
│  │  │  thinking_format (OpenAi/OpenRouter/DeepSeek/...)    │   │   │
│  │  │  Removed: requires_thinking_as_text,                 │   │   │
│  │  │    supports_strict_mode, supports_long_cache_retention│   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           RabAnthropicProvider (anthropic.rs)         │   │   │
│  │  │  Thin wrapper around yoagent's AnthropicProvider.    │   │   │
│  │  │  Fixes the hardcoded "anthropic" provider name in    │   │   │
│  │  │  assistant messages to the per-model provider from   │   │   │
│  │  │  ModelConfig (for correct cost tracking/display).    │   │   │
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
│  │  │       generate_models.rs (rab generate-models)        │   │   │
│  │  │  Fetches https://models.dev/api.json                │   │   │
│  │  │  Targets: github-copilot, opencode, opencode-go,    │   │   │
│  │  │           deepseek                                  │   │   │
│  │  │  Applies pi-style corrections (DeepSeek, Qwen,      │   │   │
│  │  │   Grok, Kimi, Anthropic, GitHub Copilot)            │   │   │
│  │  │  Preserves user edits to non-target providers       │   │   │
│  │  │  Writes src/provider/models.json                    │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           auth.rs (src/provider/auth.rs)             │   │   │
│  │  │  Moved from src/auth.rs to provider module.          │   │   │
│  │  │  Pi-compatible credential store, AuthStorageBackend  │   │   │
│  │  │  pattern (File/InMemory), file locking, OAuth refresh│   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │                                                              │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │                   yoagent 0.13.1 (MIT)                        │   │
│  │                                                              │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::types                             │   │   │
│  │  │  AgentMessage, Message (User/Assistant/ToolResult),   │   │   │
│  │  │  Content, AgentTool, AgentEvent, Usage, StreamDelta   │   │   │
│  │  │  ExtensionMessage, ToolResult                        │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::provider                          │   │   │
│  │  │  Provider trait + StreamProvider trait               │   │   │
│  │  │  OpenAiCompatProvider, AnthropicProvider,             │   │   │
│  │  │  OpenAiResponsesProvider, GoogleProvider             │   │   │
│  │  │  ModelConfig, CostConfig, ApiProtocol                │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::agent                             │   │   │
│  │  │  Agent struct, run_agent_loop(),                     │   │   │
│  │  │  text/tool streaming, event emission                 │   │   │
│  │  │  follow_up(), steer()                                │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::session                           │   │   │
│  │  │  Session struct, SessionEntry, session tree model    │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::skills                            │   │   │
│  │  │  Skill type, frontmatter parsing, SkillSet           │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────────────────────────────────────────┐   │   │
│  │  │           yoagent::mcp                              │   │   │
│  │  │  MCP types: McpToolInfo, McpContent                 │   │   │
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
│  │  GitHub Copilot (api.individual.githubcopilot.com)           │   │
│  │  DeepSeek (api.deepseek.com)                                 │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  TUI (src/tui/ + src/agent/ui/) — 59 source modules,         │   │
│  │  ~28.5K lines, ~688 tests                                    │   │
│  │  Direct Rust port on crossterm 0.29                          │   │
│  │  Image (Kitty protocol), TerminalColors (OSC 11 detection),  │   │
│  │  TreeSelector, ConfirmOverlay, LoginDialog, OAuthSelector,   │   │
│  │  ScopedModelsSelector, ForkSelector, SettingsList,           │   │
│  │  SettingsSelector                                            │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  Utility (src/util/) — paths.rs, tls.rs, mod.rs              │   │
│  │  Centralized path handling (canonicalize, resolve, display)  │   │
│  │  TLS platform verification patched for Android/Termux        │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

## Key architectural decisions

- **yoagent is the core dependency**, not genai. rab delegates the agent loop,
  provider abstraction, message types, and session tree model to yoagent 0.13.1
  (published crate). rab provides the session layer, TUI, built-in tools,
  file search tools, slash commands, lifecycle management, and a custom provider
  layer on top.

- **Custom provider layer over yoagent** — rab has its own `ProviderRegistry` that
  loads a built-in model catalog (`src/provider/models.json`, ~17910 lines)
  merged with user overrides (`~/.rab/agent/models.json`). On top of yoagent's
  providers, rab also provides:
  - `RabOpenAiCompatProvider` — custom streaming provider that handles DeepSeek
    thinking format, `reasoning_content`, configurable `max_tokens_field`, and all
    pi `OpenAICompletionsCompat` flags stored in model config.
  - `RabAnthropicProvider` — thin wrapper correcting the provider name in assistant
    messages (yoagent hardcodes "anthropic", this uses the per-model provider name).

- **`rab generate-models` subcommand** — fetches `https://models.dev/api.json`,
  applies pi-style corrections (DeepSeek, Qwen, Grok, Kimi, Anthropic Messages API
  compat flags for GitHub Copilot Claude models), and writes
  `src/provider/models.json`. All-or-nothing: any error aborts before writing.
  Preserves user edits to non-target providers (currently targets: github-copilot,
  opencode, opencode-go, deepseek).

- **Simplified session layer** — rab wraps `yoagent::Session` directly instead of
  the old three-layer architecture (Session → SessionManager → SessionStorage).
  Metadata entries (model changes, compaction, thinking level changes) and costs
  are stored as `ExtensionMessage` entries in the JSONL stream. No traits, no
  separate storage implementations, no lazy write mechanism — persistence is
  straightforward file I/O through yoagent.

- **Multi-protocol agent selection** — `main.rs` resolves the model via
  `ProviderRegistry`, then selects the appropriate yoagent provider based on
  `ApiProtocol`:
  - `OpenAiCompletions` → `Agent::from_provider(RabOpenAiCompatProvider, mc)`
  - `AnthropicMessages` → `Agent::from_provider(RabAnthropicProvider, mc)`
  - `OpenAiResponses` → `Agent::from_config(mc)`
  - `GoogleGenerativeAi` → `yoagent::provider::GoogleProvider`
  - Fallback → `yoagent::provider::OpenAiCompatProvider`

- **One extension mechanism** — built-in tools and user extensions use the same
  `Extension` trait. No separate tool registration path. All tools, commands,
  renderers, and skills go through `Extension`. Extensions have a `default_state()`
  (Builtin/Enabled/Disabled) and can be toggled via `/extensions` at runtime.

- **ToolDefinition wraps every tool** — each `AgentTool` is wrapped in a
  `ToolDefinition` that carries prompt snippet metadata, guidelines, argument
  preparation hooks (`prepare_arguments`), `before_tool_call` and
  `after_tool_call` hooks (pi-compatible), and automatic JSON Schema argument
  coercion + validation.

- **Global hook system** — `src/extension/hooks.rs` provides a global registry
  for before/after tool hooks. Extensions register hooks at startup via
  `Extension::tool_hooks()`, and they are invoked by `run_before_hooks()` /
  `run_after_hooks()` during tool execution.

- **Pluggable operations** — every built-in tool (read, write, edit, bash,
  grep, find, ls) delegates filesystem/shell operations through a trait
  (e.g. `ReadOperations`, `BashOperations`, `GrepOperations`, `FindOperations`,
  `LsOperations`), making it possible to replace local execution with remote (SSH) execution.

- **OAuth support** — `src/provider/oauth/` implements pi's `OAuthProviderInterface`
  with device code flow (RFC 8628) for headless authentication. The GitHub Copilot
  OAuth provider fetches available models after login and auto-enables them. OAuth
  tokens are refreshed on each agent turn.

- **Agent loop lives in yoagent** — rab has no `loop.rs`. yoagent's `Agent`
  struct handles streaming, tool execution, and event emission. rab subscribes
  to events via `AgentEvent` for persistence and UI updates. A fresh `Agent`
  is created per turn (new agent loop per user message), using yoagent's
  native `follow_up()` for mid-turn message queuing, `steer()` for
  turn-level message injection.

- **Types from yoagent** — `AgentMessage`, `Message`, `Content`, `AgentTool`,
  `AgentEvent`, `StreamDelta`, `ExtensionMessage`, `ToolResult` are all
  re-exported from `yoagent::types`. rab's `types.rs` is a thin shim with
  helper functions only (no rab-specific enums).

- **File mutation queue** — concurrent file writes/edits to the same file are
  serialized via `with_file_mutation_queue()` so the model can issue multiple
  sequential edits to the same file without races.

- **Steering / follow-up** — rab implements pi's `steering_mode` and
  `follow_up_mode` settings (configurable per-session), and the `/nextTurn`
  slash command for queueing messages during a turn.

---

## Pi component mapping

| pi component | rab equivalent | Status |
|---|---|---|
| `pi-tui` (terminal UI, components, editor) | `src/tui/` + `src/agent/ui/` | ✅ Complete — 59 modules, ~28.5K lines, ~688 tests. Direct Rust port on crossterm 0.29. Includes Image (Kitty), TerminalColors (OSC 11), TreeSelector, ConfirmOverlay, LoginDialog, OAuthSelector, ScopedModelsSelector, ForkSelector, SettingsList, SettingsSelector. |
| `pi-agent-core` (agent loop, session, compaction, skills) | Delegated to **yoagent** (agent loop, types, provider, session tree, skills) + rab's `AgentSession` (session lifecycle, compaction, branching) | ✅ Agent loop in yoagent (`yoagent::agent::Agent`). ✅ Simplified Session in `agent/session.rs` (~1424 lines), wraps `yoagent::Session`. ✅ Compaction in `compaction.rs` (~946 lines). ✅ Branch summarization (~455 lines). ✅ Skills loaded via `yoagent::skills::SkillSet`. |
| `coding-agent` (CLI, extensions, tools, settings, commands) | `main.rs`, `cli/`, `builtin/`, `extensions/`, `settings.rs`, `provider/auth.rs`, `src/extension/` | ✅ CLI args in `cli/args.rs`, startup in `cli/run.rs`, print mode in `cli/print_mode.rs`. ✅ Tools (read/write/edit/bash/grep/find/ls), settings, auth, CLI done. ✅ 22 slash commands including `/export`, `/import`, `/settings`, `/extensions`, `/nextTurn`, `/stop`. ✅ Prompt templates as `/name` commands. ✅ Extension trait with tools, commands, renderers, skills, hooks. |
| `GrepTool`, `FindTool`, `LsTool` (pi agent tools) | `src/extensions/file_search.rs` | ✅ grep (ripgrep/grep fallback), find (fd/find fallback), ls — all with pluggable operations. |
| provider registry + model catalog | `src/provider/` | ✅ `ProviderRegistry` loading built-in + user models.json. ✅ `RabOpenAiCompatProvider` for rich OpenAI-compatible streaming. ✅ `RabAnthropicProvider` (thin wrapper). ✅ OAuth support (device code flow, GitHub Copilot). ✅ `rab generate-models` subcommand. |
| MCP adapter (pi-mcp-adapter) | `src/extensions/mcp/` (6 modules) | ✅ Proxy `mcp` tool, direct tool adapters, config loading (global+project merge), server lifecycle (lazy connect, idle timeout), persistent metadata cache, tool renderers. |
| provider | `yoagent::provider::*` + `rab::provider::RabOpenAiCompatProvider` + `rab::provider::RabAnthropicProvider` | ✅ Multi-protocol: RabOpenAiCompatProvider (OpenAiCompletions), RabAnthropicProvider (AnthropicMessages), OpenAiResponsesProvider, GoogleProvider. Auto-detection by model config's `ApiProtocol`. |
| `beforeToolCall` / `afterToolCall` | `ToolDefinition.before_tool_call` / `.after_tool_call` + global hook system | ✅ Per-tool hooks + global `Extension::tool_hooks()` registration for blocking/preprocessing/postprocessing |
| `validateToolArguments` | `extension::coerce::validate_tool_arguments()` | ✅ Full JSON Schema validation with pi-compatible error paths |
| Argument coercion | `extension::coerce::coerce_with_json_schema()` | ✅ Type coercion matching pi's `Value.Convert` + `coerceWithJsonSchema` |
| Theme system | `src/agent/ui/theme.rs` | ✅ JSON theme system with resolution, fallback, detection (~794 lines) |
| Resource loading (AGENTS.md/CLAUDE.md) | `src/agent/context_files.rs` | ✅ AGENTS.md/CLAUDE.md discovery, `<project_context>` wrapping |
| Skills | `yoagent::skills::SkillSet` + `SystemPromptBuilder.skills()` | ✅ Skill loading, frontmatter, prompt formatting, /skill:name expansion |
| Image support (Kitty protocol) | `src/tui/components/image.rs` + markdown.rs hyperlinks | ✅ Image display via Kitty protocol with dedicated Image component. Input (clipboard paste) TBD. |
| Config files | `~/.rab/` | ✅ Same schema as pi. Auth at `~/.rab/agent/auth.json`. |
| Footer data (git branch, extensions) | `src/agent/footer_data_provider.rs` | ✅ Git branch resolution (worktree/reftable support), extension statuses, provider count. |
| File mutation queue | `src/builtin/file_mutation_queue.rs` | ✅ Per-file serialization using tokio::sync::Notify, same pattern as pi |
| MCP extension | `src/extensions/mcp/` (6 mods) | ✅ Proxy `mcp` tool, direct tools, config loading, server lifecycle, cache, renderers |
| Export/Import | `src/builtin/export.rs` | ✅ `/export` (HTML/JSONL), `/import` with embedded template assets |
| Prompt templates | `src/agent/prompt_templates.rs` | ✅ `/name` commands from `.md` files, frontmatter, placeholder expansion |
| Path utilities | `src/util/paths.rs` | ✅ Canonicalization, resolution, display (cross-platform) |
| OAuth | `src/provider/oauth/` | ✅ Device code flow (RFC 8628), GitHub Copilot provider, credential storage. Token refresh on each turn. |
| Interactive fork /settings menu | `fork_selector.rs`, `settings_list.rs`, `settings_selector.rs` | ✅ `/fork` with interactive message selector overlay. `/settings` with SettingsSelector overlay listing all configurable fields. |
| Settings deep merge | `settings.rs` — `DeepMerge` trait, nested config structs | ✅ Full pi-compatible settings with nested blocks. `ExtensionsConfig` for extension enable/disable. |
| Auth backend pattern | `src/provider/auth.rs` — `AuthStorageBackend` enum (File/InMemory) | ✅ Lock-based read-modify-write semantics, pi-compatible `AuthStorageBackend` pattern. |
| Extension enable/disable | `/extensions` command + `ExtensionsConfig` | ✅ Per-extension toggle, persists to settings.json. `ExtensionDefault::Builtin/Enabled/Disabled`. |
| Steering / follow-up | `steering_mode`, `follow_up_mode` settings, `/nextTurn` command | ✅ Pi-compatible turn-steering with one-at-a-time, all, manual modes. |
| Tree-sitter | `src/extensions/tree_sitter/` | ✅ Skeleton implementation for AST-aware tools. |
| WASM plugin system | Not started | ⬜ Phase 2 |

---

## CLI (`src/cli/`) — extracted from main.rs

The CLI module was factored out of `main.rs` for testability:

### args.rs — `CliArgs` struct + `parse_args()`

```
rab [MESSAGE]...

Subcommands:
  rab generate-models         Fetch and update built-in model catalog

Session:
  -c, --continue             Continue most recent session in cwd
  -r, --resume               Open interactive session picker
  --session PATH             Open specific session file (path or partial ID)
  --session-id ID            Create/open session with explicit ID
  --fork PATH                Fork a session from another session file
  --export PATH              Export session and exit
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

### run.rs — Startup orchestration

`validate_flag_conflicts()` prevents contradictory flags (e.g. `--fork` + `--continue`).

`build_session()` creates/opens/forks/continues a session based on CLI flags.

`build_builtin_extension()` constructs a single `BuiltinExtension` with all built-in
tools and slash commands.

`build_extensions()` loads file search, MCP, tree-sitter extensions.

`build_tools_and_snippets()` collects tools, snippets, and guidelines from enabled extensions.

`register_hooks()` calls `register_tool_hooks()` for each enabled extension.

`load_skills()` loads skills from skill dirs + extensions.

`load_prompt_templates()` loads prompt templates from prompt dirs.

Extension gating: `is_extension_enabled()` checks `settings.extensions_config.states`
(managed by `/extensions` command). Core builtins are always enabled.

### print_mode.rs — Non-interactive agent loop

Streams response to stdout. Thinking blocks shown dimmed on stderr.
Tool calls and results shown prefixed with colored indicators. 120s timeout.

### session.rs — Session resolution

`resolve_session_arg()` resolves session paths and partial UUID prefixes.

---

## Core type system (`src/agent/types.rs` + `src/extension/types.rs`)

### yoagent types (re-exported from `agent/types.rs`)

```rust
pub use yoagent::types::{AgentMessage, Content, Message, AgentEvent, StreamDelta,
                          ExtensionMessage, ToolResult};
```

### Helper functions (`agent/types.rs`)

```rust
pub fn content_text(content: &[Content]) -> String;
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
pub fn message_dedup_key(msg: &AgentMessage) -> String;
pub fn message_is_system_stop(msg: &AgentMessage) -> bool;
pub fn message_is_extension(msg: &AgentMessage) -> bool;
pub fn message_extension_kind(msg: &AgentMessage) -> Option<&str>;
pub fn message_extension_text(msg: &AgentMessage) -> Option<String>;
pub fn user_message(text: &str) -> AgentMessage;
pub fn assistant_message(text: &str) -> AgentMessage;
pub fn tool_result_message(tool_call_id: &str, tool_name: &str, text: String, is_error: bool) -> AgentMessage;
pub fn extension_message(kind: &str, text: &str, display: bool) -> AgentMessage;
pub fn extension_message_with_details(kind: &str, text: &str, display: bool, details: Value) -> AgentMessage;
pub fn base_model_config(config: &ModelConfig) -> BaseModelConfig;
```

### Extension types (`extension/types.rs`)

```rust
pub struct ToolDefinition {
    pub tool: Box<dyn AgentTool>,
    pub snippet: &'static str,
    pub guidelines: &'static [&'static str],
    pub prepare_arguments: Option<fn(Value) -> Result<Value, String>>,
    pub before_tool_call: Option<fn(&Value) -> Option<BeforeToolCallResult>>,
    pub after_tool_call: Option<fn(&ToolResult, bool) -> Option<AfterToolCallResult>>,
    pub renderer: Option<Arc<dyn ToolRenderer>>,
}

pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub handler: Box<dyn CommandHandler>,
}

pub enum CommandResult {
    Info(String), Quit, ModelChanged(String), ShowHelp, Reloaded,
    NewSession, SessionSwitched, SessionInfo, OpenSessionSelector,
    SessionNamed, OpenSettings, ScopedModels, ExportSession,
    ImportSession, CopyLastMessage, ShowChangelog, ForkSession,
    CloneSession, SessionTree, TrustDecision, Login, Logout,
    CompactSession, Stop, NextTurn, ExtensionsToggle,
    ...
}

pub struct BeforeToolCallResult { pub block: bool, pub reason: String }
pub struct AfterToolCallResult { ... }
pub struct HookRegistration { pub tool_name: &'static str, pub before: Option<BeforeHook>, pub after: Option<AfterHook> }
pub struct ToolRenderContext { ... }
pub struct AutocompleteItem { pub value: String, pub label: String, ... }
pub struct Cancel { ... }  // shared cancellation flag
```

---

## Agent lifecycle (`src/agent/agent_session.rs`) — ~839 lines

The `AgentSession` struct is the primary entry point for session management.
It owns a `Session` directly (not `SessionManager`) plus app-level config.

```rust
pub struct AgentSession {
    inner: Session,
    session_dir: Option<PathBuf>,
    last_model: Option<(String, String)>,
    last_thinking_level: String,
    last_active_tools: Option<Vec<String>>,
    compaction_settings: CompactionSettings,
    context_window: u64,
    model_name: String,
    compaction_api_key: Option<String>,
    model_config: Option<ModelConfig>,
    rab_compat: Option<RabOpenAiCompat>,
    thinking_level: ThinkingLevel,
    event_listeners: Vec<CompactionEventCallback>,
    overflow_recovery_attempted: bool,
    compaction_cancel: Cancel,
    registry: Option<Arc<ProviderRegistry>>,
}
```

### Responsibilities

1. **Factory methods** — `AgentSession::new()`, `.create()`, `.open()`,
   `.in_memory()`, `.continue_recent()`, `.fork_from()`.
2. **Event-driven persistence** — `on_agent_event()` persists messages and
   metadata entries from `AgentEvent` stream.
3. **Model/thinking/tool change tracking** — `on_model_change()`,
   `on_thinking_level_change()`, `on_active_tools_change()` append metadata
   entries only when values differ from last known (diff-based).
4. **Auto-compaction** — `check_auto_compact()` runs after agent finishes a turn.
5. **Manual compaction** — `run_manual_compact()` for `/compact`.
6. **Branch summarization** — `summarize_branch_navigation()` summarises abandoned
   branches. `set_branch()` moves the leaf pointer.
7. **Overflow recovery** — `reset_overflow_recovery()` / `overflow_recovery_attempted`.

### Typical usage in print mode

```rust
let mut agent_session = AgentSession::create(&cwd, session_dir.as_deref());
agent_session.set_compaction_config(api_key, &model, context_window, Some(model_config));

let msg = user_message("list .rs files");
agent_session.send_user_message_obj(&msg);

let agent = yoagent::agent::Agent::from_provider(RabOpenAiCompatProvider, mc)
    .with_api_key(&api_key)
    .with_system_prompt(&system_prompt)
    .with_thinking(thinking_level)
    .with_messages(messages)
    .with_tools(tools)
    .without_context_management();

agent.prompt_with_sender(msg_text, tx).await;

while let Some(event) = rx.recv().await {
    agent_session.on_agent_event(&event);
}

agent_session.check_auto_compact().await;
```

---

## Session layer (`src/agent/session.rs`) — ~1424 lines

Simplified compared to the old architecture. Wraps `yoagent::Session` directly.

### Session (high-level wrapper)

```rust
pub struct Session {
    inner: YoagentSession,
    meta: SessionMeta,
    file_path: Option<PathBuf>,
}
```

Key methods: `append_message()`, `append_message_with_cost()`, `entry_cost()`,
`append_model_change()`, `append_thinking_level_change()`,
`append_active_tools_change()`, `append_compaction()`, `append_branch_summary()`,
`append_session_info()`, `append_label_change()`, `append_custom_message_entry()`,
`build_context()`, `get_leaf_id()`, `set_leaf_id()`, `get_entries()`,
`get_entry()`, `get_branch()`, `get_children()`, `find_entries()`,
`get_label()`, `session_id()`, `session_file()`, `session_name()`,
`metadata()`, `flush()`, `ensure_flushed()`, `is_persisted()`, `fork_from()`.

### Format

```
Line 1: metadata JSON (id, cwd, createdAt, optional name, parentSession)
Lines 2+: yoagent JSONL entries via yoagent::Session::append()

Costs stored as AgentMessage::Extension with kind = "session/cost"
Metadata entries stored as AgentMessage::Extension with well-known kinds:
  "session/model_change"
  "session/thinking_level_change"
  "session/active_tools_change"
  "session/compaction"
  "session/branch_summary"
  "session/label"
  "session/custom_message"
```

### Factory methods

```rust
pub fn new(cwd: &Path) -> Self;                    // in-memory
pub fn create(cwd: &Path, dir: &Path) -> Self;    // creates new persisted session
pub fn open(path: &Path, cwd_override) -> Self;   // opens existing session file
pub fn in_memory(cwd: &Path) -> Self;              // pure in-memory
pub fn continue_recent(cwd: &Path, dir: &Path) -> Self;  // latest or new
pub fn fork_from(source: &Self, ...) -> Self;      // forks session tree
```

---

## Session repo (`src/agent/session.rs` — functions, not trait)

Session listing/deletion/forking is handled by free functions in `session.rs`:

```rust
pub fn get_default_session_dir(cwd: &Path) -> PathBuf;
pub fn encode_cwd_for_dir(cwd: &Path) -> String;
pub fn list_sessions(session_dir: &Path, filter_cwd: Option<&Path>, ...) -> Vec<SessionInfo>;
pub fn load_session_info(path: &Path) -> Option<SessionInfo>;
pub fn delete_session(path: &Path) -> io::Result<()>;
pub fn fork_session(source_path: &Path, target_dir: &Path, entry_id, position) -> io::Result<String>;
```

No trait, no `DefaultSessionRepo` — just free functions for simplicity.

### SessionInfo

```rust
pub struct SessionInfo {
    pub id: String,
    pub path: PathBuf,
    pub cwd: String,
    pub name: Option<String>,
    pub created_at: String,
    pub message_count: usize,
    pub total_tokens: u64,
    pub total_cost: f64,
}
```

---

## Compaction (`src/agent/compaction.rs`) — ~946 lines ✅ IMPLEMENTED

When the conversation approaches the model's context window, older messages
are summarized to free space. Pi-style compaction using yoagent token estimation.

### Algorithm

1. **Check threshold** (`should_compact()`) — If total tokens exceed
   `context_window - reserve_tokens`, compaction triggers.
2. **Summarize** (`compact()`) — Send older messages to the provider with a
   structured summarization prompt. Supports incremental updates (previous
   summary in `<previous-summary>` tags).
3. **Replace** — Append a `CompactionEntry` to the session with summary,
   `first_kept_entry_id`, token count, and file operation details.

### Compaction cancellation

Uses `Cancel` token for abort (pi-compatible). When compaction is cancelled
mid-way, the session state remains consistent.

### Settings

```json
{
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000
  }
}
```

Defaults: enabled, 16K reserve, 20K keep recent.

---

## Branch summarization (`src/agent/branch_summary.rs`) — ~455 lines ✅ IMPLEMENTED

When the user navigates to a different branch in the session tree, the
abandoned branch is summarized so context is preserved. Same algorithm as
before (collect entries → prepare messages → call provider → append entry).

---

## Provider layer (`src/provider/`)

### ProviderRegistry (`mod.rs`)

The provider registry loads a built-in model catalog and overlays user overrides.
Each `ResolvedModel` carries `ModelConfig` + `api_key` + `RabOpenAiCompat` +
`thinking_map` (model-specific thinking level filtering).

```rust
pub struct ResolvedModel {
    pub model_config: ModelConfig,
    pub api_key: String,
    pub rab_compat: RabOpenAiCompat,
    pub thinking_map: Option<HashMap<String, serde_json::Value>>,
}
```

### RabOpenAiCompat (`compat.rs`) — simplified

Removed unused compat fields that were in the previous version:
- `requires_thinking_as_text` — removed
- `supports_strict_mode` — removed
- `supports_long_cache_retention` — removed

Compat data is carried alongside `ModelConfig` (not smuggled through `_rab_` headers).

### RabAnthropicProvider (`anthropic.rs`) — simplified

Now a thin wrapper around `yoagent::provider::AnthropicProvider`. Its only job is
correcting the hardcoded `"anthropic"` provider name in assistant messages to the
per-model provider name from `ModelConfig` (for correct cost tracking and display).

---

## Extension system (`src/extension/`)

The old `src/agent/extension.rs` (~1191 lines) was split into 5 modules:

### Extension trait (`traits.rs`)

```rust
pub enum ExtensionDefault { Builtin, Enabled, Disabled }

pub trait Extension: Send + Sync + Any {
    fn name(&self) -> Cow<'static, str>;
    fn as_any(&self) -> &dyn Any;
    fn default_state(&self) -> ExtensionDefault { Enabled }
    fn tools(&self) -> Vec<ToolDefinition> { vec![] }
    fn commands(&self) -> Vec<SlashCommand> { vec![] }
    fn skills(&self) -> SkillSet { SkillSet::empty() }
    fn tool_hooks(&self) -> Vec<HookRegistration> { vec![] }
    fn on_reload(&self) {}
    fn on_session_shutdown(&self, _reason: &str) {}
    fn on_session_start(&self, _reason: &str) {}
}
```

### ToolRenderer trait (`traits.rs`)

```rust
pub trait ToolRenderer: Send + Sync {
    fn render_call(&self, args: &Value, theme: &dyn Theme, ctx: &ToolRenderContext) -> Box<dyn Component>;
    fn render_result(&self, content: &str, theme: &dyn Theme, ctx: &ToolRenderContext) -> Option<Box<dyn Component>>;
    fn render_self(&self) -> bool { false }
}
```

### Extension enable/disable

- `/extensions` command lists all extensions with their enabled/disabled status.
- User toggles with Enter. Changes persist to `settings.json → extensionsConfig`.
- `is_extension_enabled()` checks `settings.extensions_config.states`, falling
  back to the extension's `default_state()`.
- Builtin extensions (`ExtensionDefault::Builtin`) are always enabled.

### Argument coercion and validation (`coerce.rs`)

Every tool call goes through `ToolDefinition::execute()`:

1. **`prepare_arguments`** — custom per-tool pre-processing
2. **`coerce_with_json_schema()`** — recursive type coercion for common LLM mistakes
3. **`validate_tool_arguments()`** — full JSON Schema validation with pi-compatible error paths
4. **`before_tool_call`** — optional hook that can block execution
5. **Execute** — call inner `AgentTool::execute()`
6. **`after_tool_call`** — optional hook that can modify the result

---

## Builtin extension (`src/builtin/extension.rs`)

Single `BuiltinExtension` consolidating all built-in tools and slash commands.

### BuiltinOptions

```rust
pub struct BuiltinOptions {
    pub cwd: PathBuf,
    pub read_operations: Arc<dyn ReadOperations>,
    pub write_operations: Arc<dyn WriteOperations>,
    pub edit_operations: Arc<dyn EditOperations>,
    pub bash_options: BashToolOptions,
}
```

### Built-in tools

| Tool | Key features |
|------|-------------|
| **read** | Path resolution, line numbers, 50KB truncation. Image support (base64 data URL). Pluggable `ReadOperations`. |
| **write** | Temp file + atomic rename, parent dir creation. Preview rendering with syntax highlighting. |
| **edit** | Exact-match search/replace, error on zero/multiple matches. Diff rendering with intra-line character-level inverse. Pluggable `EditOperations`. File mutation queue. |
| **bash** | `sh -c <command>`, configurable timeout, streaming via ToolProgress. Pluggable `BashOperations`, command prefix, custom shell. |

### File search extension (`src/extensions/file_search.rs`)

| Tool | Key features |
|------|-------------|
| **grep** | ripgrep (`rg`) with `--json`, fallback to `grep`. Respects .gitignore. Pluggable `GrepOperations`. |
| **find** | `fd` with glob matching, fallback to `find -name`. Pluggable `FindOperations`. |
| **ls** | Pure Rust via `std::fs::read_dir`. Pluggable `LsOperations`. |

### File mutation queue (`src/builtin/file_mutation_queue.rs`)

Serializes concurrent file mutations targeting the same file path.
Different files run in parallel. Uses `tokio::sync::Notify`.

### DefaultToolRenderer (`src/agent/default_renderer.rs`)

Fallback renderer when no custom `ToolRenderer` is provided.
Provides a simple text-based display for tool calls and results.

### Slash commands — ✅ 22 commands

| Command | Result | Description |
|---------|--------|-------------|
| `/quit` | `Quit` | Graceful shutdown |
| `/stop` | `Stop` | Abort streaming |
| `/model <name>` | `ModelChanged(name)` / `Info` / `OpenModelSelector` | Switch model; no args opens selector; `provider/model` format supported |
| `/settings` | `OpenSettings` | Open interactive settings menu overlay |
| `/scoped-models` | `ScopedModels` | Enable/disable models for Ctrl+P cycling |
| `/extensions` | `ExtensionsToggle` | Enable/disable extensions |
| `/export [path]` | `ExportSession { path }` | Export session (HTML or .jsonl) |
| `/import <path>` | `ImportSession { path }` | Import and resume a session |
| `/clone` | `CloneSession` | Duplicate the current session at current position |
| `/fork [msg-id]` | `ForkSession { message_id }` | Fork from a previous message (interactive selector if no ID) |
| `/tree` | `SessionTree` | Navigate session tree |
| `/session` | `SessionInfo { ... }` | Show session info and stats |
| `/name <name>` | `SessionNamed { name }` | Set session display name |
| `/login [provider] [api-key]` | `Login { provider, api_key }` | Configure provider auth |
| `/logout [provider]` | `Logout { provider }` | Remove provider auth |
| `/new` | `NewSession` | Clear conversation |
| `/compact [instructions]` | `CompactSession(Option<String>)` | Manually compact session context with optional custom instructions |
| `/resume` | `OpenSessionSelector` | Open session selector |
| `/reload` | `Reloaded` | Reload keybindings, extensions, skills, prompts, themes |
| `/copy` | `CopyLastMessage` | Copy last assistant message to clipboard |
| `/hotkeys` | `ShowHelp` | Show keyboard shortcuts (dismiss on any key) |
| `/nextTurn` | `NextTurn { text }` | Queue a message for the next turn |

### Changes from previous version

- **New**: `/extensions`, `/nextTurn`, `/stop`
- **Removed**: `/share`, `/changelog`, `/trust`
- `/stop` replaces Ctrl+C interrupt semantics
- `/extensions` provides runtime toggle for extension enable/disable

---

## Settings (`src/settings.rs`) — ~1851 lines

Same file names and format as pi, under `~/.rab/agent/`. Moved from `src/agent/settings.rs`.

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

### New settings fields since previous version

| Field | Type | Description |
|-------|------|-------------|
| `transport` | `Option<String>` | Transport preference: sse, websocket, websocket-cached, auto |
| `steering_mode` | `Option<String>` | Turn steering mode: one-at-a-time, all, manual |
| `follow_up_mode` | `Option<String>` | Follow-up queue processing mode: all, manual |
| `quiet_startup` | `Option<bool>` | Suppress startup screen |
| `collapse_changelog` | `Option<bool>` | Collapse changelog on startup |
| `enable_skill_commands` | `Option<bool>` | Enable `/skill:name` expansions |
| `enable_install_telemetry` | `Option<bool>` | Install telemetry opt-in |
| `double_escape_action` | `Option<String>` | Action on double Escape |
| `tree_filter_mode` | `Option<String>` | Session tree filter mode |
| `editor_padding_x` | `Option<i32>` | Horizontal editor padding |
| `output_pad` | `Option<i32>` | Output padding |
| `autocomplete_max_visible` | `Option<i32>` | Max visible autocomplete items |
| `show_hardware_cursor` | `Option<bool>` | Use hardware cursor |
| `default_project_trust` | `Option<String>` | Default project trust decision |
| `npm_command` | `Option<Vec<String>>` | NPM command for extensions |
| `websocket_connect_timeout_ms` | `Option<u64>` | WebSocket connect timeout |
| `extensions` | `Vec<String>` | Extension script paths |
| `extensions_config` | `ExtensionsConfig` | Extension enable/disable overrides |
| `skills` | `Vec<String>` | Additional skill paths |
| `prompts` | `Vec<String>` | Additional prompt paths |
| `themes` | `Vec<String>` | Additional theme paths |
| `packages` | `Vec<PackageSource>` | Package sources (npm/git) |

### Settings schema

```json
{
  "defaultModel": "deepseek-v4-flash",
  "defaultThinkingLevel": "high",
  "defaultProvider": "opencode_go",
  "transport": "sse",
  "steeringMode": "one-at-a-time",
  "followUpMode": "all",
  "theme": "dark",
  "verbose": false,
  "hideThinkingBlock": true,
  "collapseToolOutput": true,
  "quietStartup": false,
  "collapseChangelog": false,
  "shellPath": null,
  "shellCommandPrefix": "shopt -s expand_aliases",
  "externalEditor": null,
  "defaultProjectTrust": null,
  "httpProxy": null,
  "httpIdleTimeoutMs": 30000,
  "websocketConnectTimeoutMs": 10000,
  "sessionDir": null,
  "enabledModels": [],
  "doubleEscapeAction": "clear",
  "treeFilterMode": "fuzzy",
  "editorPaddingX": 2,
  "outputPad": 1,
  "autocompleteMaxVisible": 8,
  "showHardwareCursor": false,
  "extensions": [],
  "extensionsConfig": {},
  "skills": [],
  "prompts": [],
  "themes": [],
  "packages": [],
  "compaction": { "enabled": true, "reserveTokens": 16384, "keepRecentTokens": 20000 },
  "branchSummary": { "reserveTokens": 4000, "skipPrompt": false },
  "terminal": { "showImages": true, "imageWidthCells": null, "clearOnShrink": true, "showTerminalProgress": true },
  "images": { "autoResize": true, "blockImages": false },
  "retry": { "enabled": true, "maxRetries": 3, "baseDelayMs": 1000, "provider": { "timeoutMs": 30000, "maxRetries": 2, "maxRetryDelayMs": 30000 } },
  "markdown": { "codeBlockIndent": "  " },
  "warnings": { "anthropicExtraUsage": true },
  "thinkingBudgets": { "minimal": 2000, "low": 4000, "medium": 8000, "high": 16000 },
  "lastChangelogVersion": null,
  "enableAnalytics": false,
  "trackingId": null,
  "enableSkillCommands": true,
  "enableInstallTelemetry": false
}
```

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

---

## Auth (`src/provider/auth.rs`)

Pi-compatible credential store using the `AuthStorageBackend` pattern
(File / InMemory variants) with file locking and OAuth auto-refresh.
Moved from `src/auth.rs` to the provider module.

---

## Prompt templates (`src/agent/prompt_templates.rs`) — ~570 lines

Loads `.md` files from `~/.rab/agent/prompts/` (global) and `.rab/prompts/` (project)
and registers them as `/name` commands. Pi-compatible frontmatter and placeholder expansion.

---

## App layer (`src/agent/ui/app/`) — split from single app.rs

The former `app.rs` (~5K+ lines) was split into:

| File | Description |
|------|-------------|
| `mod.rs` | Main `App` struct, `TUI` event loop, `compose_ui()`, overlay lifecycle, route_input, message queuing |
| `events.rs` | `handle_agent_event()` — processes `AgentEvent` stream for UI updates and persistence |
| `helpers.rs` | `parse_bang_command()`, `xml_escape()`, `strip_frontmatter()`, skill formatting helpers |

### Keybinding actions

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

All bindings customizable via `~/.rab/keybindings.json`.

---

## Storage layout (`~/.rab/`)

```
~/.rab/
├── agent/
│   ├── settings.json          # global settings (pi-compatible nested format)
│   ├── auth.json              # API keys and OAuth credentials
│   ├── models.json            # user provider/model overrides (merged with built-in)
│   ├── SYSTEM.md              # custom system prompt (full override)
│   ├── APPEND_SYSTEM.md       # appended to system prompt
│   ├── AGENTS.md              # global context file
│   ├── mcp.json               # MCP server configuration
│   └── prompts/               # prompt templates (.md files)
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
├── yoagent 0.13.1        (MIT)                - agent loop, provider, types, session tree,
│                                                 skills, MCP types
│   └── reqwest          (Apache 2.0)           - HTTP client (inside yoagent)
├── tokio 1              (MIT)                  - async runtime
├── tokio-util 0.7       (MIT)                  - CancellationToken
├── serde + serde_json 1 (MIT)                  - JSON serialization
├── uuid 1               (MIT)                  - message/session IDs
├── chrono 0.4           (MIT)                  - timestamps
├── directories 6        (MIT)                  - XDG paths
├── anyhow 1             (MIT)                  - error handling
├── futures 0.3          (MIT)                  - StreamExt
├── async-trait 0.1      (MIT)                  - trait async fn
├── colored 3            (MPL-2.0)              - terminal colors
├── crossterm 0.29       (MIT)                  - terminal I/O
├── unicode-segmentation 1 (MIT)                - grapheme-aware cursor movement
├── unicode-width 0.2    (MIT)                  - character display width
├── unicode-normalization 0.1 (MIT)             - Unicode normalization
├── comrak 0.54          (MIT)                  - markdown parsing (GFM)
├── syntect 5.3          (MIT/MPL 2.0, optional) - syntax highlighting
├── base64 0.22          (MIT)                  - image data URL encoding
├── regex 1.12           (MIT)                  - regex
├── reqwest 0.13         (MIT/Apache 2.0)       - HTTP client (rustls-tls, socks)
├── kameleoon-reqwest-eventsource 0.6 (Apache 2.0) - SSE streaming
├── tracing 0.1          (MIT)                  - diagnostic logging
├── fs2 0.4              (MIT)                  - cross-platform file locks
├── rustls 0.23          (Apache 2.0/ISC)       - TLS (replaces rustls-platform-verifier)
├── webpki-root-certs 1  (Apache 2.0)           - root certificate store for Android/Termux
├── libc 0.2             (MIT)                  - system calls
├── url 2.5              (MIT)                  - URL parsing
# wasmtime 26+ (Phase 2, Apache 2.0)
# notify 7    (Phase 2, CC0-1.0)
# rmcp 1      (Phase 2, MIT)
```

No GPL dependencies. All permissive (MIT / Apache 2.0 / MPL-2.0), fully
compatible with EPL-2.0. Phase 2 dependencies (wasmtime, notify, rmcp) are
gated behind Cargo features: `plugins` and `mcp`. MVP compiles without them.

Key dependency changes from earlier versions:
- `reqwest` upgraded from 0.12 to 0.13 (rustls-tls, socks)
- `reqwest-eventsource` replaced with `kameleoon-reqwest-eventsource` 0.6
- `openssl-sys` removed (fully rustls-based)
- `rustls-platform-verifier` replaced with `rustls` + `webpki-root-certs` for Android/Termux fix
- `comrak` upgraded from 0.53 to 0.54
- `edition = "2024"` (Rust 1.96.0) for `impl Trait` in const contexts and RPIT lifetime capture
- `yoagent` is a published crate at version 0.13.1
- `fs2` 0.4 added for cross-platform file locking

---

## Module count

- **Source modules**: 113 `*.rs` files
- **Total lines**: ~53,176
- **Tests**: ~688 `#[test]` annotations
- **TUI modules**: 31 `src/tui/` + 28 `src/agent/ui/` = 59 modules (~28.5K lines)

---

## Phase 2

### WASM plugin system — ⬜ NOT STARTED

Planned: WASM components via wasmtime, loaded from `~/.rab/extensions/`.
Same `Extension` trait used by builtins — plugins implement it via WIT
bindings. Hot reload via file watcher.

### MCP adapter — ✅ IMPLEMENTED

The `src/extensions/mcp/` module (6 modules, ~1200+ lines) provides a full MCP
adapter matching pi-mcp-adapter's architecture. Configured via `~/.rab/agent/mcp.json`
(global) and `.rab/mcp.json` (project-local overrides). See the MCP section in the
diagram for details.

### Tree-sitter — 🟡 SKELETON

`src/extensions/tree_sitter/mod.rs` — skeleton implementation for AST-aware tools.
Not yet fully wired.

### Models.json — ✅ IMPLEMENTED

Built-in catalog at `src/provider/models.json` (~17910 lines). User overrides
merge via `~/.rab/agent/models.json`. The `rab generate-models` subcommand
fetches the latest models from `models.dev` and updates the built-in catalog.

### User extensions (compile-time)

Already possible today — implement the `Extension` trait and register in
`main.rs` → `cli/run.rs`. No dynamic loading yet.

---

## Open questions

- **Image paste in TUI** — clipboard integration differs per platform
  (wl-paste, pbpaste, PowerShell). Kitty protocol covers display; input TBD.
- **Command deny-list** — bash tool currently runs anything. A deny-list or
  sandbox (bubblewrap, landlock) should be configurable.
- **Provider fallback** — if the primary provider fails, should rab retry
  with another? yoagent handles basic retry; full fallback chain TBD.
- **Multi-model cycling** — Ctrl+P model switching uses a scoped-models list.
  A full model registry with metadata (context window, costs) is already
  implemented via `ProviderRegistry` and `ScopedModelsSelector`.
