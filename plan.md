# rab — Implementation Plan

Reference: `~/src/cvstree/pi/` (TypeScript, same architecture).

## Phase 1 — Core agent ✅

| Area | Status |
|------|--------|
| TUI library (27 modules, 429 tests) | ✅ 1/1 with pi |
| Agent loop (streaming, tool execution, events) | ✅ |
| Session persistence (JSONL tree, 66 tests) | ✅ |
| Built-in tools (read, write, edit, bash) | ✅ 1/1 with pi |
| System prompt builder (AGENTS.md, skills, context) | ✅ |
| Settings, auth, keybindings | ✅ |
| Skills (loading, prompt formatting, `/skill:name`) | ✅ |
| App UI (ChatEditor, Messages, Footer, ModelSelector, Help) | ✅ |
| **ChatEditor → pi's CustomEditor alignment** | ✅ Ctrl+Z undo, Up/Down history via Editor, Tab via AutocompleteProvider, Enter via Editor's submit(), backslash continuation, visual-line-based history trigger, proper state cleanup on submit |

## Phase 1 — Remaining

| Item | Priority |
|------|----------|
| Multi-backend provider (`adapter/genai.rs`) | high |
| Context window compaction | high |
| Hook pipeline (`before_tool_call`, `after_tool_call`) | medium |
| Steering / follow-up queues | medium |
| `~/.rab/models.json` | medium |
| Tool execution modes (sequential) | low |
| `rab plugin new` scaffold | low |

## Phase 2 — Extensions & plugins

| Item | Priority |
|------|----------|
| WASM plugin system (wasmtime + WIT) | low |
| MCP adapter (rmcp crate) | low |
| Dynamic hot-reload | low |

## Chat/UX gaps — Completed ✅

### Rendering architecture (pi 1:1)

| Area | Status |
|------|--------|
| Component tree (TUI extends Container) | ✅ `TUI.root: Container`, recursive `render()` |
| Message Components (User, Assistant, Tool, Bash, Info, Header) | ✅ 8 components with proper Box/bg/markdown/OSC133 |
| Tool bg transitions (pending→success/error) | ✅ `ToolExecComponent` with per-tool formatting |
| Expand/collapse global toggle | ✅ `set_expanded()` on Component trait |
| Editor border color (thinking level + bash mode) | ✅ `update_border_color()` |
| Spacers between all messages | ✅ `chat_add()` helper |
| Progressive streaming (assistant text) | ✅ `Weak<RefCell<AssistantMessageComponent>>` in-place updates |
| Progressive bash output | ✅ `AgentEvent::ToolProgress` with tokio async reads |
| Syntax highlighting | ✅ syntect enabled, `highlight_code()`, `path_to_language()` |
| Edit diff rendering | ✅ `render_diff()` with intra-line character-level inverse |
| Bash duration display | ✅ "Elapsed X.Xs" / "Took X.Xs" |
| Error/abort inline display | ✅ `AgentEvent::Aborted` — inline in streaming component |
| Write success hides output | ✅ Only bg transition, no text |
| Git branch refresh | ✅ on AgentStart |
| Theme completeness | ✅ All 44 color tokens from pi, all 9 syntax colors |

### Other

| Area | Status |
|------|--------|
| Missing app actions (clear, suspend, thinking cycle, model cycle, etc.) | ✅ 11 actions implemented |
| Scrolling (Page Up/Down, scroll indicators) | ✅ PageUp/PageDown, scroll indicator, reset on submit |
| Editor & input (auto-trigger slash autocomplete) | ✅ Auto-shows on `/char`, checked after external editor/dequeue |
| Footer improvements (auto-compact, narrow terminal, extension status) | ✅ `app.compact.toggle`, graceful truncation, status line |

## Chat/UX gaps — Deferred 🟡

### Missing slash commands (14 of 22 pi built-ins not implemented)

| Command | Priority | Notes |
|---------|----------|-------|
| `/settings` | high | Settings menu/overlay — needs overlay component |
| `/export` | high | Session export (.html/.jsonl) — needs file I/O + HTML template |
| `/import` | high | Import and resume a session from JSONL — needs file picker |
| `/copy` | high | Copy last assistant message to clipboard — needs clipboard crate |
| `/compact` | high | Manual session compaction — needs compaction logic first |
| `/changelog` | high | Changelog overlay — needs overlay component + changelog data |
| `/scoped-models` | medium | Filter models for Ctrl+P cycling — needs model filtering state |
| `/fork` | medium | Fork session from previous message — needs fork UI |
| `/clone` | medium | Duplicate current session — needs clone logic |
| `/trust` | medium | Project trust decision — needs trust storage mechanism |
| `/login` | medium | Provider auth config — login-dialog overlay |
| `/logout` | medium | Remove provider auth — needs auth state management |
| `/share` | low | Share as GitHub gist — needs GitHub API |
| `/tree` | low | Session tree navigation — session-selector overlay |

See `todo.md` for detailed task list.


See `todo.md` for detailed task list. Major deferred areas:

- **Overlays**: config-selector, theme-selector, session-selector, first-time-setup, changelog, login-dialog, oauth-selector
- **Session management** (→ slash commands in todo.md): new, tree, fork, resume, toggleNamedFilter
- **Other**: suspend/resume, debug key, dynamic keybinding hints, viewport-managed scrolling

### Agent framework (from Phase 1 — Remaining)

| Item | Priority |
|------|----------|
| Multi-backend provider (`adapter/genai.rs`) | high |
| Context window compaction | high |
| Hook pipeline (`before_tool_call`, `after_tool_call`) | medium |
| Steering / follow-up queues | medium |
| `~/.rab/models.json` | medium |
| Image support (multimodal) | medium |
| Tool execution modes (sequential) | low |
| Slash commands (14 missing) | medium | See todo.md for full list, 8/22 implemented |
| `rab plugin new` scaffold | low |
