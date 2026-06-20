# rab - Implementation Plan

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
| `system_prompt.rs` ✅ | `packages/coding-agent/src/core/system-prompt.ts` |
| `context_files.rs` ✅ | `packages/coding-agent/src/core/resource-loader.ts` (`loadProjectContextFiles`) |
| `commands.rs` | `packages/coding-agent/src/core/slash-commands.ts` |
| `agent/ui/` ✅ | `packages/coding-agent/src/modes/interactive/` (app-specific UI components) |
| `adapter.rs` | pi has no genai; rab uses genai crate for HTTP+streaming |
| `skills.rs` ✅ | `packages/coding-agent/src/core/skills.ts` + `packages/agent/src/harness/skills.ts` |

---

## Phase 1

**Goal:** Full-featured coding agent with TUI, sessions, settings, compaction.
Everything in arch.md that isn't explicitly Phase 2.

### Tasks

- [ ] **`adapter/genai.rs`** - Extend PoC's `GenaiProvider` to support multiple backends:
  - OpenCode Go (existing from PoC)
  - Anthropic, OpenAI, Google, DeepSeek (direct), Ollama
  - Provider auto-detection from model name prefix (`claude*`, `gpt*`, `gemini*`)
  - `rab connect` command for interactive provider setup (like pi's `/connect`)
- [x] **`cli.rs`** - CLI with all flags and subcommands (partial - hand-rolled parser):
  - `[MESSAGE]...` positional (prompt) ✅
  - `-c, --continue`, `--session PATH`, `--no-session`, `--name <name>`, `--session-dir <dir>` ✅
  - `--model MODEL` ✅
  - `--thinking LEVEL` ❌
  - `--no-tools`, `--no-builtin-tools`, `--no-extensions` ❌
  - `-nc, --no-context-files` ❌
  - `-V, --version`, `-h, --help` ❌
  - Mode dispatch: print mode (default) vs interactive mode (TUI) ✅
- [x] **`settings.rs`** - Extend PoC settings with full pi schema:
  - Thinking level, tools allow/deny lists, theme ✅
  - `~/.rab/models.json` for custom provider/model definitions ❌
  - CLI flags override settings file values ✅ (partial - --model only)
- [x] **`system_prompt.rs`** - Build system prompt from:
  - Base prompt (hardcoded tool descriptions, response format) ✅
  - `~/.rab/AGENTS.md` (global context) ✅
  - `AGENTS.md` / `CLAUDE.md` walked up from cwd (project context) ✅
  - Wrapped in `<project_context>` tags ✅
  - `<available_skills>` XML block with skill metadata ✅
  - Respect `APPEND_SYSTEM.md` / `SYSTEM.md` (full override) ✅
  - `--no-context-files` flag ✅
  - `--system-prompt` / `--append-system-prompt` flags ✅
- [x] **`session.rs`** - `SessionManager` with JSONL storage:
  - Create new session, continue recent, open by path ✅
  - Append `AgentMessage` entries ✅
  - Walk from root along active branch (tree with `parentId`) ✅
  - `~/.rab/sessions/<cwd-hash>/` directory structure ✅
  - Corruption handling (malformed lines, empty files, missing headers) ✅
  - All 10 pi entry types ✅
  - Deferred flush (no file until first assistant message) ✅
  - 66 unit tests
- [ ] **`compaction.rs`** - Context window compaction:
  - Token estimation via model-specific heuristic or tiktoken-rs
  - Cut point finder (accumulate from oldest, preserve tail)
  - Summary generation (fast model prompt)
  - Replace old messages with synthetic summary message
  - Auto-trigger before context overflow; manual trigger via `/compact`
- [x] **`commands.rs`** - Core slash commands (partial):
  - `/model <name>` - switch active model ✅
  - `/thinking <level>` - set thinking level ❌
  - `/compact [prompt]` - manual compaction ❌
  - `/session` - print session info ✅
  - `/name <text>` - set session display name ✅
  - `/fork` - fork session from previous user message ❌
  - `/clone` - duplicate active branch into new session ❌
  - `/resume` - list previous sessions in cwd ✅ (returns OpenSessionSelector; UI not built)
  - `/new` - start fresh session ✅
  - `/copy` - copy last assistant message to clipboard ❌
  - `/export [path]` - export session to HTML ❌
  - `/settings` - print or edit settings ❌
  - `/reload` - reload AGENTS.md, skills, settings ✅
  - `/quit` - exit (interactive mode) ✅
- [x] **`editor.rs`** - Custom editor widget (extracted from tui.rs):
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
- [x] **Editor autocomplete system** - Pi-style slash command and file path autocomplete ✅:
  - Slash command completion with fuzzy matching (all chars in order, case-insensitive) ✅
  - Auto-accept single match on Tab (pi: explicitTab + single item) ✅
  - Argument completions bridged from `CommandHandler::argument_completions()` ✅
  - `@` file path completion with directory listing ✅
  - Tab file path completion without `@` prefix ✅
  - Arrow key navigation with wrap-around, Enter/Tab to accept, Esc to dismiss ✅
  - Dropdown renders below editor block border (pi-style), height auto-adjusts ✅
  - SelectList-style centered scroll window, max visible 5, column layout ✅
  - Theme styling: selected accent+bold `→`, normal muted, descriptions in column ✅
- [x] **Message queuing during streaming** - Pi-style follow-up queue:
  - `submit_message` queues instead of spawning concurrent agent loop when `is_streaming` ✅
  - `start_agent_loop()` helper extracted for single-entry spawn point ✅
  - `AgentEnd` handler dequeues and auto-submits next queued message ✅
  - Ctrl+C restores queued messages to editor (matching pi) ✅
  - Queued messages rendered between chat and editor (pi's `pendingMessagesContainer`) ✅
- [x] **Streaming text display** - Pi-style incremental rendering:
  - `pending_text` / `pending_thinking` rendered inline in compose_ui during streaming ✅
  - Text appears character-by-character as deltas arrive, not only after flush ✅
- [x] **Screen viewport tracking fix** - Content scrolling beyond terminal height:
  - `viewport_top` made mutable and incremented on scroll ✅
  - Cursor position calculations use consistent (updated) viewport ✅
  - `prev_viewport_top` recalculated at end: `max(viewport_top, render_end - height + 1)` ✅
  - `max_lines_rendered` tracked during differential renders for correct `clear_on_shrink` ✅
- [x] **Slash command autocomplete** - Pi-style dropdown below editor border:
  - Tab triggers completion for `/command` prefix ✅
  - Up/Down navigates dropdown with wrap-around ✅
  - Enter/Tab accepts selection ✅
  - Escape closes dropdown ✅
  - Suggestions from ChatEditor.get_autocomplete_suggestions() ✅
- [x] **Layout stability** - Working indicator always rendered:
  - Removed `if is_streaming` guard - one empty line when inactive keeps line count stable ✅
  - Eliminates full-screen clears when streaming starts/stops ✅
- [x] **Overflow prevention** - All lines padded/truncated to terminal width:
  - `AssistantText` lines now use `pad_to_width()` (matching User, Info, ToolCall etc.) ✅
  - `pad_to_width()` truncates via `truncate_to_width()` when `visible_width > width` ✅
- [ ] **Hook pipeline** - Extend PoC hooks with `AgentContext` parameter and `CancellationToken`:
  - `before_tool_call` - all extensions consulted, first block wins
  - `after_tool_call` - result patching
  - `CancellationToken` passed to every hook
- [ ] **Steering / follow-up queues** - Runtime message injection:
  - Steering: injected after current turn's tool calls finish (mid-run user input)
  - Follow-up: injected after agent would stop (post-run follow-up)
  - Drain modes: `one-at-a-time` and `all`
- [ ] **Tool execution modes** - `AgentTool::execution_mode` override (PoC has parallel only)
  - Sequential: execute one tool, feed result before starting next
- [ ] **Compile-time user extensions** - `Extension` trait impls registered at startup
  - `--no-extensions` flag
- [ ] **`~/.rab/models.json`** - Custom provider/model definitions
- [ ] **Error handling** - No unwraps in core loop, graceful degradation, tracing instrumentation
- [ ] **Image support** - Read tool detects image files, reads as base64, passes via multimodal payload
- [ ] **Bash security** - Command deny-list (MVP: basic blocklist)
- [ ] **`rab plugin new`** - Scaffold a compile-time extension crate (simple Cargo.toml + lib.rs)

### Deliverable

Full `rab` binary with print mode + interactive TUI mode (native main-screen, no alternate screen),
persistent sessions, context compaction, settings, slash commands, and custom compile-time extensions.

---

## Phase 2

**Goal:** Dynamic plugin system (WASM), skills, MCP adapter. Complete app per arch.md.

(Same as original - not yet started)

---

## Implemented

### PoC

- [x] **Project scaffold** - `cargo init`, Cargo.toml with PoC dependencies
- [x] **`types.rs`** - `AgentMessage`, `Role`, `ToolCall`, `Usage`, serde camelCase
- [x] **`provider.rs`** - `Provider` trait + `StreamEvent` enum + `StopReason` enum
- [x] **`adapter/genai.rs`** - `GenaiProvider` wrapping `genai::Client`, implements `Provider`
- [x] **Theme system** — JSON file-based (`src/agent/ui/themes/{dark,light}.json` + custom `~/.rab/themes/`), variable resolution, truecolor + 256 fallback, `COLORFGBG` detection, `init_theme()`, `current_theme()`, `set_theme()`
- [x] **Bash execution component** — `src/agent/ui/components/bash_execution.rs`: styled borders, status-aware colors, preview truncation, expand/collapse
- [x] **Custom tool renderers** — `AgentTool::render_call()` / `render_result()` trait methods for per-tool ANSI-styled rendering
- [x] **`components/` module** — `src/agent/ui/components/mod.rs` organizing bash_execution and future components
- [x] **BashCommand display variant** — `DisplayMsg::BashCommand` enum variant rendered via `BashExecution` component`
- [x] **`builtin/read.rs`** - Read tool (offset, limit, line numbers, 50KB/2000-line truncation)
- [x] **`builtin/write.rs`** - Write tool (parent dirs, temp file + atomic rename)
- [x] **`builtin/edit.rs`** - Edit tool (multi-edit, uniqueness check, overlap detection, camelCase args)
- [x] **`builtin/bash.rs`** - Bash tool (sh -c, timeout, stdout+stderr, truncation)
- [x] **`agent/loop.rs`** ✅ - `run_agent_loop()` with inner loop, streaming, parallel tool execution, hook pipeline, `AgentEvent` emission
- [x] **`main.rs`** - CLI: `rab [--model <m>] <message>`, print-mode emitter, session flags, git branch detection
- [x] **`builtin/commands.rs`** - Built-in commands: `/quit`, `/model`, `/hotkeys`, `/reload`, `/new`, `/resume`, `/session`, `/name`
- [x] **`settings.rs`** - Load/save `~/.rab/agent/settings.json` + `.rab/settings.json` overlay
- [x] **`auth.rs`** - Load `~/.rab/agent/auth.json`, pi format
- [x] **`lib.rs`** - Crate root exposing all modules

### Phase 1

- [x] **`session.rs`** - SessionManager with JSONL tree storage, 66 unit tests
- [x] **`context_files.rs`** - AGENTS.md/CLAUDE.md discovery (global → ancestors → cwd)
- [x] **`system_prompt.rs`** - SystemPromptBuilder with layered prompt, context XML, skills XML, date/cwd
- [x] **`skills.rs`** - Skill loading, frontmatter parsing, `format_skills_for_prompt()`, `format_skill_invocation()`, `/skill:name` expansion
- [x] **Startup resource listing** - Context files and skills shown in welcome message (pi-style)
- [x] **`settings.rs`** - Pi keys (`hideThinkingBlock`, `collapseToolOutput`), `save_to()` for testing
- [x] **`auth.rs`** - Supports `api_key` and `oauth` credential types
- [x] **`Cargo.toml`** - `native-tls` for Termux/Android, `unicode-segmentation` for editor
- [x] **Main screen layout matches pi** - Header at top with logo + hints, messages, working indicator, editor, footer
- [x] **Message queuing during streaming** - `submit_message` queues when `is_streaming`, dequeues on `AgentEnd`; Ctrl+C restores to editor
- [x] **Streaming text display** - `pending_text`/`pending_thinking` rendered inline, visible as deltas arrive
- [x] **Screen viewport tracking** - `viewport_top` mutable, updated on scroll + at end of render; `max_lines_rendered` tracked in differential path
- [x] **Working indicator always rendered** - Empty line when inactive keeps line count stable, prevents full-screen clears on streaming state change
- [x] **Overflow prevention** - All message lines padded to `width`; `pad_to_width()` truncates via `truncate_to_width()` when `visible_width > width`

### Tests: 323 total (173 unit + 150 integration)
