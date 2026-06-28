# rab Architecture

A lightweight, extensible Rust coding agent inspired by [pi-coding-agent](https://pi.dev).
rab delegates the core agent loop, types, and provider layer to the **yoagent** crate,
providing the session layer, TUI, built-in tools, slash commands, filesystem tools (grep/find/ls),
file mutation queue, and lifecycle management.

---

## Layered architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                          rab (EPL-2.0)                               │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    main.rs (manual arg parsing)                │   │
│  │  arg parsing, env reading, session init,                      │   │
│  │  mode dispatch (print / interactive), extension gating,       │   │
│  │  context file loading, skills, auth                            │   │
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
│  │              Session (session.rs)                             │   │
│  │  High-level API wrapping SessionStorage. Pi-compatible:      │   │
│  │  append_*, build_context, move_to, metadata.                 │   │
│  └──────────┬───────────────────────────────────────────────────┘   │
│             │                                                       │
│  ┌──────────▼───────────────────────────────────────────────────┐   │
│  │              SessionStorage (session_storage.rs) trait        │   │
│  │  Low-level CRUD: leaf mgmt, labels, path queries.            │   │
│  │  Impls: InMemorySessionStorage, JsonlSessionStorage          │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐      │
│  │builtin│ │  tui/  │ │commands│ │extens-│ │settings│ │ auth  │      │
│  │read   │ │ agent/ │ │.rs     │ │ions/  │ │.rs     │ │.rs    │      │
│  │write  │ │ ui/    │ │22 slash│ ┌──────────────┐ │~/.rab/ │ │API    │
│  │edit   │ │screen  │ │commands│ │filesystem (3) │ │settings│ │keys,  │
│  │bash   │ │editor  │ │        │ │mcp/ (6 mods,  │ │AGENTS  │ │OAuth  │
│  │file_  │ │list    │ │        │ │ 2K lines)     │ │.md     │ │       │
│  │mutation│ └───────┘ │        │ │AGENTS.md       │ │skills  │ │       │
│  │_queue │            │        │ │skills          │ │        │ │       │
│  │cancel │            │        │ │                │ │        │ │       │
│  └──┬────┘            │        │ └──────────────┘ └───────┘ └───────┘      │
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
│  │  hook, and bundled ToolRenderer.                             │   │
│  │  validate_tool_arguments() + coerce_with_json_schema()       │   │
│  │  Builtin + user extensions share this trait                  │   │
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
│  │  │  Provider trait + OpenAiCompatProvider               │   │   │
│  │  │  Streaming LLM calls, thinking, tool calls            │   │   │
│  │  │  Provider auto-detection by model prefix              │   │   │
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
│  │  Provider backends (in yoagent, not rab)                     │   │
│  │  OpenCode Go (opencode.ai/zen/go/v1) — default               │   │
│  │  OpenAI, Anthropic, Ollama — auto-detected by model prefix   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  TUI (src/tui/ + src/agent/ui/) — 50+ modules, ~600 tests   │   │
│  │  Direct Rust port on crossterm 0.29                          │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

## Key architectural decisions

- **yoagent is the core dependency**, not genai. rab delegates the agent loop,
  provider layer, and message types to yoagent. rab provides the session layer,
  TUI, built-in tools, filesystem tools, slash commands, and lifecycle
  management on top.

- **One extension mechanism** — built-in tools and user extensions use the same
  `Extension` trait. No separate tool registration path. All tools, commands,
  renderers, and skills go through `Extension`.

- **ToolDefinition wraps every tool** — each `AgentTool` is wrapped in a
  `ToolDefinition` that carries prompt snippet metadata, guidelines, argument
  preparation hooks (`prepare_arguments`), `before_tool_call` and
  `after_tool_call` hooks (pi-compatible), and automatic JSON Schema argument
  coercion + validation.

- **Pluggable operations** — every built-in tool (read, write, edit, bash,
  grep, find, ls) delegates filesystem/shell operations through a trait
  (e.g. `ReadOperations`, `BashOperations`, `GrepOperations`), making it
  possible to replace local execution with remote (SSH) execution.

- **Provider layer lives in yoagent** — rab has no `adapter.rs` or `provider.rs`.
  yoagent's `Provider` trait and `OpenAiCompatProvider` handle all LLM
  communication. OpenCode Go is the default backend; OpenAI, Anthropic, and
  Ollama are auto-detected by model name prefix.

- **Agent loop lives in yoagent** — rab has no `loop.rs`. yoagent's `Agent`
  struct handles streaming, tool execution, and event emission. rab subscribes
  to events via `AgentEvent` for persistence and UI updates.

- **Types from yoagent** — `AgentMessage`, `Message`, `Content`, `AgentTool`
  are all re-exported from `yoagent::types`. rab's `types.rs` is a thin shim
  with helper functions only (no rab-specific enums).

- **File mutation queue** — concurrent file writes/edits to the same file are
  serialized via `with_file_mutation_queue()` so the model can issue multiple
  sequential edits to the same file without races.

## Pi component mapping

| pi component | rab equivalent | Status |
|---|---|---|
| `pi-tui` (terminal UI, components, editor) | `src/tui/` + `src/agent/ui/` | ✅ Complete — 50+ modules, ~600 tests. Direct Rust port on crossterm 0.29. |
| `pi-agent-core` (agent loop, session, compaction, skills) | Delegated to **yoagent** (agent loop, types, provider, skills) + rab's `AgentSession` (session lifecycle, compaction, branching) | ✅ Agent loop in yoagent (`yoagent::agent::Agent`). ✅ Session in `session.rs` (~2800 lines). ✅ SessionStorage in `session_storage.rs` (~400 lines). ✅ Compaction in `compaction.rs` (~680 lines). ✅ Branch summarization in `branch_summary.rs` (~270 lines). ✅ Skills loaded via `yoagent::skills::SkillSet`. |
| `coding-agent` (CLI, extensions, tools, settings, commands) | `main.rs`, `builtin/`, `extensions/`, `settings.rs`, `auth.rs`, `commands.rs` | ✅ Tools (read/write/edit/bash/grep/find/ls), settings, auth, CLI done. ✅ 22 slash commands implemented. ✅ Extension trait with tools, commands, renderers, skills, hooks. |
| `GrepTool`, `FindTool`, `LsTool` (pi agent tools) | `src/extensions/filesystem.rs` | ✅ grep (ripgrep/grep fallback), find (fd/find fallback), ls — all with pluggable operations. |
| MCP adapter (pi-mcp-adapter) | `src/extensions/mcp/` (6 modules, ~2K lines) | ✅ Proxy `mcp` tool, direct tool adapters, SSE-aware HTTP transport, config loading (global+project merge), server lifecycle (lazy connect, idle timeout), persistent metadata cache, tool renderers. |
| provider | `yoagent::provider::OpenAiCompatProvider` | ✅ OpenCode Go default. Auto-detection by model prefix (claude → Anthropic, gpt → OpenAI, ollama → Ollama). |
| `beforeToolCall` / `afterToolCall` | `ToolDefinition.before_tool_call` / `.after_tool_call` | ✅ Per-tool hooks for blocking/preprocessing/postprocessing |
| `validateToolArguments` | `extension::validate_tool_arguments()` | ✅ Full JSON Schema validation with pi-compatible error paths |
| Argument coercion | `extension::coerce_with_json_schema()` | ✅ Type coercion matching pi's `Value.Convert` + `coerceWithJsonSchema` |
| Theme system | `src/agent/ui/theme.rs` | ✅ JSON theme system with resolution, fallback, detection (715 lines) |
| Resource loading (AGENTS.md/CLAUDE.md) | `src/agent/context_files.rs` | ✅ AGENTS.md/CLAUDE.md discovery, `<project_context>` wrapping |
| Skills | `yoagent::skills::SkillSet` + `SystemPromptBuilder.skills()` | ✅ Skill loading, frontmatter, prompt formatting, /skill:name expansion |
| Image support (Kitty protocol) | `src/tui/components/markdown.rs` (hyperlinks) + base64 data URLs | ✅ Image display via Kitty protocol. Input (clipboard paste) TBD. |
| Config files | `~/.rab/` | ✅ Same schema as pi. Auth at `~/.rab/agent/auth.json`. |
| Footer data (git branch, extensions) | `src/agent/footer_data_provider.rs` | ✅ Git branch resolution (worktree/reftable support), extension statuses, provider count |
| File mutation queue | `src/builtin/file_mutation_queue.rs` | ✅ Per-file serialization using tokio::sync::Notify, same pattern as pi |
| MCP extension | `src/extensions/mcp/` | ✅ Proxy `mcp` tool, direct tools, config loading, server lifecycle, cache, renderers |
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
    session: Session,
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
agent_session.set_compaction_config(api_key, &model, context_window);

// Submit user message
let msg = user_message("list .rs files");
agent_session.send_user_message_obj(&msg);

// Spawn yoagent agent loop
let agent = yoagent::agent::Agent::new(OpenAiCompatProvider)
    .with_model(&model)
    .with_api_key(&api_key)
    .with_system_prompt(&system_prompt)
    .with_tools(agent_tools);

agent.prompt_with_sender(msg_text, tx).await;

// Process events — AgentSession persists tool results immediately
while let Some(event) = rx.recv().await {
    agent_session.handle_yo_event(&event);
    // Update UI ...
}

// AgentEnd persists remaining messages
agent_session.check_auto_compact().await;
```

---

## Session layer (`src/agent/session.rs`) — ~2800 lines

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

## Session storage (`src/agent/session_storage.rs`) — ~400 lines

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

## Session repo (`src/agent/session_repo.rs`)

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

## Compaction (`src/agent/compaction.rs`) — ~680 lines ✅ IMPLEMENTED

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

`summarize_text()` (shared with `branch_summary.rs`) calls yoagent's
`OpenAiCompatProvider` with a non-streaming text completion (no tools,
low temperature) to generate summaries.

---

## Branch summarization (`src/agent/branch_summary.rs`) — ~270 lines ✅ IMPLEMENTED

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

## Extension trait (`src/agent/extension.rs`)

All capability — built-in or user-provided — comes through the same trait.
Supports pi-compatible `beforeToolCall` / `afterToolCall` hooks, argument
type coercion via JSON Schema, and full schema validation.

```rust
#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> Cow<'static, str>;

    /// Tools this extension provides (LLM-callable), each with prompt metadata.
    fn tools(&self) -> Vec<ToolDefinition> { vec![] }

    /// Slash commands (e.g. `/quit`, `/model`).
    fn commands(&self) -> Vec<SlashCommand> { vec![] }

    /// Skills this extension provides.
    fn skills(&self) -> yoagent::skills::SkillSet { yoagent::skills::SkillSet::empty() }
}
```

### Supporting types

```rust
/// A tool bundled with its prompt metadata.
pub struct ToolDefinition {
    pub tool: Box<dyn yoagent::types::AgentTool>,
    pub snippet: &'static str,
    pub guidelines: &'static [&'static str],
    /// Optional pre-processing of raw LLM arguments (type coercion).
    pub prepare_arguments: Option<fn(serde_json::Value) -> Result<serde_json::Value, String>>,
    /// Called before tool execution (pi's beforeToolCall).
    pub before_tool_call: Option<fn(&serde_json::Value) -> Option<BeforeToolCallResult>>,
    /// Called after tool execution (pi's afterToolCall).
    pub after_tool_call: Option<fn(&yoagent::types::ToolResult, bool) -> Option<AfterToolCallResult>>,
}

pub struct BeforeToolCallResult { pub block: bool, pub reason: String }
pub struct AfterToolCallResult { pub content: Option<Vec<Content>>, pub details: Option<Value>, pub is_error: Option<bool> }

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

/// Tool-specific rendering interface.
pub trait ToolRenderer: Send + Sync {
    fn render_call(&self, args: &Value, width: usize, theme: &dyn Theme, ctx: &ToolRenderContext) -> Vec<String>;
    fn render_result(&self, content: &str, width: usize, theme: &dyn Theme, ctx: &ToolRenderContext) -> Vec<String>;
    fn render_self(&self) -> bool;
    fn render_bg_key(&self) -> Option<&'static str>;
}

/// Context passed to ToolRenderer methods (pi-compatible).
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
    pub details: Option<serde_json::Value>,       // structured rendering data
    pub invalidate: Option<UnboundedSender<()>>,  // re-render request callback
}
```

### Argument coercion and validation

Every tool call goes through `ToolDefinition::execute()`:

1. **`prepare_arguments`** — custom per-tool pre-processing (e.g., write tool
   coerces `path`/`content` to strings)
2. **`coerce_with_json_schema()`** — recursive type coercion for common LLM
   mistakes (numbers as strings, booleans as strings, null handling, etc.)
3. **`validate_tool_arguments()`** — full JSON Schema validation with
   pi-compatible error paths (Required, additionalProperties, type mismatch)
4. **`before_tool_call`** — optional hook that can block execution
5. **Execute** — call inner `AgentTool::execute()`
6. **`after_tool_call`** — optional hook that can modify the result

At startup, extensions are collected from builtins + filesystem:

```rust
let extensions: Vec<Box<dyn Extension>> = vec![
    Box::new(CommandsExtension::new(available_models)),
    Box::new(ReadExtension::new(cwd)),
    Box::new(WriteExtension::new(cwd)),
    Box::new(EditExtension::new(cwd)),
    Box::new(BashExtension::new(cwd)),
    Box::new(FilesystemExtension::new(cwd)),  // grep, find, ls
];
```

Extension gating is done via `is_extension_active()` which checks
`settings.tools` (whitelist) and `settings.exclude_tools` (blacklist).
Core extensions (commands, read, write, edit, bash) are always active
when no whitelist is set. Grep/find/ls are opt-in via settings.

---

## Built-in extensions (`builtin/`)

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

### Filesystem extension (`src/extensions/filesystem.rs`)

| Tool | Key features |
|------|-------------|
| **grep** | Uses ripgrep (`rg`) with `--json` output, falls back to `grep`. Respects .gitignore. Options: pattern, path, glob, ignoreCase, literal, context, limit (default 100). Pluggable `GrepOperations`. |
| **find** | Uses `fd` (rust rewrite of find) with glob matching and .gitignore awareness, falls back to `find -name`. Options: pattern, path, limit (default 1000). Pluggable `FindOperations`. |
| **ls** | Directory listing with `/` suffix for directories, dotfiles included. Options: path, limit (default 500). Pluggable `LsOperations`. |

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

## Settings (`src/agent/settings.rs`) — ~800 lines

Same file names and format as pi, under `~/.rab/agent/`.

### Config files

| Pi path | rab path | Status |
|---|---|---|
| `~/.pi/agent/settings.json` | `~/.rab/agent/settings.json` | ✅ |
| `.pi/settings.json` | `.rab/settings.json` | ✅ (project-local overrides) |
| `~/.pi/agent/auth.json` | `~/.rab/agent/auth.json` | ✅ |
| `~/.pi/agent/models.json` | `~/.rab/models.json` | ⬜ Not implemented |
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

## CLI (`main.rs`) — manual arg parsing

```
rab [MESSAGE]...

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

**Currently only OpenCode Go is configured** (via `yoagent::provider::OpenAiCompatProvider`
with `openai_compat()` model config pointing to `opencode.ai/zen/go/v1`).
Models: `deepseek-v4-flash` (default), `deepseek-v4-pro`.

Extension gating: `is_extension_active()` checks `settings.tools` whitelist
and `settings.exclude_tools` blacklist. Core extensions are always active
when no whitelist is set. Filesystem tools (grep, find, ls) are opt-in.

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
`agent_session.handle_yo_event()` for persistence.

### Interactive (TUI) mode

Same agent loop, different sink: `App` in `src/agent/ui/` subscribes to the
agent event stream and renders to a pi-tui-style main-screen TUI instead of
stdout. Uses `yoagent::agent::Agent::prompt_with_sender()` with event channels.

---

## TUI (`src/tui/` + `src/agent/ui/`) — 50+ modules, ~600 tests

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
| `terminal.rs` | Terminal abstraction — `TerminalTrait`, `ProcessTerminal`, raw mode, cursor hide/show, synchronized output |
| `keys.rs` | Key event handling — `key_event_to_id()`, 27+ action IDs |
| `keybindings.rs` | JSON keybinding loading from `~/.rab/keybindings.json`, merge, resolution |
| `theme.rs` | Theme trait + default JSON theme loader |
| `fuzzy.rs` | Fuzzy matching for autocomplete |
| `autocomplete.rs` | Editor autocomplete popup — completions, rendering, keyboard navigation |
| `kill_ring.rs` | Kill ring for editor cut/copy/paste |
| `undo_stack.rs` | Undo/redo for editor |
| `word_nav.rs` | Word-boundary navigation utilities |
| `visual_truncate.rs` | Shared `truncate_to_visual_lines()` utility |
| `util.rs` | Shared utilities |

### Components (`src/tui/components/`)

| Module | Description |
|--------|-------------|
| `editor.rs` | Multi-line editor — word-wrap, undo stack, kill ring, paste markers, bracketed paste, history recall, character jump, sticky column, border_color, autocomplete |
| `markdown.rs` | comrak-based renderer with syntax highlighting, tables, code blocks, Kitty hyperlinks |
| `diff.rs` | Unified diff with colored +/lines and intra-line character-level inverse |
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
| `chat_editor.rs` | `ChatEditor` wrapper — input processing, slash command dispatch, /skill:name expansion |
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
| `components/tool_messages.rs` | Tool execution components (read, write, edit, bash, grep, find, ls) |
| `components/bash_execution.rs` | Bash execution component with streaming, duration, borders |
| `components/info_message.rs` | Info message component (dim text) |
| `components/session_picker.rs` | Session selector overlay |
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
  │   ├── BashExecutionComponent
  │   ├── InfoMessageComponent
  │   └── Spacer(1) between each
  ├── pending_section (DynamicLines — streaming text/thinking)
  ├── status_section (DynamicLines — transient status)
  ├── queued_section (DynamicLines — ◷ queued messages)
  ├── working_section (DynamicLines — ⠋ Working...)
  ├── EditorComponent (border color: thinking level / bash mode)
  └── FooterComponent (cwd, git branch, token usage, model, auto-compact)
```

### Message queuing

When the user submits a message while streaming, it is queued (not sent to
a new concurrent agent loop). Queued messages appear between chat and editor.
On `AgentEnd`, the next queued message is auto-submitted. Ctrl+C during
streaming restores queued messages to the editor.

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
│   ├── SYSTEM.md              # custom system prompt (full override)
│   ├── APPEND_SYSTEM.md       # appended to system prompt
│   └── AGENTS.md              # global context file
├── models.json                # ⬜ custom provider/model definitions (not implemented)
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
├── openssl-sys 0.9       (MIT)        - vendored OpenSSL
├── libc 0.2              (MIT)        - system calls
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

See the old architecture doc for the detailed WIT interface and
plugin author experience (design deferred to Phase 2).

### MCP adapter — ✅ IMPLEMENTED

The `src/extensions/mcp/` module (6 modules, ~2K lines) provides a full MCP
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

### models.json — ⬜ NOT IMPLEMENTED

Custom provider/model definitions. Currently OpenCode Go is the only
configured backend.

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
