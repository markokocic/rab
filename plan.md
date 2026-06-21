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

| Area | Status |
|------|--------|
| Missing app actions (clear, suspend, thinking cycle, model cycle, etc.) | ✅ 10 actions implemented |
| Message rendering polish (Markdown, OSC 133, tool expand/collapse) | ✅ Markdown + table rendering, OSC 133, expand/collapse |
| Scrolling (Page Up/Down, scroll indicators) | ✅ PageUp/PageDown, scroll indicator, reset on submit |

## Chat/UX gaps — Deferred 🟡

See `todo.md` for detailed task list. Major deferred areas:

- **Session management**: new, tree, fork, resume, toggleNamedFilter
- **Overlays**: config-selector, theme-selector, session-selector, first-time-setup, changelog, login-dialog, oauth-selector
- **Image support**: paste from clipboard, multimodal
- **Other**: suspend/resume, debug key, dynamic keybinding hints, viewport-managed scrolling
