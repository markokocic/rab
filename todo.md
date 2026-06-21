## pi-tui alignment — ✅ COMPLETE

All 6 phases of the pi-tui alignment are implemented. 429 tests pass. 27 modules cover all scoped pi-tui functionality (excluding images, Markdown, and Kitty protocol which were scoped out).

- **Core framework**: TUI struct, overlay system, focus management, Screen diff renderer, cursor marker extraction
- **Terminal**: `TerminalTrait`, `ProcessTerminal`, Kitty keyboard protocol (flags 1+2+4), bracketed paste, progress indicator, `drainInput()`, `setTitle()`
- **Keys & keybindings**: String-based key IDs, 27 action IDs, JSON config loading, all components migrated
- **Utilities**: Width caching, `applyBackgroundToLine`, `extractSegments`, `CJK_BREAK_REGEX`, `WordNavigationOptions`, `PUNCTUATION_CHARS`
- **Components**: Editor (paste markers, undo coalescing, sticky column, character jump, history draft, `border_color`, autocomplete), Input, SelectList, SettingsList, Loader, CancellableLoader, Box, Text, TruncatedText — all 1/1 with pi
- **Autocomplete**: `AutocompleteProvider` trait, `CombinedAutocompleteProvider` (slash commands + file paths)
- **Overlays**: HelpOverlay, ModelSelector via `TUI.show_overlay()`

### TUI low-priority deferred items
- [ ] migrate app loop to `TerminalTrait` (currently uses crossterm directly — no behavior difference)
- [ ] color scheme notifications (OSC 2031) — optional
- [ ] write logging (`PI_TUI_WRITE_LOG`) — optional

## tools
- [ ] check tool execution modes in pi, parallel, sequence, ... and compare with rab

## Chat/UX
- [ ] auto-trigger slash commands on `/` (currently requires `/` + Tab)
- [ ] mouse wheel events for chat scrolling
- [ ] Page Up/Down chat scrolling (when editor not focused)
- [ ] scrollbar/scroll indicators
- [ ] check other message types rendering (tool results, assistant text)
- [ ] footer token display padding fix on narrow terminals
- [ ] assistant text: apply theme colors for bold, code, links

## Agent framework
- [ ] `adapter/genai.rs` — multiple backends (Anthropic, OpenAI, Google, Ollama)
- [ ] `compaction.rs` — context window compaction
- [ ] Hook pipeline — `before_tool_call`, `after_tool_call`, `CancellationToken`
- [ ] Steering / follow-up queues — runtime message injection
- [ ] Tool execution modes — sequential mode
- [ ] `~/.rab/models.json` — custom provider/model definitions
- [ ] Image support — multimodal payload
- [ ] `rab plugin new` — scaffold extension crate

## ✅ Done
- [x] System prompt (AGENTS.md/CLAUDE.md, SYSTEM.md, APPEND_SYSTEM.md, project context)
- [x] Context file discovery
- [x] Skills loading and `/skill:name` expansion
- [x] CLI flags (`--no-context-files`, `--system-prompt`, `--append-system-prompt`)
- [x] Startup resource listing
- [x] Built-in tools (bash, read, write, edit) — behavioral 1/1 with pi
- [x] Thinking message rendering with per-level colors
- [x] **Complete pi-tui alignment** — 27 modules, 429 tests, all 6 phases
