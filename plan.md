# rab — Implementation Plan

Reference: `~/src/cvstree/pi/` (TypeScript, same architecture).

## Phase 1 — Core agent ✅

| Area | Status |
|------|--------|
| TUI library (29 modules, **662 tests**) | ✅ 1/1 with pi |
| Agent loop (streaming, tool execution, events, hook pipeline) | ✅ before_tool_call + after_tool_call wired; steering/follow-up queues infrastructure in place |
| Session persistence (JSONL tree, 66+ tests) | ✅ |
| Built-in tools (read, write, edit, bash) | ✅ 1/1 with pi (all renderers aligned) |
| System prompt builder (AGENTS.md, skills, context) | ✅ |
| Settings, auth, keybindings | ✅ (`~/.rab/agent/settings.json`, `~/.rab/agent/auth.json`, `~/.rab/keybindings.json`) |
| Skills (loading, prompt formatting, `/skill:name`) | ✅ `src/agent/skills.rs` - frontmatter parsing, XML prompt formatting |
| App UI (ChatEditor, Messages, Footer, ModelSelector, Help, overlays) | ✅ Full component tree, overlay system, 8 components |
| **ChatEditor → pi's CustomEditor alignment** | ✅ Ctrl+Z undo, Up/Down history via Editor, Tab via AutocompleteProvider, Enter via Editor's submit(), backslash continuation, visual-line-based history trigger, proper state cleanup on submit |
| **Message rendering (8 gaps closed)** | ✅ Tool renderers, progressive rendering, diff preview, bash streaming, truncation, caching |
| **Image system (basic)** | ⬜ `src/tui/image.rs` has data URL encoding + Kitty protocol sequences, but no TUI Component, no capabilities detection, no iTerm2 support, no resize/convert/paste |
| **Keybindings** | ✅ 27+ action IDs with defaults, JSON config loading |
| **Overlay system** | ✅ Anchor-based positioning, sizing, margins, compositing in `src/tui/overlay.rs` + `src/tui/tui_core.rs` |
| **Markdown rendering** | ✅ `src/tui/components/markdown.rs` (2103 lines) - pulldown-cmark, syntax highlighting, tables, code blocks |
| **Diff rendering** | ✅ Unified diff with colored +/- lines and intra-line character-level inverse |

## Phase 1 — Remaining

| Item | Priority | Notes |
|------|----------|-------|
| Multi-backend provider (`adapter/genai.rs`) | high | Currently only OpenCode Go support. Need auto-detection (claude→Anthropic, gpt→OpenAI, gemini→Gemini, fallback→Ollama) |
| Context window compaction | high | `compact` field exists in types, no summarization logic yet |
| `~/.rab/models.json` | medium | Custom provider/model definitions |
| Image system (7 gaps, see below) | medium | Basic kitty protocol exists, needs capabilities detect, iTerm2, sizing, resize, convert, paste, selector UI |
| UI components (10 gaps, see below) | medium | Session selector, theme selector, thinking level selector, settings selector, login dialog, trust selector, first-time setup |
| Tool execution modes (sequential) | low | Only parallel is implemented |
| Steering / follow-up queues (active use) | medium | Infrastructure exists, not actively used by TUI yet |
| Slash commands (14 missing) | medium | 8/22 implemented; see todo.md for full list |

## Image system gaps (7)

| # | Gap | Est. | Notes |
|---|-----|------|-------|
| C4 | TUI `Image` component (Kitty + iTerm2 + fallback) | medium | Basic Kitty protocol in `image.rs`, no Component impl |
| C5 | Terminal capabilities detection (`getCapabilities()`) | small | |
| C6 | Cell dimension tracking for pixel-accurate sizing | small | |
| C7 | Image resize utility | medium | |
| C8 | Image convert utility | small | |
| C9 | Clipboard image paste | medium | |
| C10 | Show images selector UI | medium | |

## UI component gaps (10)

| # | Gap | Est. | Notes |
|---|-----|------|-------|
| C12 | Session selector (`session-selector.ts` + search) | medium | `CommandResult::OpenSessionSelector` exists, no UI |
| C13 | Theme selector overlay | medium | |
| C14 | Thinking level selector | small | |
| C15 | Extension editor / input / selector | large | |
| C16 | Config / settings selector | medium | |
| C17 | Model selector improvements | medium | Basic SelectList-based selector exists |
| C18 | OAuth login dialog | medium | |
| C19 | Trust selector | small | |
| C20 | First-time setup | medium | |

## Phase 2 — Extensions & plugins

| Item | Priority | Notes |
|------|----------|-------|
| WASM plugin system (wasmtime + WIT) | low | Not started |
| MCP adapter (rmcp crate) | low | Not started |
| Dynamic hot-reload | low | Not started |

## Chat/UX gaps — Completed ✅

### Rendering architecture (pi 1:1)

| Area | Status |
|------|--------|
| Component tree (TUI extends Container) | ✅ `TUI.root: Container`, recursive `render()` |
| Message Components (User, Assistant, Tool, Bash, Info, Header) | ✅ 8+ components with proper Box/bg/markdown/OSC133 |
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
| Overlay compositing | ✅ Anchor-based overlay positioning, sizing, margin, focus management |
| Keybinding system | ✅ 27+ action IDs, JSON config, `matches_key()` dispatch |

### Tool rendering (8 gaps, all closed)

| # | Gap | Solution |
|---|-----|----------|
| 1 | Image support (Kitty protocol) | `tui::image.rs` — data URL encoding, Kitty sequences, is_image_line detection |
| 2 | Visual-line-aware truncation | `tui::visual_truncate.rs` — `truncate_to_visual_lines()` shared utility |
| 3 | Progressive arg rendering | `ToolCallArgsUpdate` event + `set_args()` with dirty tracking |
| 4 | lastComponent caching | `RenderCache` with `state_hash()` key |
| 5 | invalidate() per row | `dirty` flag on all setters, `Component` trait methods |
| 6 | Write incremental caching | `RwLock<WriteCache>` with content hash key |
| 7 | Edit diff preview | Compact old/new preview in `EditRenderer::render_call()` |
| 8 | grep/find/ls renderers | Command detection in `BashRenderer` |

### Other

| Area | Status |
|------|--------|
| Missing app actions (clear, suspend, thinking cycle, model cycle, etc.) | ✅ 11 actions implemented |
| Scrolling (Page Up/Down, scroll indicators) | ✅ PageUp/PageDown, scroll indicator, reset on submit |
| Editor & input (auto-trigger slash autocomplete) | ✅ Auto-shows on `/char`, checked after external editor/dequeue |
| Footer improvements (auto-compact, narrow terminal, extension status) | ✅ `app.compact.toggle`, graceful truncation, status line |

## Chat/UX gaps — 🟡 In Progress / Deferred

### Slash commands (14 of 22 pi built-ins not implemented; 8 implemented)

| Command | Status | Priority | Notes |
|---------|--------|----------|-------|
| `/quit` | ✅ | — | Graceful shutdown |
| `/model` | ✅ | — | Switch model; no args lists available models |
| `/hotkeys` | ✅ | — | Show keyboard shortcuts |
| `/reload` | ✅ | — | Reload settings and auth from disk |
| `/new` | ✅ | — | Clear conversation |
| `/resume` | ✅ | — | Open session selector |
| `/session` | ✅ | — | Show session info |
| `/name` | ✅ | — | Set session display name |
| `/settings` | ❌ | high | Settings menu/overlay |
| `/export` | ❌ | high | Session export (.html/.jsonl) |
| `/import` | ❌ | high | Import and resume a session from JSONL |
| `/copy` | ❌ | high | Copy last assistant message to clipboard |
| `/compact` | ❌ | high | Manual session compaction |
| `/changelog` | ❌ | high | Changelog overlay |
| `/scoped-models` | ❌ | medium | Filter models for Ctrl+P cycling |
| `/fork` | ❌ | medium | Fork session from previous message |
| `/clone` | ❌ | medium | Duplicate current session |
| `/trust` | ❌ | medium | Project trust decision |
| `/login` | ❌ | medium | Provider auth config |
| `/logout` | ❌ | medium | Remove provider auth |
| `/share` | ❌ | low | Share as GitHub gist |
| `/tree` | ❌ | low | Session tree navigation |

### Agent framework (from Phase 1 — Remaining)

| Item | Priority | Notes |
|------|----------|-------|
| Multi-backend provider (`adapter/genai.rs`) | high | Currently single backend (OpenCode Go) |
| Context window compaction | high | Not implemented |
| `~/.rab/models.json` | medium | Not implemented |
| Image system (7 gaps) | medium | Basic image.rs exists, needs full support |
| UI components (10 gaps) | medium | See table above |
| Tool execution modes (sequential) | low | |
| Steering / follow-up queues (active use) | medium | Infrastructure exists, not wired in TUI message queuing |
| `rab plugin new` scaffold | low | |
