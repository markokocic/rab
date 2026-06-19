# rab — Implementation Plan

Reference implementation: `~/src/cvstree/pi/` (TypeScript, same architecture).
Study these files before implementing each Rust equivalent.

## Pi source reference map

| rab module | pi source (study before implementing) |
|---|---|
| `agent/types.rs` ✅ | `packages/agent/src/types.ts`, `packages/coding-agent/src/core/extensions/types.ts` |
| `agent/provider.rs` ✅ | `packages/ai/src/types.ts`, `packages/ai/src/providers/openai-completions.ts` |
| `adapter/genai.rs` | pi has no genai; rab uses genai crate for HTTP+streaming. Study `openai-completions.ts` for the OpenAI chat completions protocol that OpenCode Go uses |
| `agent/extension.rs` ✅ | `packages/agent/src/types.ts` (`AgentTool`, `AgentContext`, `AgentEvent`) |
| `tui/components/editor.rs` ✅ | `packages/tui/src/components/editor.ts` (full port), `packages/tui/src/autocomplete.ts` |
| `tui/components/input.rs` ✅ | `packages/tui/src/components/input.ts` |
| `tui/components/settings_list.rs` ✅ | `packages/tui/src/components/settings-list.ts` |
| `tui/components/select_list.rs` ✅ | `packages/tui/src/components/select-list.ts` |
| `tui/screen.rs` ✅ | `packages/tui/src/tui.ts` (doRender diff engine) |
| `tui/terminal.rs` ✅ | `packages/tui/src/terminal.ts` |
| `tui/keys.rs` ✅ | `packages/tui/src/keys.ts` |
| `tui/util.rs` ✅ | `packages/tui/src/utils.ts` |
| `tui/fuzzy.rs` ✅ | `packages/tui/src/fuzzy.ts` |
| `builtin/read.rs` | `packages/coding-agent/src/core/tools/read.ts` |
| `builtin/write.rs` | `packages/coding-agent/src/core/tools/write.ts` |
| `builtin/edit.rs` | `packages/coding-agent/src/core/tools/edit.ts`, `edit-diff.ts` |
| `builtin/bash.rs` | `packages/coding-agent/src/core/tools/bash.ts`, `packages/coding-agent/src/utils/shell.ts` |
| `agent/types.rs` ✅ | `packages/agent/src/types.ts` (`AgentMessage`, `Role`, `ToolCall`, `Usage`) |
| `agent/provider.rs` ✅ | `packages/ai/src/types.ts`, `packages/ai/src/providers/openai-completions.ts` |
| `agent/loop.rs` ✅ | `packages/agent/src/agent-loop.ts` (the canonical loop) |
| `agent/session.rs` | `packages/agent/src/harness/session/`, `packages/coding-agent/src/core/session-manager.ts` |
| `compaction.rs` | `packages/agent/src/harness/compaction/compaction.ts`, `packages/coding-agent/src/core/compaction/` |
| `agent/settings.rs` | `packages/coding-agent/src/core/settings-manager.ts` |
| `system_prompt.rs` | `packages/coding-agent/src/core/system-prompt.ts` |
| `commands.rs` | `packages/coding-agent/src/core/slash-commands.ts` |
| `agent/ui/` ✅ | `packages/coding-agent/src/modes/interactive/` (app-specific UI components) |
| `adapter.rs` | pi has no genai; rab uses genai crate for HTTP+streaming |
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
- [x] **`cli.rs`** — CLI with all flags and subcommands (partial — hand-rolled parser):
  - `[MESSAGE]...` positional (prompt) ✅
  - `-c, --continue`, `--session PATH`, `--no-session`, `--name <name>`, `--session-dir <dir>` ✅
  - `--model MODEL` ✅
  - `--thinking LEVEL` ❌
  - `--no-tools`, `--no-builtin-tools`, `--no-extensions` ❌
  - `-nc, --no-context-files` ❌
  - `-V, --version`, `-h, --help` ❌
  - Mode dispatch: print mode (default) vs interactive mode (TUI) ✅
- [x] **`settings.rs`** — Extend PoC settings with full pi schema:
  - Thinking level, tools allow/deny lists, theme ✅
  - `~/.rab/models.json` for custom provider/model definitions ❌
  - CLI flags override settings file values ✅ (partial — --model only)
- [ ] **`system_prompt.rs`** — Build system prompt from:
  - Base prompt (hardcoded tool descriptions, response format)
  - `~/.rab/AGENTS.md` (global context)
  - `AGENTS.md` / `CLAUDE.md` walked up from cwd (project context)
  - Wrapped in `<project_context>` tags
  - Respect `APPEND_SYSTEM.md` / `SYSTEM.md` (full override)
  - `--no-context-files` flag
- [x] **`session.rs`** — `SessionManager` with JSONL storage:
  - Create new session, continue recent, open by path ✅
  - Append `AgentMessage` entries ✅
  - Walk from root along active branch (tree with `parentId`) ✅
  - `~/.rab/sessions/<cwd-hash>/` directory structure ✅
  - Corruption handling (malformed lines, empty files, missing headers) ✅
  - All 10 pi entry types ✅
  - Deferred flush (no file until first assistant message) ✅
  - 66 unit tests
- [ ] **`compaction.rs`** — Context window compaction:
  - Token estimation via model-specific heuristic or tiktoken-rs
  - Cut point finder (accumulate from oldest, preserve tail)
  - Summary generation (fast model prompt)
  - Replace old messages with synthetic summary message
  - Auto-trigger before context overflow; manual trigger via `/compact`
- [x] **`commands.rs`** — Core slash commands (partial):
  - `/model <name>` — switch active model ✅
  - `/thinking <level>` — set thinking level ❌
  - `/compact [prompt]` — manual compaction ❌
  - `/session` — print session info ✅
  - `/name <text>` — set session display name ✅
  - `/fork` — fork session from previous user message ❌
  - `/clone` — duplicate active branch into new session ❌
  - `/resume` — list previous sessions in cwd ✅ (returns OpenSessionSelector; UI not built)
  - `/new` — start fresh session ✅
  - `/copy` — copy last assistant message to clipboard ❌
  - `/export [path]` — export session to HTML ❌
  - `/settings` — print or edit settings ❌
  - `/reload` — reload AGENTS.md, skills, settings ✅
  - `/quit` — exit (interactive mode) ✅
- [x] **`editor.rs`** — Custom editor widget (extracted from tui.rs):
  - Multi-line text editing with Emacs-style keybindings ✅
  - Grapheme-aware cursor (unicode-segmentation) ✅
  - Proper word wrapping with CJK break rules ✅
  - Undo stack (Ctrl+_) with fish-style word coalescing ✅
  - Kill ring (Ctrl+K/U/W kill, Ctrl+Y yank, Alt+Y yank-pop) ✅
  - Word movement (Alt+←→, Ctrl+←→) and word deletion (Alt+Backspace/Del) ✅
  - Pi-style paste: normalizes line endings, expands tabs, filters control chars,
    smart space before file paths, large paste compression (`[paste #N +L lines]`) ✅
  - Prompt history with up/down arrow recall (oldest-first storage, draft restoration) ✅
  - `render_with_max()` for fixed-height viewport with internal scrolling ✅
- [x] **Editor autocomplete system** — Pi-style slash command and file path autocomplete ✅:
  - Slash command completion with fuzzy matching (all chars in order, case-insensitive) ✅
  - Auto-accept single match on Tab (pi: explicitTab + single item) ✅
  - Argument completions bridged from `CommandHandler::argument_completions()` ✅
  - `@` file path completion with directory listing ✅
  - Tab file path completion without `@` prefix ✅
  - Arrow key navigation with wrap-around, Enter/Tab to accept, Esc to dismiss ✅
  - Dropdown renders below editor block border (pi-style), height auto-adjusts ✅
  - SelectList-style centered scroll window, max visible 5, column layout ✅
  - Theme styling: selected accent+bold `→`, normal muted, descriptions in column ✅
- [x] **`tui.rs`** — Terminal UI with ratatui + crossterm:
  - Pi-style layout: messages → working indicator → editor → footer ✅
  - Messages widget: scrollable chat, pi dark theme colors, tool output collapsed by default ✅
  - Working indicator: animated braille spinner above editor during streaming ✅
  - Footer: 2-line pi-style (cwd + git branch, tokens left + model right) ✅
  - Model selector overlay: Ctrl+L, search, filtering, arrow nav, Enter to select ✅
  - Thinking toggle Ctrl+T persisted to settings.json ✅
  - Tool output toggle Ctrl+O persisted to settings.json ✅
  - `!`/`!!` bash inline execution with abort support ✅
  - Pi-style paste detection: 20ms timing heuristic avoids auto-submit ✅
  - **↓ BEING REPLACED BY `src/tui/` + `src/agent/ui/` (see New TUI section)**
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
(all PoC deps) + directories, tracing, crossterm 0.29, unicode-segmentation 1
```

**ratatui is dropped.** The TUI is rebuilt as a native main-screen library
(`src/tui/` + `src/agent/ui/`) porting pi's `@earendil-works/pi-tui` package directly.
See [`tui.md`](tui.md) for full design.

### New TUI (replaces ratatui-based TUI) ✅ IMPLEMENTED

- [x] **`src/tui/`** — Core TUI library (generic, reusable). Port of `@earendil-works/pi-tui`:
  - [x] `screen.rs` — Diff renderer (379 lines). Line-level comparison, ANSI cursor moves, synchronized output. Port of `tui.ts:doRender()`.
  - [x] `terminal.rs` — Crossterm wrapper (125 lines): raw mode, events, cursor, resize.
  - [x] `keys.rs` — Key identifiers (267 lines), `matches_key()`. Wrap crossterm `KeyEvent`.
  - [x] `util.rs` — ANSI-aware width, wrap, truncate, slice (817 lines).
  - [x] `component.rs` — `Component` trait, `Focusable` trait, `CURSOR_MARKER`.
  - [x] `container.rs` — `Container` struct.
  - [x] `fuzzy.rs` — `fuzzy_match()`, `fuzzy_filter()` (263 lines).
  - [x] `theme.rs` — `Theme` trait (fg/bg/bold color functions).
  - [x] `kill_ring.rs` — Emacs kill/yank ring buffer (128 lines).
  - [x] `undo_stack.rs` — Generic undo stack (73 lines).
  - [x] `word_nav.rs` — `find_word_backward()`, `find_word_forward()` (281 lines).
  - [x] `components/text.rs` (142), `truncated_text.rs` (72), `spacer.rs` (38), `box.rs` (113) — Structural primitives.
  - [x] `components/loader.rs` (109), `cancellable_loader.rs` (82) — Spinners.
  - [x] `components/select_list.rs` (305) — Scrollable selection list with fuzzy search.
  - [x] `components/settings_list.rs` (353) — Toggleable settings picker (cycle values, search).
  - [x] `components/input.rs` (549) — Single-line text input (grapheme cursor, kill-ring, undo).
  - [x] `components/editor.rs` (776) — Multi-line editor (word-wrap, kill-ring, undo, history).
- [x] **`src/agent/ui/`** — Rab-specific app components built on `src/tui/`:
  - [x] `app.rs` (731) — Main event loop, App state.
  - [x] `chat_editor.rs` (102) — Thin wrapper around `tui::Editor` for slash commands.
  - [x] `messages.rs` (155) — Renders conversation history as styled lines.
  - [x] `working.rs` (73) — Spinner during streaming.
  - [x] `footer.rs` (103) — Cwd + git branch + token stats + model.
  - [x] `model_selector.rs` (96) — Model picker overlay using `tui::SelectList`.
  - [x] `help.rs` (98) — `/help` display.
  - [x] `theme.rs` (105) — Concrete color theme, direct ANSI emission.
- [x] **Remove** `src/rattui/` (old ratatui-based TUI, ~3,200 lines)
- [x] **Remove** `src/theme.rs` (ratatui `Style`-based, 171 lines)
- [x] **Remove** `ratatui` from Cargo.toml dependencies
- [x] **Add** `crossterm 0.28` and `unicode-width 0.2` to Cargo.toml

### Deliverable

Full `rab` binary with print mode + interactive TUI mode (native main-screen, no alternate screen),
persistent sessions, context compaction, settings, slash commands, and custom compile-time extensions.

---

## Phase 2

**Goal:** Dynamic plugin system (WASM), skills, MCP adapter. Complete app per arch.md.

(Same as original — not yet started)

---

## Implemented

### PoC

- [x] **Project scaffold** — `cargo init`, Cargo.toml with PoC dependencies
- [x] **`types.rs`** — `AgentMessage`, `Role`, `ToolCall`, `Usage`, serde camelCase
- [x] **`provider.rs`** — `Provider` trait + `StreamEvent` enum + `StopReason` enum
- [x] **`adapter/genai.rs`** — `GenaiProvider` wrapping `genai::Client`, implements `Provider`
- [x] **`agent/extension.rs`** ✅ — `Extension` trait, `AgentTool` trait, `CommandHandler` trait, `CommandResult`, `SlashCommand`, `BlockReason`
- [x] **`builtin/read.rs`** — Read tool (offset, limit, line numbers, 50KB/2000-line truncation)
- [x] **`builtin/write.rs`** — Write tool (parent dirs, temp file + atomic rename)
- [x] **`builtin/edit.rs`** — Edit tool (multi-edit, uniqueness check, overlap detection, camelCase args)
- [x] **`builtin/bash.rs`** — Bash tool (sh -c, timeout, stdout+stderr, truncation)
- [x] **`agent/loop.rs`** ✅ — `run_agent_loop()` with inner loop, streaming, parallel tool execution, hook pipeline, `AgentEvent` emission
- [x] **`main.rs`** — CLI: `rab [--model <m>] <message>`, print-mode emitter, session flags, git branch detection
- [x] **`builtin/commands.rs`** — Built-in commands: `/quit`, `/model`, `/hotkeys`, `/reload`, `/new`, `/resume`, `/session`, `/name`
- [x] **`settings.rs`** — Load/save `~/.rab/agent/settings.json` + `.rab/settings.json` overlay
- [x] **`auth.rs`** — Load `~/.rab/agent/auth.json`, pi format
- [x] **`lib.rs`** — Crate root exposing all modules

### Phase 1

- [x] **`editor.rs`** (~2,500 lines) — Extracted from tui.rs, full-featured editor widget:
  - Grapheme-aware cursor, word wrapping, undo stack, kill ring
  - Word movement/deletion, bracketed paste with large paste markers
  - Slash command + file path autocomplete (pi-style: dropdown below border, fuzzy match, auto-accept)
  - 114 unit tests
- [x] **`tui.rs`** — Terminal UI with ratatui + crossterm:
  - Pi dark theme colors, tool output collapsed by default, thinking block folding
  - Model selector overlay, thinking/tool output toggles persisted to settings
  - `!`/`!!` bash inline execution with abort support
  - Pi-style paste detection (20ms timing heuristic)
  - Session history loading, message persistence on AgentEnd
  - 6 unit tests (session message conversion)
  - **↓ BEING REPLACED BY `src/tui/` + `src/agent/ui/` (see New TUI section)**
- [x] **`theme.rs`** — Theme struct with pi-style color fields, style helpers
- [x] **`session.rs`** — SessionManager with JSONL tree storage, 66 unit tests
- [x] **`settings.rs`** — Pi keys (`hideThinkingBlock`, `collapseToolOutput`), `save_to()` for testing
- [x] **`auth.rs`** — Supports `api_key` and `oauth` credential types
- [x] **`Cargo.toml`** — `native-tls` for Termux/Android, `unicode-segmentation` for editor

### Tests: 290 total (266 unit + 24 integration)

---

## Known Issues

### Scrolling in chat messages area
- No mouse wheel scrolling, no Page Up/Down, no arrow key scrolling for messages
- When messages overflow the viewport, scrollback is missing
- `scroll_line` and `auto_scroll` exist but no input handling to scrub

### TUI colors and styles
- Assistant markdown text not styled with pi's markdown theme colors (headings, code, links, quotes)
- Tool call lines missing bold tool name
- No markdown syntax highlighting in rendered output
- No per-thinking-level colors (pi has 6 levels: off→xhigh)
- No visual distinction between streaming/pending text and final text
- Footer tokens not padded/right-aligned properly on narrow terminals

## TODO

### Markdown rendering, diff display, code syntax highlighting
- Render assistant messages as markdown (headings, links, code blocks, quotes, lists) with pi theme colors
- Render diffs inline with `toolDiffAdded`/`toolDiffRemoved`/`toolDiffContext` colors (pi-style)
- Syntax highlighting for code blocks in markdown and tool output

### Chat messages scrolling
- Wire up mouse wheel events (crossterm MouseEvent) to scroll chat
- Wire up Page Up/Down and arrow keys (when editor is not focused) to scroll chat
- Add scrollbar or scroll indicators

### Additional features
- Markdown styling for user messages
- Streaming text vs final text visual distinction
- Per-thinking-level colors
- Footer token display padding fix
