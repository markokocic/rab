# rab Architecture

A lightweight, extensible Rust coding agent inspired by [pi-coding-agent](https://pi.dev).
rab delegates the core agent loop, types, and provider layer to the **yoagent** crate,
providing the session layer, TUI, built-in tools, slash commands, and lifecycle management.

---

## Layered architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                          rab (EPL-2.0)                               │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    main.rs (manual arg parsing)                │   │
│  │  arg parsing, env reading, session init,                      │   │
│  │  mode dispatch (print / interactive)                          │   │
│  └────────────────────┬─────────────────────────────────────────┘   │
│                       │                                              │
│  ┌────────────────────▼─────────────────────────────────────────┐   │
│  │              AgentSession (agent_session.rs)                  │   │
│  │  Lifecycle layer bridging yoagent events ↔ session storage   │   │
│  │  - Event-driven message persistence (crash-safe)             │   │
│  │  - Model/thinking/tool change detection & recording          │   │
│  │  - Auto/manual compaction (compaction.rs)                    │   │
│  │  - Branch summarization (branch_summary.rs)                  │   │
│  │  - Branch navigation (set_branch)                            │   │
│  └────┬──────────┬──────────┬──────────┬───────────────────────┘   │
│       │          │          │          │                            │
│  ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐      │
│  │builtin│ │  tui/  │ │commands│ │settings│ │ sys   │ │ auth  │      │
│  │read   │ │ agent/ │ │.rs     │ │.rs     │ │prompt │ │.rs    │      │
│  │write  │ │ ui/    │ │22 slash│ │~/.rab/ │ │.rs    │ │API    │      │
│  │edit   │ │screen  │ │commands│ │settings│ │AGENTS │ │keys,  │      │
│  │bash   │ │editor  │ │        │ │        │ │.md    │ │OAuth  │      │
│  │       │ │list    │ │        │ │        │ │skills │ │       │      │
│  │       │ └───────┘ │        │ │        │ │       │ │       │      │
│  └──┬────┘           │        │ └────────┘ └───────┘ └───────┘      │
│     │                │        │                                     │
│     │     impl Extension trait + yoagent::types::AgentTool          │
│     │                                                               │
│  ┌──▼──────────────────────────────────────────────────────────┐   │
│  │              agent/extension.rs (Extension trait)            │   │
│  │  pub trait Extension: Send + Sync {                          │   │
│  │    fn name(&self) -> Cow<'static, str>;                      │   │
│  │    fn tools(&self) -> Vec<ToolWithMeta>;                     │   │
│  │    fn commands(&self) -> Vec<SlashCommand>;                  │   │
│  │    fn tool_renderer(&self, name) -> Option<Box<dyn ToolRenderer>>;│ │
│  │    fn skills(&self) -> SkillSet;                             │   │
│  │  }                                                           │   │
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
│  │  TUI (src/tui/ + src/agent/ui/) — 48 modules, ~508 tests    │   │
│  │  Direct Rust port on crossterm 0.29                          │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

## Key architectural decisions

- **yoagent is the core dependency**, not genai. rab delegates the agent loop,
  provider layer, and message types to yoagent. rab provides the session layer,
  TUI, built-in tools, slash commands, and lifecycle management on top.

- **One extension mechanism** — built-in tools and user extensions use the same
  `Extension` trait. No separate tool registration path. All tools, commands,
  renderers, and skills go through `Extension`.

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

## Pi component mapping

| pi component | rab equivalent | Status |
|---|---|---|
| `pi-tui` (terminal UI, components, editor) | `src/tui/` + `src/agent/ui/` | ✅ Complete — 48 modules, ~508 tests. Direct Rust port on crossterm 0.29. |
| `pi-agent-core` (agent loop, session, compaction, skills) | Delegated to **yoagent** (agent loop, types, provider, skills) + rab's `AgentSession` (session lifecycle, compaction, branching) | ✅ Agent loop in yoagent (`yoagent::agent::Agent`). ✅ Session in `session.rs` (2749 lines). ✅ Compaction in `compaction.rs` (679 lines) — fully implemented. ✅ Branch summarization in `branch_summary.rs` (270 lines). ✅ Skills loaded via `yoagent::skills::SkillSet`. |
| `coding-agent` (CLI, extensions, tools, settings, commands) | `main.rs`, `builtin/`, `settings.rs`, `auth.rs`, `commands.rs` | ✅ Tools (read/write/edit/bash), settings, auth, CLI done. ✅ 22 slash commands implemented. ✅ Extension trait with tools, commands, renderers, skills. |
| provider | `yoagent::provider::OpenAiCompatProvider` | ✅ OpenCode Go default. Auto-detection by model prefix (claude → Anthropic, gpt → OpenAI, ollama → Ollama). |
| Theme system | `src/agent/ui/theme.rs` | ✅ JSON theme system with resolution, fallback, detection (715 lines) |
| Resource loading (AGENTS.md/CLAUDE.md) | `src/agent/context_files.rs` | ✅ AGENTS.md/CLAUDE.md discovery, `<project_context>` wrapping |
| Skills | `yoagent::skills::SkillSet` | ✅ Skill loading, frontmatter, prompt formatting, /skill:name expansion |
| Image support (Kitty protocol) | `src/tui/components/markdown.rs` (hyperlinks) + base64 data URLs | ✅ Image display via Kitty protocol. Input (clipboard paste) TBD. |
| Config files | `~/.rab/` | ✅ Same schema as pi. Auth at `~/.rab/agent/auth.json`. |
| MCP extensions | Not started | ⬜ Phase 2 |
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
// Message::Assistant { content, model, provider, usage, ... }
// Message::ToolResult { tool_call_id, content, is_error, ... }
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

The `AgentSession` struct coordinates the session lifecycle, bridging the
yoagent agent loop event stream with session persistence.

```rust
pub struct AgentSession {
    session: SessionManager,
    last_model: Option<(String, String)>,           // change detection
    last_thinking_level: String,                    // change detection
    last_active_tools: Option<Vec<String>>,          // change detection
    persisted_message_ids: HashSet<String>,          // dedup tracking
    persisted_tool_call_ids: HashSet<String>,        // dedup tracking
    compaction_settings: CompactionSettings,
    context_window: u64,
    model_name: String,
    compaction_api_key: Option<String>,
}
```

### Responsibilities

1. **Event-driven persistence** — `handle_yo_event()` persists `ToolExecutionEnd`
   events immediately (crash-safe). `on_agent_end()` persists remaining
   assistant messages not yet captured. Uses `persisted_message_ids` /
   `persisted_tool_call_ids` for deduplication.

2. **Model/thinking/tool change tracking** — `on_model_change()`,
   `on_thinking_level_change()`, `on_active_tools_change()` detect changes and
   append metadata entries to the session (diff-based — only appends when
   value differs from last known).

3. **Auto-compaction** — `check_auto_compact()` runs after the agent finishes
   a turn. Checks `should_compact()` against the model's context window,
   calls `compact()` to generate a summary, and appends a `CompactionEntry`.

4. **Manual compaction** — `run_manual_compact()` for `/compact` command.

5. **Branch summarization** — `summarize_branch_navigation()` summarises
   abandoned branches when navigating the session tree. `set_branch()` moves
   the leaf pointer and optionally triggers branch summarization.

6. **New session** — `new_session()` resets all tracked state.

### Typical usage in print mode

```rust
let mut agent_session = AgentSession::new(session);
agent_session.set_compaction_config(api_key, &model, context_window);

// Submit user message
let msg = user_message("list .rs files");
agent_session.submit_user_message_obj(&msg);

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

## Session layer (`src/agent/session.rs`) — 2749 lines

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
`parentId`, forming a tree. Branching moves the leaf pointer without
modifying history — entries are append-only.

### SessionManager

```rust
pub struct SessionManager {
    storage: Box<dyn SessionStorage>,
    session_id: String,
    session_file: Option<PathBuf>,
    session_dir: PathBuf,
    cwd: PathBuf,
    persist: bool,
    session_header: Option<SessionHeader>,
    file_entries: Vec<SessionEntry>,
    by_id: HashMap<String, SessionEntry>,
    leaf_id: Option<String>,
    // labels_by_id, label_timestamps_by_id
}
```

Key methods: `create()`, `open()`, `continue_recent()`, `fork_from()`,
`append_message()`, `append_model_change()`, `append_thinking_level_change()`,
`append_compaction()`, `append_branch_summary()`, `build_session_context()`,
`set_branch()`, `list_sessions()`, `session_info()`.

---

## Session storage (`src/agent/session_storage.rs`)

Abstract storage trait for session persistence:

```rust
pub trait SessionStorage: Send {
    fn load(&self) -> (Option<SessionHeader>, Vec<SessionEntry>);
    fn append(&self, entry: &SessionEntry) -> io::Result<()>;
    fn write_full(&self, header: &SessionHeader, entries: &[SessionEntry]) -> io::Result<()>;
    fn path(&self) -> Option<&Path>;
    fn exists(&self) -> bool;
}
```

Implementations:
- **`JsonlSessionStorage`** — file-backed JSONL file on disk (default).
- **`InMemorySessionStorage`** — no-op, discards all writes (for `--no-session`).

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

## Compaction (`src/agent/compaction.rs`) — 679 lines ✅ IMPLEMENTED

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

Defaults: enabled, 16K reserve, 20K keep recent.

### Manual trigger

Via `/compact` slash command → `AgentSession::run_manual_compact()` →
`compact()` → append `CompactionEntry`.

### Shared summarization helper

`summarize_text()` (shared with `branch_summary.rs`) calls yoagent's
`OpenAiCompatProvider` with a non-streaming text completion (no tools,
low temperature) to generate summaries.

---

## Branch summarization (`src/agent/branch_summary.rs`) — 270 lines ✅ IMPLEMENTED

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
   Append a `BranchSummaryEntry` with file operation details.

---

## Extension trait (`src/agent/extension.rs`)

All capability — built-in or user-provided — comes through the same trait.

```rust
#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> Cow<'static, str>;

    /// Tools this extension provides (LLM-callable), each with prompt metadata.
    fn tools(&self) -> Vec<ToolWithMeta> { vec![] }

    /// Slash commands (e.g. `/quit`, `/model`).
    fn commands(&self) -> Vec<SlashCommand> { vec![] }

    /// Tool-specific renderer for the TUI.
    fn tool_renderer(&self, _name: &str) -> Option<Box<dyn ToolRenderer>> { None }

    /// Skills this extension provides.
    fn skills(&self) -> yoagent::skills::SkillSet { yoagent::skills::SkillSet::empty() }
}
```

### Supporting types

```rust
/// A tool bundled with its prompt metadata (replaces old snippet/guideline hooks).
pub struct ToolWithMeta {
    pub tool: Box<dyn yoagent::types::AgentTool>,
    pub snippet: &'static str,
    pub guidelines: &'static [&'static str],
    pub prepare_arguments: Option<fn(serde_json::Value) -> Result<serde_json::Value, String>>,
}

pub struct AutocompleteItem { pub value: String, pub label: String, pub description: Option<String> }

pub trait CommandHandler: Send + Sync {
    fn execute(&self, args: &str) -> anyhow::Result<CommandResult>;
    fn argument_completions(&self, prefix: &str) -> Vec<AutocompleteItem>;
}

pub enum CommandResult { Info(String), Quit, ModelChanged(String), ... }

pub struct SlashCommand { pub name: String, pub description: String, pub handler: Box<dyn CommandHandler> }

pub trait ToolRenderer: Send + Sync {
    fn render_call(&self, args: &Value, width: usize, theme: &dyn Theme, ctx: &ToolRenderContext) -> Vec<String>;
    fn render_result(&self, content: &str, width: usize, theme: &dyn Theme, ctx: &ToolRenderContext) -> Vec<String>;
    fn render_self(&self) -> bool;
    fn render_bg_key(&self) -> Option<&'static str>;
}
```

At startup, extensions are collected from builtins:

```rust
let extensions: Vec<Box<dyn Extension>> = vec![
    Box::new(CommandsExtension::new(available_models)),
    Box::new(ReadExtension::new(cwd)),
    Box::new(WriteExtension::new(cwd)),
    Box::new(EditExtension::new(cwd)),
    Box::new(BashExtension::new(cwd)),
];
```

Tools, commands, renderers, and hooks are all derived by
flattening all extensions — no separate registration path. `ToolWithMeta`
wraps each inner `AgentTool` with prompt snippets and guidelines, and
`impl AgentTool for ToolWithMeta` handles argument normalization (shape guard,
null stripping, per-tool `prepare_arguments`).

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
| **read** | Path resolution, line numbers, 50KB truncation. Image support (base64 data URL). |
| **write** | Temp file + atomic rename, parent dir creation |
| **edit** | Exact-match search/replace, error on zero/multiple matches. Diff rendering with intra-line character-level inverse. |
| **bash** | `sh -c <command>`, 120s default timeout, streaming via ToolProgress. Last 2000 lines / 50KB truncation. Security: no deny-list (run anything). |

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

Load order: `~/.rab/agent/auth.json`. Currently only `opencode-go` provider
is configured (API key from `oc_...`).

---

## Settings (`src/agent/settings.rs`) — 798 lines

Same file names and format as pi, under `~/.rab/agent/`.

### Config files

| Pi path | rab path | Status |
|---|---|---|
| `~/.pi/agent/settings.json` | `~/.rab/agent/settings.json` | ✅ |
| `.pi/settings.json` | `.rab/settings.json` | ✅ (project-local overrides) |
| `~/.pi/agent/auth.json` | `~/.rab/agent/auth.json` | ✅ |
| `~/.pi/agent/models.json` | `~/.rab/models.json` | ⬜ Not implemented |
| `~/.pi/agent/AGENTS.md` | `~/.rab/AGENTS.md` | ✅ |
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
    "collapseToolOutput": true
}
```

Load order: global `~/.rab/agent/settings.json`, then project `.rab/settings.json`
overlays. CLI flags (`--model`, `--thinking`, `--no-context-files`, etc.) take
precedence over both.

---

## System prompt (`system_prompt.rs`) — 399 lines

Built via `SystemPromptBuilder`:

1. **Default prompt** — tool descriptions, response format, tool guidelines.
2. **Custom SYSTEM.md** — `~/.rab/agent/SYSTEM.md` (global) or `.rab/SYSTEM.md`
   (project). Replaces the default prompt.
3. **APPEND_SYSTEM.md** — appended after all prompts.
4. **Context files** — AGENTS.md/CLAUDE.md walked from cwd to root,
   wrapped in `<project_instructions path="...">` tags.
5. **Skills** — available skills listed as `<available_skills>` XML.
6. **Date and cwd** — `Current date: YYYY-MM-DD`, `Current working directory: /path`.

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

**Currently only OpenCode Go is configured** (via `yoagent::provider::OpenAiCompatProvider`
with `openai_compat()` model config pointing to `opencode.ai/zen/go/v1`).
Models: `deepseek-v4-flash` (default), `deepseek-v4-pro`.

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
Tool calls and results shown prefixed. Uses a simple event loop:
`yoagent::agent::Agent::prompt_with_sender()` → process `AgentEvent` stream →
`AgentSession::handle_yo_event()` for persistence.

### Interactive (TUI) mode

Same agent loop, different sink: `App` in `src/agent/ui/` subscribes to the
agent event stream and renders to a pi-tui-style main-screen TUI instead of
stdout. Uses `yoagent::agent::Agent::prompt_with_sender()` with event channnels.

---

## TUI (`src/tui/` + `src/agent/ui/`) — 48 modules, ~508 tests

The TUI library is a 1/1 port of pi's `@earendil-works/pi-tui`.

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
| `app.rs` | Main `App` struct — event handler, agent loop management, message queuing, compose_ui |
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
| `components/tool_messages.rs` | Tool execution components (read, write, edit, bash) |
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
  │   ├── ToolExecComponent (read/write/edit/bash)
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

### MCP adapter — ⬜ NOT STARTED

Planned: An `Extension` that connects to MCP servers via `rmcp` and
exposes their tools. Configured via `.rab/mcp.json`.

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
