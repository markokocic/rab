# rab Architecture

A minimal Rust reimplementation of [pi-coding-agent](https://pi.dev) — a
terminal coding harness that gives an LLM tools (read, write, edit, bash) and
lets it act on your codebase.

## Pi component mapping

| pi (`packages/`) | rab equivalent | Notes |
|---|---|---|
| `pi-ai` (providers, streaming, models) | `Provider` trait + `adapter/genai.rs` → [genai](https://github.com/jeremychone/rust-genai) crate | Isolated behind trait; swappable. PoC targets [OpenCode Go](https://opencode.ai/docs/go/) (DeepSeek V4 Flash/Pro) via genai's OpenAI adapter. Phase 1 adds Anthropic, OpenAI, Google, Ollama |
| `pi-agent-core` (agent loop, session, compaction, skills) | `agent.rs`, `session.rs`, `compaction.rs`, `types.rs` | Loop ported directly from `agent-loop.ts` |
| `pi-tui` (terminal UI, components, editor) | [ratatui](https://ratatui.rs) 0.29 + [tui-textarea](https://github.com/rhysd/tui-textarea) 0.7 + [crossterm](https://github.com/crossterm-rs/crossterm) 0.28 | Thin glue in `tui.rs` (~150 lines). ratatui does diff, layout, widgets |
| `coding-agent` (CLI, extensions, built-in tools, settings) | `cli.rs`, `extension.rs`, `builtin/`, `commands.rs`, `settings.rs` | Single `Extension` trait for built-in + user extensions; core commands in `commands.rs` |
| `coding-agent/modes/interactive` | `tui.rs` module | Same crate, different event sink |
| MCP extensions (third-party) | `pi-mcp-adapter` built-in extension | Phase 2. Uses `rmcp` crate. Configured via `.rab/mcp.json` |
| Config files (`~/.pi/agent/`) | `~/.rab/` | Same file names and JSON schema as pi |

## Design constraints

- **One extension mechanism** — built-in tools and user extensions use the same
  `Extension` trait. No separate tool registration path. `--no-builtin-tools`
  just skips loading builtins; user extensions still load.
- **No live-reload of extensions** — extensions are compiled in, not hot-reloaded.
- **Provider layer is isolated behind a trait** — rab defines its own `Provider`
  trait. The default implementation wraps [genai](https://github.com/jeremychone/rust-genai)
  (Apache 2.0, 711★, 50 contributors). The agent loop depends only on the trait,
  so genai can be swapped for another backend without touching loop logic.
- **Agent loop mirrors pi** — steering queues, follow-up queues, hook-based
  tool lifecycle, event stream. Ported from pi's `runAgentLoop` in
  `packages/agent/src/agent-loop.ts`.

## License

rab is **EPL-2.0**. The `genai` dependency is Apache 2.0 (compatible) but
isolated behind a trait — replaceable with no changes to core logic.

---

## Layered architecture

```
┌──────────────────────────────────────────────────────────┐
│                     rab (EPL-2.0)                        │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │                 cli.rs                            │   │
│  │  clap-based arg parsing, env reading,             │   │
│  │  mode dispatch (print / interactive)              │   │
│  └────────────────────┬─────────────────────────────┘   │
│                       │                                   │
│  ┌────────────────────▼─────────────────────────────┐   │
│  │               agent.rs                             │   │
│  │  Agent struct, run_agent_loop(), event stream,    │   │
│  │  steering/follow-up queues, hook pipeline         │   │
│  │  depends on: Provider trait (not genai)           │   │
│  └────┬──────────┬──────────┬──────────┬────────────┘   │
│       │          │          │          │                  │
│  ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐ ┌────▼──┐     │
│  │builtin│ │session│ │commands│ │settings│ │  sys  │     │
│  │read   │ │.rs    │ │.rs     │ │.rs     │ │prompt │     │
│  │write  │ │JSONL  │ │/model  │ │~/.rab/ │ │.rs    │     │
│  │edit   │ │append │ │/tree   │ │settings│ │AGENTS │     │
│  │bash   │ │walk   │ │/compact│ │        │ │.md    │     │
│  └──┬────┘ └───────┘ └───────┘ └────────┘ └───────┘     │
│     │  impl Extension trait                               │
│  ┌──▼───────────────────────────────────────────────┐   │
│  │            extension.rs  (Extension trait)         │   │
│  │  pub trait Extension {                             │   │
│  │    fn tools(&self) -> Vec<AgentTool>;              │   │
│  │    fn commands(&self) -> Vec<SlashCommand>;        │   │
│  │    fn hooks(&self) -> ...;                         │   │
│  │  }                                                 │   │
│  │  Builtin + user extensions share this trait        │   │
│  └───────────────────────────────────────────────────┘   │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │            provider.rs  (rab trait)                │   │
│  │  pub trait Provider { ... }                        │   │
│  │  pub struct StreamEvent { ... }                    │   │
│  │  Agent loop depends ONLY on this, not on genai     │   │
│  └────────────────────┬─────────────────────────────┘   │
│                       │                                   │
│  ┌────────────────────▼─────────────────────────────┐   │
│  │          adapter/genai.rs  (impl Provider)         │   │
│  │  struct GenaiAdapter { client: genai::Client }     │   │
│  │  impl Provider for GenaiAdapter { ... }            │   │
│  │  The only file that imports genai                  │   │
│  └────────────────────┬─────────────────────────────┘   │
│                       │                                   │
└───────────────────────┼───────────────────────────────────┘
                        │
               ┌────────▼────────┐
               │ genai (Apache   │
               │ 2.0)            │
               │ replaceable     │
               └─────────────────┘
```

---

## Core type system (`types.rs`)

### AgentMessage

The universal message type. Every entry in a session transcript is one of these.

```
AgentMessage
├── id: String                      # UUID v4
├── parent_id: Option<String>       # for session tree (MVP: linear)
├── role: Role                      # User | Assistant | ToolResult
├── content: String                 # text content
├── tool_calls: Vec<ToolCall>       # present on Assistant messages
├── tool_call_id: Option<String>    # present on ToolResult messages
├── usage: Option<Usage>            # tokens in/out/cache, present on Assistant
├── is_error: bool                  # for ToolResult: was execution an error?
└── timestamp: i64                  # Unix millis
```

### AgentTool trait

```rust
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON Schema
    fn label(&self) -> &str;                    // human-readable for UI
    async fn execute(&self, tool_call_id: String, args: Value)
        -> Result<String>;                      // error → is_error ToolResult
}
```

### AgentEvent

Emitted by the loop for consumers (print mode writes to stdout; TUI later
renders to screen). Mirrors pi's `AgentEvent` union.

```
AgentEvent
├── AgentStart
├── TurnStart
├── TextDelta { delta: String }
├── ThinkingDelta { delta: String }
├── ToolCall { id, name, args }
├── ToolResult { id, name, content, is_error }
├── TurnEnd
├── AgentEnd { messages: Vec<AgentMessage> }
```

---

## Agent loop (`agent.rs`)

Adapted directly from pi's `runAgentLoop` in `agent-loop.ts`. The loop is the
heart of the system — everything else feeds into or reads from it.

### Pseudocode

```rust
async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    context: &AgentContext,      // system_prompt + tools (flattened from all extensions) + history
    config: &LoopConfig,         // model, thinking, hooks, queues
    emit: &dyn EventSink,
    signal: CancellationToken,
) -> Result<Vec<AgentMessage>> {
    let mut messages = context.messages.clone();
    messages.extend(prompts);
    let mut new_messages: Vec<AgentMessage> = prompts.clone();

    emit(AgentStart);
    emit(TurnStart);

    // Outer loop: restarts on follow-up messages
    loop {
        // Inner loop: stream LLM → execute tools → repeat
        loop {
            // 1. Convert AgentMessage[] to LLM format
            let llm_messages = convert::to_llm(&messages);

            // 2. Stream assistant response
            let response = stream_assistant(
                &config.model, &context.system_prompt,
                &llm_messages, &context.tools, signal
            ).await?;

            // 2a. Emit deltas as they arrive
            emit(TextDelta { delta: response.text });
            messages.push(response.as_message());
            new_messages.push(response.as_message());

            // 2b. Handle errors / abort
            if response.stop_reason == "error" || signal.is_cancelled() {
                emit(AgentEnd { messages: new_messages });
                return Ok(new_messages);
            }

            // 3. Execute tool calls (parallel by default)
            if !response.tool_calls.is_empty() {
                for tc in &response.tool_calls {
                    emit(ToolCall { id: tc.id, name: tc.name, args: tc.args });
                    let result = execute_tool(&context.tools, tc, signal).await;
                    let msg = AgentMessage::tool_result(tc.id, &result);
                    emit(ToolResult { ... });
                    messages.push(msg);
                    new_messages.push(msg);
                }
                // Loop continues — tool results go back to LLM
                continue;
            }

            // 4. No tool calls — turn complete
            emit(TurnEnd);

            // 5. Check steering queue (inject mid-run)
            if let Some(steering) = config.poll_steering().await {
                messages.push(steering.clone());
                new_messages.push(steering);
                continue;   // re-enter inner loop with steering message
            }

            break;  // inner loop done
        }

        // 6. Check follow-up queue (only after agent would stop)
        if let Some(follow_up) = config.poll_follow_up().await {
            messages.push(follow_up.clone());
            new_messages.push(follow_up);
            continue;   // re-enter outer loop
        }

        break;  // outer loop done
    }

    emit(AgentEnd { messages: new_messages });
    Ok(new_messages)
}
```

### Hook pipeline

Hooks live on the `Extension` trait, not on the loop config. When a tool
is about to execute, all extensions are consulted:

```rust
// In execute_tool():
for ext in &context.extensions {
    if let Some(reason) = ext.before_tool_call(&tool_call, &context).await {
        return ToolResult::blocked(reason);
    }
}
let result = tool.execute(args).await;
for ext in &context.extensions {
    if let Some(override) = ext.after_tool_call(&tool_call, &result).await {
        // patch result
    }
}
```

Every hook receives the agent's `CancellationToken` and must honour it.

### Queue modes

- **Steering queue**: injected after the current assistant turn finishes
  executing tool calls. Used for mid-run user input.
- **Follow-up queue**: injected only after the agent would otherwise stop
  (no tool calls, no steering). Used for post-run follow-up questions.
- Both support `one-at-a-time` and `all` drain modes.

### Tool execution modes

| Mode | Behaviour |
|------|-----------|
| `parallel` (default) | Preflight all tool calls, execute concurrent, emit results in source order |
| `sequential` | Execute one tool at a time, feed result before starting next |

A tool can override the global mode via `AgentTool::execution_mode`.

---

## Session layer (`session.rs`)

### Format

JSONL file, one object per line. Same format as pi's sessions.

```jsonl
{"id":"01J...1","parentId":null,"role":"user","content":"list .rs files","timestamp":1700000000000}
{"id":"01J...2","parentId":"01J...1","role":"assistant","content":"Found 3 files: ...","usage":{"input":50,"output":80},"timestamp":1700000001000}
{"id":"01J...3","parentId":"01J...2","role":"toolResult","toolCallId":"tool_01","content":"src/main.rs\nsrc/lib.rs","timestamp":1700000002000}
```

### Storage

```
~/.rab/
├── settings.json              # global settings
├── models.json                # custom provider/model definitions
├── keybindings.json           # custom keybinds (phase 2)
├── AGENTS.md                  # global context file
├── extensions/                # user extensions (phase 2)
├── skills/                    # agent skills (phase 2)
├── themes/                    # TUI themes (phase 2)
└── sessions/
    └── <cwd-hash>/            # one directory per project
        ├── 01J...abc.jsonl
        └── 01J...def.jsonl

./
├── .rab/
│   └── settings.json          # project-local overrides
├── AGENTS.md                  # project context (also walks parent dirs)
└── CLAUDE.md                  # alias for AGENTS.md
```

### Session struct

```rust
struct SessionManager {
    path: PathBuf,             // path to .jsonl file
}

impl SessionManager {
    fn create(cwd: &Path) -> Self;
    fn open(path: &Path) -> Self;
    fn continue_recent(cwd: &Path) -> Option<Self>;
    fn append(&mut self, entry: &AgentMessage) -> Result<()>;
    fn messages(&self) -> Result<Vec<AgentMessage>>;    // walk from root
    fn id(&self) -> &str;
}
```

Every entry has a `parentId`, so sessions are a tree from day one. Messages
are resolved by walking from the root along the active branch. Branching
happens when a new entry points to a non-tail parent — no format changes
needed.

## Compaction (`compaction.rs`)

When the conversation approaches the model's context window, older messages
are summarized to free space. Ported from pi's compaction algorithm.

```
Original: [sys] [user1] [asst1+tool] [user2] [asst2+tool] [user3]
Compacted: [sys] [summary_of_1_and_2] [user3]
```

Algorithm:
1. **Check threshold** — estimate total tokens. If under limit, skip.
2. **Find cut point** — walk messages from oldest to newest, accumulating
tokens. Cut where the tail (newest messages) fits in the remaining budget.
3. **Generate summary** — prompt a fast model with the older messages to
produce a concise summary. The summary replaces the older entries.
4. **Replace** — swap old messages with a single synthetic user message
containing the summary. Tool results are included in what gets summarized.

Manual trigger via `/compact` (TUI). Automatic trigger before context
overflow causes an error.

---

## Extension trait (`extension.rs`)

All capability — built-in or user-provided — comes through the same trait.
There is no separate tool registration path.

```rust
#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> &str;

    /// Tools this extension provides (LLM-callable).
    fn tools(&self) -> Vec<Box<dyn AgentTool>> { vec![] }

    /// Additional slash commands (e.g. `/mycommand`).
    /// Core commands (/model, /tree, /compact, ...) are handled by the agent,
    /// not through this trait.
    fn commands(&self) -> Vec<SlashCommand> { vec![] }

    /// Called before any tool executes. Return Some(reason) to block.
    async fn before_tool_call(&self, _tc: &ToolCall, _ctx: &AgentContext)
        -> Option<BlockReason> { None }

    /// Called after a tool executes. Return Some(text) to replace result.
    async fn after_tool_call(&self, _tc: &ToolCall, _result: &str)
        -> Option<String> { None }
}
```

At startup, extensions are collected from builtins and (later) user-provided
paths. Tools are derived by flattening all extensions:

```rust
fn collect_tools(exts: &[Box<dyn Extension>]) -> Vec<Box<dyn AgentTool>> {
    exts.iter().flat_map(|ext| ext.tools()).collect()
}
```

`--no-builtin-tools` simply skips loading builtin extensions; user extensions
still load. `--no-extensions` skips both.

## Built-in extensions (`builtin/`)

Each built-in tool is an `Extension` that provides exactly one tool. They
serve as the reference implementation for user extensions.

### read

| Field | Value |
|-------|-------|
| **Parameters** | `path: string`, `offset?: int`, `limit?: int` |
| **Behaviour** | Reads file contents. Prefixed line numbers. Truncated at 50KB. |
| **Image support** | If path ends in `.png`/`.jpg`/`.gif`/`.webp`, reads as base64 image (passed via the `Provider` trait's multimodal payload, adapter-specific) |

### write

| Field | Value |
|-------|-------|
| **Parameters** | `path: string`, `content: string` |
| **Behaviour** | Creates parent directories. Writes to temp file, then atomic rename. Returns success/error message. |

### edit

| Field | Value |
|-------|-------|
| **Parameters** | `path: string`, `search: string`, `replace: string` |
| **Behaviour** | Reads file, finds exact-match `search`, replaces with `replace`. Error if `search` appears zero or >1 times. |

### bash

| Field | Value |
|-------|-------|
| **Parameters** | `command: string`, `timeout_secs?: int` (default 120) |
| **Behaviour** | Runs `sh -c <command>`. Captures stdout + stderr combined. Truncated to last 2000 lines / 50KB. |
| **Security** | Command deny-list (optional for MVP). Working directory is the project root. |

## Slash commands

Core commands live in the agent, not in extensions. Extensions can register
additional commands via `Extension::commands()`.

### Built-in commands

| Command | Handler |
|---|---|
| `/model <name>` | Switches active model. Parses provider from name prefix (`claude*`, `gpt*`, `gemini*`). |
| `/thinking <level>` | Sets thinking level: `off`, `minimal`, `low`, `medium`, `high`. |
| `/compact [prompt]` | Manually compacts context. Optional custom summary prompt. |
| `/session` | Prints current session ID, path, message count, token totals. |
| `/name <text>` | Sets session display name (saved in session metadata). |
| `/tree` | Opens session branch navigator (TUI). |
| `/fork` | Forks session from a previous user message into a new session file. |
| `/clone` | Duplicates current active branch into a new session file. |
| `/resume` | Lists previous sessions in cwd for selection. |
| `/new` | Starts a fresh session, saving the current one. |
| `/copy` | Copies last assistant message to clipboard. |
| `/export [path]` | Exports session to HTML file. |
| `/settings` | Opens settings editor (TUI) or prints current settings (print mode). |
| `/reload` | Reloads AGENTS.md, skills, settings. |
| `/quit` | Exits (interactive mode only). |

### Extension commands

```rust
// Extension trait (in extension.rs)
trait Extension {
    fn commands(&self) -> Vec<SlashCommand> { vec![] }
}

struct SlashCommand {
    name: &'static str,     // "/mycommand"
    description: &'static str,
    handler: fn(args: &str, ctx: &mut CommandContext) -> Result<String>,
}
```

User extensions add custom `/` commands through the same trait. Conflict
resolution: first registered wins (builtins first, then user extensions
in load order).

---

## System prompt (`system_prompt.rs`)

Built from the same sources as pi, concatenated:

1. **Base prompt** — hardcoded, describes available tools, response format,
   edit tool semantics, `@` file references.
2. **Global AGENTS.md** — `~/.rab/AGENTS.md` (user-wide instructions).
3. **Project context files** — walked up from cwd, loads `AGENTS.md` and
   `CLAUDE.md` (alias) from every ancestor directory. Each file wrapped in
   `<project_context>` tags.

Also respects `APPEND_SYSTEM.md` and `SYSTEM.md` (full override) with the
same discovery rules as pi.

Disable with `--no-context-files` / `-nc`.

---

## Provider trait (`provider.rs`)

rab defines its own provider abstraction. The agent loop depends on this
trait, never on genai directly. To swap backends, write a new impl — no
changes to `agent.rs`.

```rust
/// Events emitted during a streaming LLM request.
pub enum StreamEvent {
    TextDelta { text: String },
    ThinkingDelta { text: String },
    ToolCall { id: String, name: String, arguments: String },
    Done {
        text: String,
        usage: Usage,
        stop_reason: StopReason,
        tool_calls: Vec<ToolCall>,
    },
    Error { message: String },
}

pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Error,
}

/// The one thing the agent loop needs from a provider.
#[async_trait]
pub trait Provider: Send + Sync {
    async fn stream(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
        tools: &[ToolDef],
        signal: CancellationToken,
    ) -> Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>>;
}
```

The trait takes `AgentMessage` directly — no intermediate conversion layer.
Each adapter translates rab types into its own backend format internally.

### Genai adapter (`adapter/genai.rs`)

The **only file** that imports genai. Wraps `genai::Client`, configured for the
target provider.

**PoC:** Client configured for OpenCode Go (`https://opencode.ai/zen/go/v1`)
using genai's OpenAI adapter. Models: `deepseek-v4-flash` (default), `deepseek-v4-pro`.

**Phase 1:** Extended with provider auto-detection from model name prefix —
`claude*` → Anthropic, `gpt*` → OpenAI, `gemini*` → Google, fallback → Ollama.
OpenCode Go remains available as an explicit provider.

```rust
pub struct GenaiProvider {
    client: genai::Client,
}

#[async_trait]
impl Provider for GenaiProvider {
    async fn stream(&self, model: &str, system: &str,
                    messages: &[AgentMessage], tools: &[ToolDef],
                    signal: CancellationToken)
        -> Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>>
    {
        let req = ChatRequest::new(to_genai_messages(messages))
            .with_system(system)
            .with_tools(to_genai_tools(tools));
        let genai_stream = self.client
            .exec_chat_stream(model, req, None).await?;
        Ok(Box::pin(genai_stream.map(|ev| convert_event(ev))))
    }
}
```

`agent.rs` only sees `Box<dyn Provider>`.

Before the provider call, `transform_context` can prune or inject
AgentMessages (e.g. for compaction, later).

---

## Settings (`settings.rs`)

Same file names and format as pi, but under `~/.rab/` instead of
`~/.pi/agent/`.

### Config files

| Pi path | rab path | Purpose |
|---|---|---|
| `~/.pi/agent/settings.json` | `~/.rab/settings.json` | Global settings (model, thinking, session dir) |
| `.pi/settings.json` | `.rab/settings.json` | Project-local overrides |
| `~/.pi/agent/AGENTS.md` | `~/.rab/AGENTS.md` | Global context instructions |
| `AGENTS.md` / `CLAUDE.md` | `AGENTS.md` / `CLAUDE.md` | Project context files (walked up from cwd) |
| `~/.pi/agent/keybindings.json` | `~/.rab/keybindings.json` | Custom keybinds (phase 2) |
| `~/.pi/agent/models.json` | `~/.rab/models.json` | Custom provider/model definitions |
| `~/.pi/agent/sessions/` | `~/.rab/sessions/` | Session files |
| `~/.pi/agent/extensions/` | `~/.rab/extensions/` | User extensions (phase 2 — WASM) |
| `~/.pi/agent/skills/` | `~/.rab/skills/` | Agent skills (phase 2) |
| `~/.pi/agent/themes/` | `~/.rab/themes/` | TUI themes (phase 2) |

### `settings.json` format

Same JSON schema as pi:

```json
{
    "model": "claude-sonnet-4-20250514",
    "thinking": "high",
    "models": ["claude-*", "gpt-4o"],
    "sessionDir": null,
    "noBuiltinTools": false,
    "tools": ["read", "write", "edit", "bash"],
    "excludeTools": [],
    "env": {
        "ANTHROPIC_API_KEY": "sk-ant-...",
        "OPENAI_API_KEY": "sk-..."
    },
    "theme": "dark",
    "verbose": false
}
```

Load order: global `~/.rab/settings.json` first, then project `.rab/settings.json`
overlays. CLI flags (`--model`, `--thinking`, `--no-tools`) take precedence
over both.

---

## CLI (`cli.rs`)

```
rab [OPTIONS] [MESSAGE]...

Modes:
  (default)        Print mode with piped stdin support
Session:
  -c, --continue   Continue most recent session in cwd
  --session PATH   Open specific session file
  --no-session     Ephemeral, don't save

Model:
  --model MODEL    Model name (provider auto-detected from name via adapter)
  --thinking LEVEL off|minimal|low|medium|high

Tools:
  --no-tools       Disable all tools (chat-only mode)

Context:
  -nc, --no-context-files   Skip AGENTS.md loading

Other:
  -h, --help
  -V, --version
```

Model auto-detection: `gpt*` → OpenAI, `claude*` → Anthropic, `gemini*` → Gemini,
fallback → Ollama.

---

## Run modes

### Print mode

```
$ rab -p "What does git status do?"
Shows the current state of the working directory and staging area...
```

```
$ cat README.md | rab -p "Summarize this"
This README describes a project that...
```

Streams the response to stdout. Thinking blocks shown dimmed. Tool calls and
results shown prefixed.

### Interactive mode

Same agent loop, different sink: `tui.rs` subscribes to the agent event
stream and renders to a ratatui TUI instead of stdout. Same crate — no
separate abstraction layer needed.

---

## TUI (`tui.rs`)

~100—200 lines. Thin glue between `AgentEvent` stream and ratatui rendering.
No abstraction layer on top of ratatui — ratatui **is** the abstraction.

| pi-tui (3,000+ lines) | ratatui + crossterm (library code) |
|---|---|
| `Component.render(width) → lines` + diff engine | `Widget::render(area, buf)` + `Frame` diff — built-in |
| `TUI` class: component tree, focus, overlays | 30 lines of `Layout::vertical` in `tui.rs` |
| `EditorComponent` (1,200+ lines) | `tui-textarea` — third-party widget, 3K+ ⭐ |
| Keyboard handling, keybindings | `crossterm::event::read()` — raw event loop |
| `Overlay` / `FocusManager` / `ComponentTree` | Not needed — ratatui renders widgets directly to layout areas |
| Kitty image protocol | `crossterm` raw escape sequences |

```rust
// tui.rs — the entire structure
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    messages: Vec<AgentMessage>,
    streaming_text: String,
    editor: Editor<'static>,       // tui-textarea
    model: String,
    session_id: String,
    usage: Usage,
}

impl Tui {
    pub async fn run(&mut self, events: Receiver<AgentEvent>) -> Result<()> {
        loop {
            self.terminal.draw(|f| self.render(f))?;

            tokio::select! {
                key = crossterm::event::read() => self.handle_key(key?),
                event = events.recv() => match event {
                    Some(AgentEvent::TextDelta { delta }) =>
                        self.streaming_text.push_str(&delta),
                    Some(AgentEvent::ToolCall { name, args }) =>
                        self.messages.push(...),
                    Some(AgentEvent::AgentEnd { .. }) => break,
                    None => break,
                }
            }
        }
        Ok(())
    }

    fn render(&self, f: &mut Frame) {
        let [hdr, msgs, ed, ftr] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ]).areas(f.area());
        f.render_widget(Header, hdr);
        f.render_widget(MessagesList(&self.messages), msgs);
        f.render_widget(&self.editor, ed);
        f.render_widget(Footer, ftr);
    }
}
```

Everything pi-tui had to build from scratch (component tree, diff rendering,
focus management, overlay stack) comes free with ratatui's `Frame` and
`Layout` system. `tui.rs` only needs to wire input events and agent events
to widget state.

```
┌─────────────────────────────────────────────────┐
│  rab  model: claude-sonnet-4  thinking:high │ ← Header
├─────────────────────────────────────────────────┤
│  User: list the .rs files                       │
│                                                 │
│  Assistant: Found 3 .rs files:                  │ ← Messages
│  • src/main.rs                                  │   (scrollable)
│  • src/lib.rs                                   │
│  • tests/integration.rs                         │
│                                                 │
│  ── tool: list_files ──────────────────────     │
│  src/main.rs                                    │   (collapsible)
│  src/lib.rs                                     │
│  tests/integration.rs                           │
│  ──────────────────────────────────────────     │
├─────────────────────────────────────────────────┤
│  > fix the bug in main.rs            █          │ ← Editor
├─────────────────────────────────────────────────┤
│  /tmp/project  session:abc123   ↑500 ↓300  $0.02│ ← Footer
└─────────────────────────────────────────────────┘
```

Components:
- **Messages widget** — scrollable chat history, collapsible tool output,
  thinking block folding
- **Editor widget** — multiline input via `tui-textarea`, `@` file completion,
  Tab path completion, `!command` / `!!command` detection
- **Header** — model name, thinking level, shortcut hints
- **Footer** — working directory, session ID, token usage, cost

---

## Phase 2

### User extensions (compile-time)

Same `Extension` trait used by builtins. To add a custom tool, implement the
trait and register it at startup:

```rust
struct MyTool;
impl Extension for MyTool {
    fn name(&self) -> &str { "my-tool" }
    fn tools(&self) -> vec![ /* ... */ ]
}

// main.rs — alongside builtins
if !args.no_extensions {
    exts.push(Box::new(MyTool));
}
```

### Dynamic plugin system (WASM via wasmtime)

The primary mechanism for user plugins. Plugins are `.wasm` components loaded
at runtime from `~/.rab/extensions/` (global) and `.rab/extensions/` (project).
WASM was chosen because: sandboxed by default (plugin crashes can't take down the
host), hot reload is trivial (recompile → replace file), and the WIT interface
is stable across Rust compiler versions (unlike C ABI for dylib). For plugins that
need C libraries (rare), a native dylib escape hatch can be added later.

#### Architecture

```
┌──────────────────────────────────────────────────────────┐
│                      rab host                             │
│                                                          │
│  ┌──────────────────────┐   ┌────────────────────────┐  │
│  │   BuiltinExtension   │   │   WasmExtension         │  │
│  │   (read/write/...)   │   │                         │  │
│  │   impl Extension     │   │   impl Extension {      │  │
│  │                      │   │     name()              │  │
│  │                      │   │     tools() → delegates │  │
│  │                      │   │       to wasm instance  │  │
│  │                      │   │   }                     │  │
│  └──────────────────────┘   └───────────┬────────────┘  │
│                                         │ WIT calls      │
│                               ┌─────────▼────────────┐  │
│                               │  PluginRegistry       │  │
│                               │  ├─ wasmtime Engine   │  │
│                               │  ├─ Vec<LoadedPlugin> │  │
│                               │  ├─ load(path)        │  │
│                               │  ├─ unload(name)      │  │
│                               │  └─ reload_all()      │  │
│                               └───────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

`WasmExtension` implements the host `Extension` trait by forwarding calls into
a wasm component via WIT bindings. The agent loop sees no difference between
a builtin and a wasm plugin.

#### WIT interface

```wit
package rab:plugin;

interface types {
    record tool-def {
        name: string,
        description: string,
        parameters: string,   // JSON Schema
        label: string,
    }

    record slash-command {
        name: string,
        description: string,
    }
}

world rab-plugin {
    export name: func() -> string;
    export tools: func() -> list<tool-def>;
    export commands: func() -> list<slash-command>;
    // tool-call-id and args-json come as strings; host owns serialization
    export execute-tool: func(
        tool-name: string,
        tool-call-id: string,
        args-json: string
    ) -> result<string, string>;
}
```

#### Plugin author experience

A `rab-plugin-sdk` crate provides a `Plugin` trait and an `export_plugin!` macro
that hides WIT internals. Plugin authors write plain Rust — no `extern "C"`,
no `#[repr(C)]`, no unsafe:

```rust
// my-plugin/src/lib.rs
use rab_plugin_sdk::*;

struct MyPlugin;

impl Plugin for MyPlugin {
    fn name() -> String { "my-tool".into() }

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "hello".into(),
            description: "Says hello".into(),
            parameters: json!({"type": "object", "properties": {}}),
            label: "Hello Tool".into(),
        }]
    }

    fn execute(tool: &str, call_id: &str, args: Value) -> Result<String, String> {
        match tool {
            "hello" => Ok(format!("Hello from plugin! call_id={call_id}")),
            _ => Err(format!("Unknown tool: {tool}")),
        }
    }
}

export_plugin!(MyPlugin);
```

Minimal `Cargo.toml`:

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
rab-plugin-sdk = "0.1"
serde_json = "1"
```

Build: `cargo build --target wasm32-wasip2 --release`, copy the `.wasm` to
`~/.rab/extensions/`. The host picks it up on next startup (and on `/reload`
in TUI mode).

Scaffolding: `rab plugin new my-plugin` generates the Cargo.toml, lib.rs, and
WIT file automatically.

#### Plugin lifecycle

| Operation | Behaviour |
|---|---|
| **Load** | `PluginRegistry::load(path)` compiles `.wasm` → native via cranelift, instantiates the component, calls `name()` and `tools()` for metadata |
| **Unload** | Drop the `WasmExtension` + `Store` + `Instance`. Wasmtime releases all guest memory |
| **Hot reload** | File watcher (`notify` crate) detects change → unload old → load new. `/reload-plugins` slash command for manual trigger |
| **Validation** | On load, call `self-test` export (optional). If it returns `err`, print warning, skip plugin |
| **Error isolation** | If tool execution panics inside wasm, wasmtime catches it as a `Trap`. `WasmExtension` returns `ToolResult::error`. Host process unaffected |
| **Resource limits** | `Store::set_fuel(1_000_000)` ~covers most tool logic. Configurable per-plugin in `.rab/extensions.toml` (future) |

#### Directory layout

```
~/.rab/extensions/           ← global plugins (available in every project)
├── hello.wasm
├── code-reviewer/
│   ├── code-reviewer.wasm
│   └── config.toml          ← plugin metadata, resource limits (future)
└── sources/                 ← optional: keep source alongside

./.rab/extensions/           ← project-local plugins
└── project-tool.wasm
```

#### Future: native dylib escape hatch

For plugins that need C libraries (e.g. `libgit2`, `tree-sitter`), a parallel
native path via `crate-type = ["cdylib"]` + `dlopen2`. Same `Plugin` trait,
different compile target. Requires matching Rust compiler version between host
and plugin — documented as a power-user feature, not the default path.

### Skills

Skills are markdown files following the [Agent Skills standard](https://agentskills.io).
Loaded from `~/.rab/skills/` and `.rab/skills/`. When the model's
request matches a skill's trigger, the skill instructions are injected into the
system prompt.

### pi-mcp-adapter

An `Extension` that connects to MCP (Model Context Protocol) servers and
exposes their tools to the agent. Uses the `rmcp` crate for client-side MCP
protocol (stdio + SSE transports). Each connected MCP server's tools appear
as regular `AgentTool` instances, indistinguishable from builtins.

```rust
struct McpAdapter;
impl Extension for McpAdapter {
    fn name(&self) -> &str { "mcp-adapter" }
    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        // Discover and wrap MCP server tools
        self.mcp_clients.iter()
            .flat_map(|client| client.list_tools())
            .map(|tool| Box::new(McpToolWrapper::new(tool)))
            .collect()
    }
}
```

Configured via `.rab/mcp.json`:

```json
{
    "servers": [
        { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] },
        { "url": "https://mcp.example.com/sse" }
    ]
}
```

This mirrors what pi users do with MCP extensions, but as a first-party
built-in Phase 2 extension.

---

## Open questions

- **Dynamic extension loading** — Resolved: WASM via wasmtime + WIT
  Component Model. See §Dynamic plugin system.
- **OAuth** — genai's `AuthResolver` supports dynamic tokens, but browser
  login flow, token refresh, and credential storage need a separate crate
  (e.g. `oauth2`). MVP uses env API keys only.
- **Image paste in TUI** — clipboard integration differs per platform (wl-paste,
  pbpaste, PowerShell). Kitty protocol covers display; input is TBD.
- **Command deny-list** — bash tool currently runs anything. A deny-list or
  sandbox (bubblewrap, landlock) should be configurable per project.
- **Multi-model cycling** — Ctrl+P model switching like pi requires a model
  registry. genai auto-detects from name prefix; a full registry with metadata
  (context window, costs) is future work.
- **Provider fallback** — if the primary provider fails, should rab retry
  with another? pi doesn't do this; worth considering.

---

## Dependency tree

```
rab (EPL-2.0)
├── genai 0.6              (Apache 2.0) — isolated: only adapter/genai.rs imports this
├── clap 4                 (MIT)        — CLI parsing
├── tokio 1                (MIT)        — async runtime
├── serde + serde_json 1   (MIT)        — JSON serialization
├── uuid 1                 (MIT)        — message/session IDs
├── chrono 0.4             (MIT)        — timestamps
├── directories 5          (MIT)        — XDG paths
├── anyhow 1               (MIT)        — error handling
├── futures 0.3            (MIT)        — StreamExt
├── async-trait 0.1        (MIT)        — trait async fn
├── colored 2              (MPL-2.0)    — terminal colors
├── tracing 0.1            (MIT)        — structured logging
├── ratatui 0.29           (MIT)        — TUI framework
├── crossterm 0.28         (MIT)        — terminal backend
├── tui-textarea 0.7       (MIT)        — multiline editor
├── wasmtime 26+           (Apache 2.0) — WASM runtime for dynamic plugins (phase 2)
├── notify 7               (CC0-1.0)    — file watcher for plugin hot reload (phase 2)
└── rmcp 1                 (MIT)        — MCP client for pi-mcp-adapter (phase 2)
```

No GPL dependencies. All are permissive (MIT / Apache 2.0 / MPL-2.0 / CC0-1.0), fully
compatible with EPL-2.0. genai is the only external provider dependency and
is swappable via the `Provider` trait — replace or remove it without touching
core logic.

Phase 2 dependencies (wasmtime, notify, rmcp)
are gated behind Cargo features: `plugins` and `mcp`. MVP compiles
without them.
