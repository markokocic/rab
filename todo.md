## pi-tui alignment — ✅ COMPLETE

All 6 phases of the pi-tui alignment are implemented. 429 tests pass. 27 modules cover all scoped pi-tui functionality (excluding images, Markdown, and Kitty protocol which were scoped out).

- **Core framework**: TUI struct, overlay system, focus management, Screen diff renderer, cursor marker extraction
- **Terminal**: `TerminalTrait`, `ProcessTerminal`, Kitty keyboard protocol (flags 1+2+4), bracketed paste, progress indicator, `drainInput()`, `setTitle()`
- **Keys & keybindings**: String-based key IDs, 27 action IDs, JSON config loading, all components migrated
- **Utilities**: Width caching, `applyBackgroundToLine`, `extractSegments`, `CJK_BREAK_REGEX`, `WordNavigationOptions`, `PUNCTUATION_CHARS`
- **Components**: Editor (paste markers, undo coalescing, sticky column, character jump, history draft, `border_color`, autocomplete), Input, SelectList, SettingsList, Loader, CancellableLoader, Box, Text, TruncatedText — all 1/1 with pi
- **Autocomplete**: `AutocompleteProvider` trait, `CombinedAutocompleteProvider` (slash commands + file paths)
- **Overlays**: HelpOverlay, ModelSelector via `TUI.show_overlay()`

### TUI — ✅ complete (write logging is optional debug tooling)
- [x] App loop uses `ProcessTerminal` + `TerminalTrait` (no direct crossterm)
- [x] Color scheme notifications (OSC 2031)
- [ ] Write logging (`PI_TUI_WRITE_LOG`) — optional, defer

## tools
- [ ] check tool execution modes in pi, parallel, sequence, ... and compare with rab

## Chat/UX gaps vs pi

### Missing app actions (pi has 18 app.*, rab has 9)
- [ ] `app.clear` — Ctrl+C when not streaming = clear editor
- [ ] `app.suspend` — Ctrl+Z / SIGTSTP suspend
- [ ] `app.thinking.cycle` — cycle thinking levels (off/low/medium/high/xhigh)
- [ ] `app.model.cycleForward` / `app.model.cycleBackward` — cycle models with keybindings
- [ ] `app.tools.expand` — toggle all tool output expansion
- [ ] `app.editor.external` — open \$EDITOR for current editor content
- [ ] `app.message.followUp` — type-ahead: queue a message while streaming
- [ ] `app.message.dequeue` — edit all queued messages
- [ ] `app.clipboard.pasteImage` — paste clipboard image as attachment
- [ ] `app.session.fork` / `app.session.new` / `app.session.resume` / `app.session.tree` — session management

### Message rendering
- [ ] user messages: render as Markdown (same as pi — currently plain text)
- [ ] assistant messages: use theme colors for bold, code, links (pi uses Markdown component)
- [ ] OSC 133 zone markers around messages (`\x1b]133;A\x07` / `\x1b]133;B\x07` / `\x1b]133;C\x07`)
- [ ] tool output: expand/collapse with preview truncation (pi's `BashExecutionComponent`)
- [ ] tool output: preview vs full toggle keybinding
- [ ] visual truncation of long output lines (pi's `visual-truncate.ts`)
- [ ] countdown timer for auto-retry (pi's `CountdownTimer`)

### Scrolling & navigation
- [ ] mouse wheel events for chat scrolling
- [ ] Page Up/Down chat scrolling (when editor not focused)
- [ ] scrollbar/scroll indicators

### Footer
- [ ] context window auto-compact indicator toggle
- [ ] token display padding fix on narrow terminals
- [ ] extension status line (already partially there)

### Editor & input
- [ ] auto-trigger slash commands on `/` (currently requires `/` + Tab)
- [ ] external editor (":e" or keybinding opens \$EDITOR)
- [ ] paste image from clipboard (requires image support)

### Overlays (missing pi components)
- [ ] `config-selector` — pick from stored configs
- [ ] `theme-selector` — pick theme
- [ ] `session-selector` — tree view of sessions
- [ ] `first-time-setup` — guided setup on first run
- [ ] `changelog` — what's new since last version
- [ ] `login-dialog` — OAuth login
- [ ] `oauth-selector` — pick OAuth provider

### Other
- [ ] suspend/resume (Ctrl+Z → `kill -CONT`)
- [ ] debug key (Shift+Ctrl+D)
- [ ] keybinding hints in header (dynamic, based on context)

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
