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
| `skills.rs` (Phase 2) | `packages/coding-agent/src/core/skills.ts` |

---

## PoC Phase

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

Settings live at `~/.rab/settings.json` (global) with project overrides from
`.rab/settings.json`, matching pi's schema:

```json
{
    "model": "deepseek-v4-flash",
    "thinking": "off",
    "env": {
        "OPENCODE_API_KEY": "oc_..."
    },
    "providers": {
        "opencode-go": {
            "base_url": "https://opencode.ai/zen/go/v1",
            "api_key": "oc_..."
        }
    }
}
```

### Tasks

- [ ] **Project scaffold** — `cargo init`, Cargo.toml with PoC dependencies
- [ ] **`types.rs`** — `AgentMessage`, `Role`, `ToolCall`, `Usage`, no session tree (MVP: linear, no `parentId`)
- [ ] **`provider.rs`** — `Provider` trait + `StreamEvent` enum + `StopReason` enum (per arch.md)
- [ ] **`adapter/genai.rs`** — `GenaiProvider` wrapping `genai::Client`, implements `Provider`
  - Takes `Settings` struct at construction (api key, base url, model)
  - Uses genai's OpenAI adapter internally (OpenCode Go is OpenAI-compatible)
  - Translates `AgentMessage` → genai chat messages, `StreamEvent` ← genai stream events
  - Model IDs passed through: `deepseek-v4-flash`, `deepseek-v4-pro`
  - The only file that imports genai — per arch.md isolation behind `Provider` trait
- [ ] **`extension.rs`** — `Extension` trait, `AgentTool` trait, `SlashCommand` struct, `BlockReason`
- [ ] **`builtin/read.rs`** — Read tool extension (reads files, line numbers, 50KB truncation)
- [ ] **`builtin/write.rs`** — Write tool extension (temp file + atomic rename, creates parent dirs)
- [ ] **`builtin/edit.rs`** — Edit tool extension (exact-match search/replace, errors on 0 or >1 matches)
- [ ] **`builtin/bash.rs`** — Bash tool extension (runs `sh -c <command>`, timeout, truncation)
- [ ] **`agent.rs`** — `run_agent_loop()` per arch.md pseudocode:
  - System prompt (hardcoded base prompt, no AGENTS.md loading)
  - Default model: `deepseek-v4-flash` (fast/cheap); `deepseek-v4-pro` selectable via arg
  - Steering queue and follow-up queue (stubbed — no runtime injection yet)
  - Inner loop: stream LLM → execute tools → repeat
  - Outer loop: handle follow-up
  - Tool execution: parallel by default
  - `CancellationToken` support (stubbed)
  - Event emission via `EventSink`
- [ ] **`settings.rs`** — Load settings from `~/.rab/settings.json` + `.rab/settings.json` overlay
  - Same JSON schema as pi: `model`, `thinking`, `env`, `providers`
  - Load order: global first, then project-local merges on top
  - No env var fallback — all config comes from files
  - Used by `adapter/genai.rs` for api key + base url, by `agent.rs` for model selection
- [ ] **`main.rs`** — Minimal CLI: `rab "message"` → loads settings, runs agent loop in print mode
  - `--model deepseek-v4-pro` overrides settings file
  - No clap yet — just `std::env::args()`
  - Errors if settings file missing or api key not configured
- [ ] **Integration smoke test** — `rab "list .rs files in src/"` produces correct output with tool calls

### Dependencies

```
tokio, serde, serde_json, uuid, chrono, anyhow, futures, async-trait, colored, genai
```

genai wraps the HTTP layer and streaming. No reqwest needed at this stage.

### Deliverable

A binary that reads provider/model config from `~/.rab/settings.json`, connects
to OpenCode Go (DeepSeek V4 Flash by default), runs the agent loop with tool
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
- [ ] **Hook pipeline** — Extension hooks wired into agent loop:
  - `before_tool_call` — all extensions consulted, first block wins
  - `after_tool_call` — result patching
  - `CancellationToken` passed to every hook
- [ ] **Steering / follow-up queues** — Runtime message injection:
  - Steering: injected after current turn's tool calls finish (mid-run user input)
  - Follow-up: injected after agent would stop (post-run follow-up)
  - Drain modes: `one-at-a-time` and `all`
- [ ] **Tool execution modes** — Parallel (default) vs sequential
  - `AgentTool::execution_mode` override
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
