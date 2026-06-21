# rab - Implementation Plan

Reference implementation: `~/src/cvstree/pi/` (TypeScript, same architecture).

## Pi source reference map

| rab module | pi source | Status |
|---|---|---|
| `agent/types.rs` | `packages/agent/src/types.ts` | ✅ |
| `agent/provider.rs` | `packages/ai/src/types.ts` | ✅ |
| `tui/` (all modules) | `packages/tui/src/` | ✅ **complete** |
| `agent/ui/` (app components) | `packages/coding-agent/src/modes/interactive/` | ✅ **core complete** |
| `agent/loop.rs` | `packages/agent/src/agent-loop.ts` | ✅ |
| `agent/session.rs` | `packages/agent/src/harness/session/` | ✅ |
| `agent/settings.rs` | `packages/coding-agent/src/core/settings-manager.ts` | ✅ |
| `agent/system_prompt.rs` | `packages/coding-agent/src/core/system-prompt.ts` | ✅ |
| `agent/context_files.rs` | `packages/coding-agent/src/core/resource-loader.ts` | ✅ |
| `agent/skills.rs` | `packages/agent/src/harness/skills.ts` | ✅ |
| `builtin/read.rs` | `packages/coding-agent/src/core/tools/read.ts` | ✅ |
| `builtin/write.rs` | `packages/coding-agent/src/core/tools/write.ts` | ✅ |
| `builtin/edit.rs` | `packages/coding-agent/src/core/tools/edit.ts` | ✅ |
| `builtin/bash.rs` | `packages/coding-agent/src/core/tools/bash.ts` | ✅ |
| `builtin/commands.rs` | `packages/coding-agent/src/core/slash-commands.ts` | ✅ |
| `auth.rs` | `packages/coding-agent/src/core/auth-storage.ts` | ✅ |
| `adapter/genai.rs` | pi has no genai; rab uses genai crate | ⬜ needs multi-backend |
| `compaction.rs` | `packages/agent/src/harness/compaction/` | ⬜ not implemented |
| `agent/extension.rs` (hooks) | `packages/agent/src/types.ts` | ⬜ partial |
| `settings.rs` (models.json) | `packages/coding-agent/src/core/settings-manager.ts` | ⬜ not implemented |

---

## What's done

### TUI library (`src/tui/`) — ✅ 1/1 with pi (429 tests)

- **Core**: Component trait, Container, Focusable, Screen diff renderer, overlay system, cursor markers
- **Terminal**: TerminalTrait, ProcessTerminal, Kitty keyboard protocol (flags 1+2+4), bracketed paste, progress indicator, drainInput, setTitle, OSC 2031 color scheme notifications
- **Keys & keybindings**: String-based key IDs, 27 action IDs, JSON config loading, all components migrated
- **Utilities**: Width caching, applyBackgroundToLine, extractSegments, CJK_BREAK_REGEX, WordNavigationOptions, PUNCTUATION_CHARS
- **Components**: Editor (paste markers, undo coalescing, sticky column, character jump, history draft, border_color, autocomplete provider), Input, SelectList, SettingsList, Loader, CancellableLoader, Box, Text, TruncatedText, Spacer, AutocompleteProvider
- **All 27 pi-tui source modules covered** (24 matched 1:1, 3 excluded by design)

### Agent framework

- Agent loop with streaming, parallel tool execution, AgentEvent emission
- SessionManager with JSONL tree storage, 66 tests
- Settings load/save with project-local overlay
- Auth storage (API keys + OAuth)
- System prompt builder with layered prompts, project context, skills XML
- Context file discovery (AGENTS.md/CLAUDE.md ancestor walk)
- Skill loading and `/skill:name` expansion
- Built-in tools (read, write, edit, bash) — behavioral 1/1 with pi
- Slash commands (quit, model, reload, new, resume, session, name)
- Multi-line editor with Emacs keybindings, paste markers, autocomplete, kill ring, undo

### App UI (`src/agent/ui/`)

- Main screen layout matching pi: header, messages, streaming text, queued messages, working indicator, editor, footer
- ChatEditor with slash command autocomplete
- Message queuing during streaming (dequeue on AgentEnd, restore on Ctrl+C)
- Streaming text display (pending_text/pending_thinking rendered inline)
- Overflow prevention (all lines padded/truncated to terminal width)
- Working indicator always rendered (empty line when inactive for layout stability)
- HelpOverlay and ModelSelector via TUI.show_overlay()
- Theme system: JSON-based dark/light with variable resolution, truecolor+256 fallback
- Bash execution component with styled borders and expand/collapse

---

## What's left

### Agent framework (8 items)

| Item | Effort | Notes |
|---|---|---|
| `adapter/genai.rs` — multiple backends | ⭐⭐⭐ | Anthropic, OpenAI, Google, Ollama — biggest remaining piece |
| `compaction.rs` — context window compaction | ⭐⭐ | Token estimation, cut point, summary generation |
| Hook pipeline — `before_tool_call`, `after_tool_call` | ⭐⭐ | CancellationToken passed to all hooks |
| Steering / follow-up queues | ⭐⭐ | Runtime message injection mid-turn and post-turn |
| `~/.rab/models.json` | ⭐ | Custom provider/model definitions |
| Image support — multimodal payload | ⭐ | Read tool detects images, passes via provider |
| Tool execution modes — sequential | ⭐ | Execute one tool at a time, feed result before next |
| `rab plugin new` — scaffold | ⭐ | Simple Cargo.toml + lib.rs |

### Chat/UX gaps (see todo.md for full detail)

- **12 missing app actions**: clear, suspend, thinking.cycle, model.cycleForward/Backward, tools.expand, editor.external, message.followUp/dequeue, clipboard.pasteImage, session management
- **Message rendering**: user messages as Markdown, OSC 133 zone markers, tool output expand/collapse with preview, visual truncation, countdown timer
- **Scrolling**: mouse wheel, Page Up/Down, scroll indicators
- **Footer**: auto-compact indicator, token padding
- **Editor**: auto-trigger slash commands on `/`, external editor
- **Missing overlays**: config-selector, theme-selector, session-selector, first-time-setup, changelog, login-dialog
- **Other**: suspend/resume (Ctrl+Z), debug key, dynamic keybinding hints

---

## Test count: 429 passing, 0 failing
