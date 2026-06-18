# rab — Implementation Plan

Reference implementation: `~/src/cvstree/pi-mono/` (TypeScript, same architecture).
Study these files before implementing each Rust equivalent.

## Pi source reference map

| rab module | pi source (study before implementing) |
|---|---|
| `types.rs` | `packages/agent/src/types.ts`, `packages/coding-agent/src/core/extensions/types.ts` |
| `provider.rs` | `packages/ai/src/types.ts`, `packages/ai/src/providers/openai-completions.ts` |
| `adapter/genai.rs` | pi has no genai; rab uses genai crate for HTTP+streaming. Study `openai-completions.ts` for the OpenAI chat completions protocol that OpenCode Go uses |
| `extension.rs` | `packages/agent/src/types.ts` (`AgentTool`, `AgentContext`, `AgentEvent`) |
| `builtin/read.rs` | `packages/coding-agent/src/core/tools/read.ts` |
| `builtin/write.rs` | `packages/coding-agent/src/core/tools/write.ts` |
| `builtin/edit.rs` | `packages/coding-agent/src/core/tools/edit.ts`, `edit-diff.ts` |
| `builtin/bash.rs` | `packages/coding-agent/src/core/tools/bash.ts`, `packages/coding-agent/src/utils/shell.ts` |
| `agent.rs` | `packages/agent/src/agent-loop.ts` (the canonical loop) |
| `session.rs` (Phase 1) | `packages/agent/src/harness/session/`, `packages/coding-agent/src/core/session-manager.ts` |
| `compaction.rs` (Phase 1) | `packages/agent/src/harness/compaction/compaction.ts`, `packages/coding-agent/src/core/compaction/` |
| `settings.rs` (Phase 1) | `packages/coding-agent/src/core/settings-manager.ts` |
| `system_prompt.rs` (Phase 1) | `packages/coding-agent/src/core/system-prompt.ts` |
| `commands.rs` (Phase 1) | `packages/coding-agent/src/core/slash-commands.ts` |
| `tui.rs` (Phase 1) | `packages/coding-agent/src/modes/interactive/` (ink-based TUI, not ratatui — different rendering model) |
| `settings.rs` | `packages/coding-agent/src/core/settings-manager.ts` |
| `skills.rs` (Phase 2) | `packages/coding-agent/src/core/skills.ts` |

---

## PoC Phase ✅

**Goal:** End-to-end agent loop via [OpenCode Go](https://opencode.ai/docs/go/) with
DeepSeek V4 Flash and Pro models plus four built-in tools. Uses settings files
(same schema as pi) for provider and model configuration. Everything else is
in-memory, no persistence.

### Provider: OpenCode Go

OpenCode Go is a subscription service ($10/month) providing an OpenAI-compatible
API at `https://opencode.ai/zen/go/v1`. Models:

| Model | ID | Reasoning |
|---|---|---|
| DeepSeek V4 Flash | `deepseek-v4-flash` | `off`, `high`, `max` |
| DeepSeek V4 Pro | `deepseek-v4-pro` | `off`, `high`, `max` |

Both models use the `/chat/completions` endpoint with standard OpenAI
request/response format. Auth is `Authorization: Bearer <api_key>`.
API key and base URL come from settings files, not environment variables.

`~/.rab/agent/settings.json`:
```json
{
    "defaultModel": "deepseek-v4-flash",
    "defaultThinkingLevel": "max",
    "defaultProvider": "opencode-go"
}
```

`~/.rab/agent/auth.json`:
```json
{
    "opencode-go": {
        "type": "api_key",
        "key": "oc_..."
    }
}
```

### Dependencies

```
tokio, serde, serde_json, uuid, chrono, anyhow, futures, async-trait, colored, genai, directories, async-stream
```

### Deliverable

A binary that reads provider/model config from `~/.rab/agent/settings.json` and
`~/.rab/agent/auth.json`, connects to OpenCode Go, runs the agent loop with tool
calling, and prints the result. No session files, no TUI, no env vars.

---

## Phase 1

**Goal:** Full-featured coding agent with TUI, sessions, settings, compaction.
Everything in arch.md that isn't explicitly Phase 2.

### Tasks

- [ ] **`adapter/genai.rs`** — Extend PoC's `GenaiProvider` to support multiple backends:
  - OpenCode Go (existing from PoC)
  - Anthropic, OpenAI, Google, DeepSeek (direct), Ollama
  - Provider auto-detection from model name prefix (`claude*`, `gpt*`, `gemini*`)
  - `rab connect` command for interactive provider setup (like pi's `/connect`)
- [ ] **`cli.rs`** — clap-based CLI with all flags and subcommands:
  - `[MESSAGE]...` positional (prompt)
  - `-c, --continue`, `--session PATH`, `--no-session`
  - `--model MODEL`, `--thinking LEVEL`
  - `--no-tools`, `--no-builtin-tools`, `--no-extensions`
  - `-nc, --no-context-files`
  - `-V, --version`, `-h, --help`
  - Mode dispatch: print mode (default) vs interactive mode (`-i` / TUI)
- [ ] **`settings.rs`** — Extend PoC settings with full pi schema:
  - Thinking level, tools allow/deny lists, theme, models list
  - `~/.rab/models.json` for custom provider/model definitions
  - CLI flags override settings file values
- [ ] **`system_prompt.rs`** — Build system prompt from:
  - Base prompt (hardcoded tool descriptions, response format)
  - `~/.rab/AGENTS.md` (global context)
  - `AGENTS.md` / `CLAUDE.md` walked up from cwd (project context)
  - Wrapped in `<project_context>` tags
  - Respect `APPEND_SYSTEM.md` / `SYSTEM.md` (full override)
  - `--no-context-files` flag
- [ ] **`session.rs`** — `SessionManager` with JSONL storage:
  - Create new session, continue recent, open by path
  - Append `AgentMessage` entries
  - Walk from root along active branch (tree with `parentId`)
  - `~/.rab/sessions/<cwd-hash>/` directory structure
- [ ] **`compaction.rs`** — Context window compaction:
  - Token estimation via model-specific heuristic or tiktoken-rs
  - Cut point finder (accumulate from oldest, preserve tail)
  - Summary generation (fast model prompt)
  - Replace old messages with synthetic summary message
  - Auto-trigger before context overflow; manual trigger via `/compact`
- [ ] **`commands.rs`** — Core slash commands:
  - `/model <name>` — switch active model
  - `/thinking <level>` — set thinking level (off/minimal/low/medium/high)
  - `/compact [prompt]` — manual compaction
  - `/session` — print session info
  - `/name <text>` — set session display name
  - `/fork` — fork session from previous user message
  - `/clone` — duplicate active branch into new session
  - `/resume` — list previous sessions in cwd
  - `/new` — start fresh session
  - `/copy` — copy last assistant message to clipboard
  - `/export [path]` — export session to HTML
  - `/settings` — print or edit settings
  - `/reload` — reload AGENTS.md, skills, settings
  - `/quit` — exit (interactive mode)
- [ ] **`tui.rs`** — Terminal UI with ratatui + tui-textarea + crossterm:
  - Header: model name, thinking level
  - Messages widget: scrollable chat history, collapsible tool output, thinking block folding
  - Editor widget: multiline input, `@` file completion, Tab path completion, `!command` detection
  - Footer: working directory, session ID, token usage, cost
  - Subscribes to `AgentEvent` stream from agent loop
  - Keyboard handling via crossterm event loop
- [ ] **Hook pipeline** — Extend PoC hooks with `AgentContext` parameter and `CancellationToken`:
  - `before_tool_call` — all extensions consulted, first block wins
  - `after_tool_call` — result patching
  - `CancellationToken` passed to every hook
- [ ] **Steering / follow-up queues** — Runtime message injection:
  - Steering: injected after current turn's tool calls finish (mid-run user input)
  - Follow-up: injected after agent would stop (post-run follow-up)
  - Drain modes: `one-at-a-time` and `all`
- [ ] **Tool execution modes** — `AgentTool::execution_mode` override (PoC has parallel only)
  - Sequential: execute one tool, feed result before starting next
- [ ] **Compile-time user extensions** — `Extension` trait impls registered at startup
  - `--no-extensions` flag
- [ ] **`~/.rab/models.json`** — Custom provider/model definitions
- [ ] **Error handling** — No unwraps in core loop, graceful degradation, tracing instrumentation
- [ ] **Image support** — Read tool detects image files, reads as base64, passes via multimodal payload
- [ ] **Bash security** — Command deny-list (MVP: basic blocklist)
- [ ] **`rab plugin new`** — Scaffold a compile-time extension crate (simple Cargo.toml + lib.rs)

### Dependencies

```
(all PoC deps) + clap, directories, tracing, tiktoken-rs (optional), ratatui, crossterm, tui-textarea
```

### Deliverable

Full `rab` binary with print mode + interactive TUI mode, persistent sessions,
context compaction, settings, slash commands, and custom compile-time extensions.

---

## Phase 2

**Goal:** Dynamic plugin system (WASM), skills, MCP adapter. Complete app per arch.md.

### Tasks

- [ ] **`rab-plugin-sdk` crate** — Plugin SDK with `Plugin` trait and `export_plugin!` macro
  - Hides WIT internals from plugin authors
  - `ToolDef`, `SlashCommand` structs mirror host types
  - JSON serialization for args/results over WASM boundary
- [ ] **WIT interface** — `wit/plugin.wit` defining the plugin contract
  - `name`, `tools`, `commands`, `execute-tool` exports
  - `result<string, string>` return type
- [ ] **`plugin.rs` / `PluginRegistry`** — wasmtime host integration:
  - Load `.wasm` components from `~/.rab/extensions/` and `.rab/extensions/`
  - Instantiate with engine, store, linker
  - `WasmExtension` wraps an instance, implements `Extension` trait
  - Unload: drop store + instance, release all guest memory
  - Error isolation: wasm panics caught as `Trap`, returned as `ToolResult::error`
  - Resource limits: fuel metering per plugin
- [ ] **Hot reload** — `notify` crate file watcher:
  - Watch extension directories
  - On change: unload old, load new
  - `/reload-plugins` slash command for manual trigger
  - Zero-downtime swap (load new before unloading old)
- [ ] **`rab plugin new`** — Scaffold a WASM plugin:
  - `Cargo.toml` with `crate-type = ["cdylib"]`, `wasm32-wasip2` target
  - `src/lib.rs` with `Plugin` impl stub
  - `wit/plugin.wit`
  - `rab plugin new --native` for dylib escape hatch scaffold
- [ ] **Skills system** — Agent Skills standard support:
  - Load `.md` files from `~/.rab/skills/` and `.rab/skills/`
  - Match request against skill triggers
  - Inject matched skill instructions into system prompt
- [ ] **pi-mcp-adapter** — MCP client extension:
  - `rmcp` crate for stdio + SSE transport
  - Configured via `.rab/mcp.json`
  - Server tools exposed as `AgentTool` instances
  - Tool discovery on connect, re-discovery on reconnect
- [ ] **Keybindings customization** — `~/.rab/keybindings.json`
  - User-overridable key maps for TUI
- [ ] **Themes** — `~/.rab/themes/`
  - ratatui color themes, loaded from JSON
- [ ] **Bash sandboxing** — Configurable per-project sandbox:
  - bubblewrap / landlock support
  - Configured in `.rab/settings.json`
- [ ] **Multi-model cycling** — Ctrl+P model switching with registry
  - Model metadata: context window, costs, capabilities
- [ ] **Provider fallback** — Retry with alternate provider on failure
- [ ] **OAuth flow** — Browser-based login for providers that need it
  - Token storage in keyring or encrypted file
  - Refresh token support
- [ ] **Image paste in TUI** — Clipboard image → base64 → multimodal payload
  - Platform-specific clipboard access (wl-paste, pbpaste, PowerShell)

### Dependencies

```
(all Phase 1 deps) + wasmtime, notify, rmcp, dlopen2 (for native plugin escape hatch)
```

### Deliverable

Complete rab with dynamic plugin system, skills, MCP integration, and all
polish features. Plugin authors run `rab plugin new hello`, implement a trait,
run `cargo build --target wasm32-wasip2`, and drop the `.wasm` into
`~/.rab/extensions/` — hot reload picks it up automatically.

---

## Implemented

### PoC

- [x] **Project scaffold** — `cargo init`, Cargo.toml with PoC dependencies
- [x] **`types.rs`** — `AgentMessage`, `Role`, `ToolCall`, `Usage`, serde camelCase
- [x] **`provider.rs`** — `Provider` trait + `StreamEvent` enum + `StopReason` enum
- [x] **`adapter/genai.rs`** — `GenaiProvider` wrapping `genai::Client`, implements `Provider`
  - OpenCode Go via `opencode_go::` namespace with `AuthResolver` (no env vars)
  - Reasoning effort from settings mapped to genai `ReasoningEffort`
  - Proper `ToolResponse` for round-tripping tool results
- [x] **`extension.rs`** — `Extension` trait, `AgentTool` trait, `CommandHandler` trait, `CommandResult`, `SlashCommand`, `BlockReason`
- [x] **`builtin/read.rs`** — Read tool (offset, limit, line numbers, 50KB/2000-line truncation)
- [x] **`builtin/write.rs`** — Write tool (parent dirs, temp file + atomic rename)
- [x] **`builtin/edit.rs`** — Edit tool (multi-edit, uniqueness check, overlap detection, camelCase args)
- [x] **`builtin/bash.rs`** — Bash tool (sh -c, timeout, stdout+stderr, truncation)
- [x] **`agent.rs`** — `run_agent_loop()` with inner loop, streaming, parallel tool execution, hook pipeline, `AgentEvent` emission
- [x] **`main.rs`** — Minimal CLI: `rab [--model <m>] <message>`, print-mode emitter, loads command extensions, git branch detection
- [x] **`builtin/commands.rs`** — Built-in commands extension: `/quit`, `/model` with argument completions
- [x] **`settings.rs`** — Load `~/.rab/agent/settings.json` + `.rab/settings.json` overlay, pi schema, camelCase
- [x] **`auth.rs`** — Load `~/.rab/agent/auth.json`, pi format (`{"provider": {"type": "api_key", "key": "..."}}`)
- [x] **`lib.rs`** — Crate root exposing all modules for integration tests
- [x] **Tests** — 45 integration tests: types (6), settings (6), auth (4), read (4), write (3), edit (6), bash (6)

### Phase 1 (partial)

- [x] **`tui.rs`** — Terminal UI with ratatui + crossterm:
  - Pi-style layout: messages → working indicator → editor → footer
  - Messages widget: scrollable chat, pi dark theme colors, tool output collapsed by default, thinking block folding
  - Editor: custom multiline editor inlined in tui.rs, rendered with ratatui Paragraph, hardware cursor via Frame
  - Footer: 2-line pi-style (cwd + git branch, tokens left + model right with thinking level)
  - Keyboard: Enter submit, Shift+Enter/Alt+Enter/Ctrl+Enter/Ctrl+J newline, Ctrl+C abort+clear, Ctrl+D quit (empty), Esc abort (streaming), Tab complete, arrow history
  - Slash command autocomplete: Tab completes in-place, Enter executes prefix match
  - Working indicator: animated braille spinner above editor during streaming
  - Agent abort: Esc/Ctrl+C during streaming aborts the tokio task
  - Emacs navigation: Ctrl+A/E/B/F/P/N for line start/end/left/right/up/down
  - Emacs editing: Ctrl+K/U/W for kill/delete operations
  - Pi-style keybindings: Esc→interrupt (no clear), Ctrl+C→clear+abort, Ctrl+D→quit
  - Model selector overlay: Ctrl+L opens centered overlay with search, filtering, arrow navigation, Enter to select
  - Thinking toggle Ctrl+T persisted to settings.json (`hideThinkingBlock`), shows status message
  - Tool output toggle Ctrl+O persisted to settings.json (`collapseToolOutput`), shows status message
- [x] **Model persistence** — Model changes via selector or `/model` command save `defaultModel` to `~/.rab/agent/settings.json`
- [x] **Unified command system** — Commands use the same `Extension` trait as tools:
  - `CommandHandler` trait with `execute()` and `argument_completions()`
  - `CommandResult` enum (Info, Quit, ModelChanged, ShowHelp, Reloaded, NewSession)
  - `/quit`, `/model`, `/hotkeys`, `/reload`, `/new` via `CommandsExtension` (built-in)
  - Exact match first, then prefix match (e.g. `/q` → `/quit`)
  - "Did you mean" suggestions for ambiguous prefixes
  - `/model` and `/m` without args open the model selector overlay (pi-style)
- [x] **`theme.rs`** — Theme struct with pi's exact dark theme colors:
  - Chat styles: user_msg, tool_pending/success/error, thinking, dim, accent
  - Footer and editor styles
  - Style helper methods, ready for future theming support
- [x] **Arrow-key history** — ↑↓ recalls previous user messages when editor is empty
- [x] **`auth.rs`** — Supports both `api_key` and `oauth` credential types (pi-compatible)
- [x] **`settings.rs`** — Load + save `~/.rab/agent/settings.json`, pi keys (`hideThinkingBlock`, `collapseToolOutput`), `save_to()` for testing, `Option<bool>` for proper merge semantics
- [x] **`~/.rab/agent/settings.json`** — Global config: provider, model, thinking level, theme, thinking/tool-output toggles
- [x] **`~/.rab/agent/auth.json`** — Provider credentials (copied from pi)
- [x] **`Cargo.toml`** — Switched to `native-tls` (rustls-platform-verifier panics on Termux/Android)
- [x] **`.cargo/config.toml`** — OPENSSL_DIR for Termux build without pkg-config
- [x] **Tests** — 130 total: auth (4), settings (24), commands (18), model selector (30), editor behavior (19), tools (19), types (6)

## Known Issues

### Editor cursor
- Cursor display is buggy on empty editor (sometimes not visible until typing starts)
- Cursor positioning may be off on lines with multi-byte Unicode characters (byte vs char index)
- No visual cursor-line highlight (pi uses subtle background on the cursor row)

### Editor shortcuts missing
- Alt+Arrow (word movement) — not implemented
- Ctrl+Left/Right (jump word) — not implemented
- No kill ring (yank/pop) — not implemented

### TUI colors and styles
- Assistant markdown text not styled with pi's markdown theme colors (headings, code, links, quotes)
- Tool call lines missing bold tool name, only one uniform style
- Tool result background colors not matching pi's exact shades
- Thinking blocks use same dim color for all lines, pi has per-line indentation
- User messages not rendering as markdown (pi renders user text + skills as markdown)
- No visual distinction between streaming/pending text and final text
- Status dot colors reversed (green when idle, accent when streaming, pi uses dim circle when idle)
- Footer tokens not padded/right-aligned properly on narrow terminals

## TODO

### Slash command autocomplete selector
- Replace current inline Tab-completion with a proper autocomplete dropdown
  - Show command list when typing `/` (like pi)
  - Show argument completions when typing `/cmd ` (like pi)
  - Allow Tab/Enter to select, Esc to dismiss
  - Theme-aware styling matching pi's autocomplete dropdown
