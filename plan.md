# rab — Implementation Plan

Reference: `~/src/cvstree/pi/` (TypeScript, same architecture).

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

## Chat/UX gaps — 🟡 In Progress / Deferred

### Slash commands (14 of 22 pi built-ins not implemented; 8 implemented)

| Command | Status | Priority | Notes |
|---------|--------|----------|-------|
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
