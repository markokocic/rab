# Remaining work

## pi-tui alignment

Goal: architectural and behavioral 1/1 match with pi's `packages/tui/src/` on everything except images, Kitty protocol, and Markdown.

### Phase 1 — Core framework (TUI class, terminal, input pipeline)

- [x] **TUI struct** — `src/tui/tui_core.rs` wraps `Screen` with overlay compositing, focus routing, cursor marker extraction, hardware cursor positioning
  - [x] `TUI::new()` / `set_dimensions()` / `request_render()` / `is_dirty()` / `render()` / `finalize()`
  - [x] `render(lines, width, height, writer)` — composites overlays → extracts cursor marker → applies `SEGMENT_RESET` → delegates to `Screen` for diff
  - [x] `SEGMENT_RESET` (`\x1b[0m\x1b]8;;\x07`) appended after cursor marker extraction
  - [x] cursor marker extraction — find `CURSOR_MARKER` in visible viewport, strip, position hardware cursor
  - [x] synchronized output — delegated to Screen (already uses `\x1b[?2026h/l`)

- [x] **Overlay system** — `src/tui/overlay.rs` + compositing in `tui_core.rs`
  - [x] `show_overlay(component, options)` / `hide_overlay(id)` / `pop_overlay()` / `has_overlays()`
  - [x] overlay stack — `Vec<OverlayEntry>`, focus order counter, preFocus tracking
  - [x] anchor-based positioning — `OverlayAnchor` enum, `resolveAnchorRow`/`resolveAnchorCol`
  - [x] `SizeValue` — `Absolute(usize)` + `Percent(f64)` with `.resolve(reference)`
  - [x] margin parsing — `OverlayMargin` with per-side or `uniform()`
  - [x] `composite_overlays()` — pre-render all visible overlays sorted by focus order, composite at calculated positions
  - [x] `composite_line_at()` — single-pass overlay splice: `extract_segments()` → slice overlay → pad → `SEGMENT_RESET` → safety truncation

- [x] **Focus management**
  - [x] `set_focus(overlay_idx)` — track which overlay is focused
  - [x] `route_input(key)` — routes crossterm `KeyEvent` to focused overlay first, then non-capturing overlays

- [x] **Input pipeline (basic)**
  - [x] `route_input()` integrated into `app.rs` event loop — overlays get first crack at input before app `handle_input()`
  - [ ] `addInputListener` / `removeInputListener` — deferred to Phase 2 (needs StdinBuffer/Kitty protocol)

- [x] **Utility additions** — `src/tui/util.rs`
  - [x] `normalize_terminal_output(line)` — appends `\x1b[0m\x1b]8;;\x07`
  - [x] `extract_segments(line, before_end, after_start, after_len, strict)` — for overlay compositing
  - [x] `is_whitespace_char(grapheme)`

- [x] **Screen enhancements** — `src/tui/screen.rs`
  - [x] `prev_viewport_top()` accessor (needed by TUI for cursor positioning)
  - [x] `prev_width()` / `prev_height()` accessors

- [x] **Wired into app** — `src/agent/ui/app.rs`
  - [x] `TUI` replaces direct `Screen` usage
  - [x] `tui.route_input(&key)` called before `handle_input(&mut app, &key)`
  - [x] `tui.render(lines, width, height, writer)` instead of `screen.render()`
  - [x] `tui.finalize(writer)` instead of `screen.finalize()`

- [ ] **Terminal upgrades**
  - [ ] `Terminal` trait with `start(onInput, onResize)`, `stop()`, `drainInput()`, `write()`, `columns`/`rows`, `kittyProtocolActive`, `moveBy()`, `hideCursor()`/`showCursor()`, `clearLine()`/`clearFromCursor()`/`clearScreen()`, `setTitle()`, `setProgress()`
  - [ ] `ProcessTerminal` impl — crossterm-backed:
    - [ ] `PushKeyboardEnhancementFlags` with `DISAMBIGUATE_ESCAPE_CODES | REPORT_EVENT_TYPES | REPORT_ALTERNATE_KEYS` at startup
    - [ ] `PopKeyboardEnhancementFlags` at shutdown
    - [ ] bracketed paste mode via `\x1b[?2004h/l`
    - [ ] progress indicator — `\x1b]9;4;3\x07` with keepalive interval
    - [ ] `drainInput()` — timeout-based idle detection, pop keyboard enhancement before stopping
    - [ ] `kittyProtocolActive` — check `KeyEventKind` for release/repeat filtering
    - [ ] `setTitle()` — OSC 0
    - [ ] color scheme notifications — `\x1b[?2031h/l` (optional)
    - [ ] write logging — `PI_TUI_WRITE_LOG` (optional)
  - [ ] no StdinBuffer needed — crossterm handles event splitting
  - [ ] no manual CSI-u parsing needed — crossterm parses Kitty sequences into `KeyEvent` with `KeyEventKind`

### Phase 2 — Keys and keybindings

- [x] **Keys — string-based key IDs** — `src/tui/keys.rs`
  - [x] `key_event_to_id(event)` — converts `crossterm::KeyEvent` to pi-compatible key ID string (`"ctrl+c"`, `"shift+enter"`, `"alt+left"`, etc.)
  - [x] `match_key_id(event, key_id)` — matches a KeyEvent against a key ID string with relaxed modifier handling
  - [x] `parse_key_id(key_id)` — splits key ID into (key_name, ctrl, shift, alt, super)
  - [x] `matches_key_name(code, key_name)` — matches KeyCode against key name (Enter, Escape, F-keys, chars, etc.)
  - [x] full modifier support: ctrl, shift, alt, super in all combinations
  - [x] `is_key_release(event)` / `is_key_repeat(event)` — use `KeyEventKind` from crossterm
  - [x] `decode_kitty_printable` — use `key_event_to_string` (crossterm already decodes CSI-u)
  - [x] raw terminal data parsing — no longer needed (crossterm handles it)

- [x] **Keybindings system** — `src/tui/keybindings.rs`
  - [x] `Keybindings` struct — `HashMap<String, Vec<String>>` mapping action IDs to key ID lists
  - [x] `Keybindings::matches(event, action_id)` — checks if event matches any bound key
  - [x] `Keybindings::with_defaults()` — pi-compatible default bindings
  - [x] `get_keybindings()` / `init_keybindings(kb)` — global `OnceLock` accessor
  - [x] `Keybindings::load()` / `save()` — JSON persistence
  - [x] action ID constants: `ACTION_EDITOR_*`, `ACTION_INPUT_*`, `ACTION_SELECT_*`, `ACTION_APP_*` (27 actions total)
  - [x] default bindings: 27 actions with ~40 key assignments

- [x] **Migration complete** — all components use `get_keybindings().matches(event, action_id)`:
  - [x] `editor.rs` — Editor (movement, deletion, yank, undo, page, escape, autocomplete)
  - [x] `input.rs` — Input (movement, deletion, yank, undo, submit, escape)
  - [x] `select_list.rs` — SelectList (up/down, confirm, cancel, search backspace)
  - [x] `settings_list.rs` — SettingsList (up/down, confirm, cancel, search toggle)
  - [x] `cancellable_loader.rs` — CancellableLoader (escape = cancel)
  - [x] `chat_editor.rs` — ChatEditor (escape, interrupt, exit, model, thinking, collapse, help, tab, submit, newline, history, page)
  - [x] `model_selector.rs` — ModelSelector (confirm, cancel)

### Phase 3 — Utility upgrades

- [x] **Add missing utilities**
  - [x] `normalizeTerminalOutput(line)` — append `\x1b[0m\x1b]8;;\x07` (reset + hyperlink close) after content
  - [x] `applyBackgroundToLine(line, width, bg_fn)` — pad to width, wrap in bg coloring
  - [x] `isImageLine(line)` — detect Kitty image sequences (always false stub for non-image)
  - [x] `extractSegments(line, beforeStart, beforeEnd, afterLen, strict)` — split line into before/after segments for overlay compositing
  - [x] `sliceWithWidth(text, start, len, strict)` — like `sliceByColumn` but returns `{ text, width }`
  - [x] `cjkBreakRegex` export — `CJK_BREAK_REGEX` regex pattern string
  - [x] `isWhitespaceChar(grapheme)` — single-char whitespace predicate
  - [x] width caching for non-ASCII strings (thread-local LRU cache, 512 entries)

- [x] **Word navigation — align with pi**
  - [x] `WordNavigationOptions` — custom segmenter + `isAtomicSegment` predicate
  - [x] `find_word_backward_with` / `find_word_forward_with` — options-aware versions
  - [x] `PUNCTUATION_CHARS` constant (exported as slice)

### Phase 4 — Component upgrades

- [ ] **Editor — align with pi** (deferred — largest remaining item)
  - [ ] paste-marker system
  - [ ] bracketed paste buffering
  - [ ] undo coalescing (fish-style)
  - [ ] sticky column, character jump, history draft
  - [ ] autocomplete auto-trigger
  - [ ] paste-marker-aware segmentation

- [x] **Input — align with pi**
  - [x] undo coalescing — `last_action: Option<&'static str>` with fish-style coalescing
  - [x] horizontal scroll with smart centering — half-width centering, cursor-at-end column reservation
  - [x] `handle_paste()` — paste handling method
  - [x] keybinding-based dispatch (Phase 2)

- [x] **SelectList — align with pi**
  - [x] `SelectListLayoutOptions` — `min_primary_column_width`, `max_primary_column_width`, `truncate_primary` callback
  - [x] primary column width calculation — clamp between min/max bounds, measure widest item
  - [x] two-column layout (value + description) when width > 40 and description exists
  - [x] `normalize_to_single_line()` for description
  - [x] `on_selection_change` callback
  - [x] `get_selected_item()` — return `Option<&SelectItem>`
  - [x] `set_filter()` — prefix-based filter
  - [x] keybinding-based dispatch

- [x] **SettingsList — align with pi**
  - [x] submenu support — `SettingItem.submenu` field
  - [x] `submenu_component` / `submenu_item_index` — delegate input to submenu when active
  - [x] two-column layout (label aligned left, value right) with max-label-width calculation
  - [x] description display for selected item (wrapped, padded)
  - [x] hint line at bottom (dynamic based on search enabled)
  - [x] keybinding-based dispatch

- [x] **Loader — align with pi**
  - [x] Frame/message color function fields (`spinner_color_fn`, `message_color_fn`)
  - [x] Timer-based animation via `start()`/`stop()` with `tick()` callback
  - [x] `render()` returns `["", ...]` — one blank line above for spacing
  - [x] `LoaderIndicatorOptions` (custom frames, interval)
  - [x] `render_indicator_verbatim` flag

- [x] **CancellableLoader — align with pi**
  - [x] `cancelled: bool` + `on_abort: Option<Box<dyn FnMut()>>`
  - [x] `handle_input()` — Escape via `ACTION_SELECT_CANCEL`
  - [x] `dispose()` — stop animation

- [x] **Box — align with pi**
  - [x] render cache structure (child_lines, width, bg_sample comparison)
  - [x] `applyBg()` uses `apply_background_to_line` utility

- [x] **Text — align with pi**
  - [x] render cache via `RefCell` (for &self render)
  - [x] empty/whitespace text returns `vec![]` (already correct)
  - [x] tabs → 3 spaces (already correct)

- [x] **TruncatedText — align with pi**
  - [x] `padding_x`, `padding_y` fields via `with_padding()` builder
  - [x] pad to full width with spaces
  - [x] only first line before newline is used
  - [x] vertical padding (empty lines above/below)
  - [x] cache via `RefCell`

### Phase 5 — Overlay-aware application layer

- [x] **Wire up overlay system in app**
  - [x] `HelpOverlay` and `ModelSelector` shown via `TUI.show_overlay()` instead of manual compose_ui early-return
  - [x] `compose_ui()` always returns base content — TUI composites overlays via `composite_overlays()`
  - [x] `handle_input()` takes `&mut TUI` for overlay management
  - [x] `tui.route_input(&key)` → if overlay consumed, app skips `handle_input()`
  - [x] `pop_overlay()` called when overlay doesn't consume the key (any-key-dismiss)
  - [ ] add `nonCapturing: true` for non-interactive overlays (future)

- [x] **Wire up keybinding system in app**
  - [x] `~/.rab/keybindings.json` loaded on startup via `Keybindings::load()`
  - [x] custom bindings merged with defaults via `Keybindings::merge()`
  - [x] schema: `{ "action.id": ["key1", "key2"] }` matching pi format
  - [x] all components already use `getKeybindings().matches()`

### Phase 6 — Terminal trait abstraction

- [ ] define `Terminal` trait matching pi's interface: `start(onInput, onResize)`, `stop()`, `drainInput()`, `write()`, `columns`/`rows`, `kittyProtocolActive`, `moveBy()`, `hideCursor()`/`showCursor()`, `clearLine()`/`clearFromCursor()`/`clearScreen()`, `setTitle()`, `setProgress()`
- [ ] `ProcessTerminal` impl — uses crossterm for everything (raw mode, event polling, keyboard enhancement flags, cursor ops, size, clear, bracketed paste)
- [ ] migrate app code (event loop) to depend on `Terminal` trait, not crossterm directly

## tools
- [ ] check tool execution modes in pi, parallel, sequence, ... and compare with rab

## chat editor
- [x] check if pi have separate editor and chat editor components. Where are they defined Does rab do the same? — rab mirrors pi: `Editor` (core tui) + `ChatEditor` (app wrapper, key dispatch via `InputAction` enum)
- [ ] file autocomplete

## Slash command autocomplete
- [ ] in pi, selector appears as soon as user types `/`, in rab you must type `/` + Tab
- [ ] selector for slash command has plain styling, should match pi both visually and behaviourally

## Review reusable TUI components
- [ ] review usage of reusable tui components in app layer (messages.rs, help.rs, footer.rs)
- [ ] assistant text should render markdown (bold, code, headings, links, quotes) with pi theme colors

## Built-in tools
- [x] review each builtin tool: check if behaviour and rendering matches pi 1/1
  - [x] bash - tail truncation (lines/bytes), cancel check, process group killing, timeout, full output saved to temp file
  - [x] read - line accumulation truncation, firstLineExceedsLimit, trimTrailingEmptyLines, formatSize, compact labels, cancel, prompt guidelines
  - [x] write - file mutation queue, cancel check, prompt guidelines
  - [x] edit - BOM, line ending (CRLF/LF), fuzzy matching, input normalization, diff output, better errors, prompt guidelines, file mutation queue

## Messages
- [x] review rendering for thinking messages.
  - [x] consistent italic + background + per-level colors between `render_messages` and `compose_ui`
  - [x] add `level` field to `DisplayMsg::Thinking` for per-level color support
  - [x] tests for thinking rendering in `messages.rs` (visible, hidden, per-level colors, blank lines, level mapping)
- [ ] check other message types

## Scrolling
- [ ] wire up mouse wheel events (crossterm MouseEvent) to scroll chat
- [ ] wire up Page Up/Down and arrow keys (when editor is not focused) to scroll chat
- [ ] add scrollbar or scroll indicators

## Visual polish
- [x] per-thinking-level colors (pi has 6 levels: off→xhigh)
  - [x] added `thinking_level_low`, `thinking_level_medium`, `thinking_level_high`, `thinking_level_xhigh` theme colors
  - [x] `thinking_level_color()` mapper function
  - [x] per-level colors used in both `render_messages` and `compose_ui`
- [ ] footer token display padding fix on narrow terminals
- [ ] tool call lines bold tool name (already done via theme.bold)

## Done
- [x] `system_prompt.rs` - AGENTS.md/CLAUDE.md loading, project context, SYSTEM.md, APPEND_SYSTEM.md
- [x] `context_files.rs` - context file discovery (ancestor walk)
- [x] `skills.rs` - load skills, format for prompt, `/skill:name` expansion
- [x] `--no-context-files`, `--system-prompt`, `--append-system-prompt` CLI flags
- [x] Startup resource listing (context files, skills) in welcome message

## Phase 1 remaining
- [ ] `adapter/genai.rs` - multiple backends (Anthropic, OpenAI, Google, Ollama)
- [ ] `compaction.rs` - context window compaction
- [ ] Hook pipeline - `before_tool_call`, `after_tool_call`, `CancellationToken`
- [ ] Steering / follow-up queues - runtime message injection
- [ ] Tool execution modes - sequential mode
- [ ] Compile-time user extensions - `--no-extensions` flag
- [ ] `~/.rab/models.json` - custom provider/model definitions
- [ ] Image support - read tool detects image files, multimodal payload
- [ ] `rab plugin new` - scaffold extension crate
