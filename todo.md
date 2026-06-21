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

- [ ] **Terminal upgrades** — deferred to Phase 2 (needs StdinBuffer/Kitty protocol)
  - [ ] `Terminal` trait with `start(onInput, onResize)`, `stop()`, `drainInput()`, `write()`, `columns`/`rows`, `kittyProtocolActive`, `moveBy()`, `hideCursor()`/`showCursor()`, `clearLine()`/`clearFromCursor()`/`clearScreen()`, `setTitle()`, `setProgress()`
  - [ ] `ProcessTerminal` impl — raw mode, bracketed paste, Kitty keyboard protocol negotiation (flags 1+2+4), modifyOtherKeys fallback, StdinBuffer integration
  - [ ] `StdinBuffer` — split batched input into individual sequences, forward as data events, re-wrap paste content
  - [ ] `drainInput()` — disable Kitty protocol first, flush trailing release events
  - [ ] progress indicator — `\x1b]9;4;3\x07` with keepalive interval
  - [ ] color scheme notifications — OSC 2031 h/l
  - [ ] cell size query — CSI 16 t
  - [ ] write logging — `PI_TUI_WRITE_LOG`
  - [ ] Windows VT input enablement (stub)
  - [ ] keyboard protocol negotiation sequence parsing (split-response handling via flush timer)

### Phase 2 — Keys and keybindings

- [x] **Keys — string-based key IDs** — `src/tui/keys.rs`
  - [x] `key_event_to_id(event)` — converts `crossterm::KeyEvent` to pi-compatible key ID string (`"ctrl+c"`, `"shift+enter"`, `"alt+left"`, etc.)
  - [x] `match_key_id(event, key_id)` — matches a KeyEvent against a key ID string with relaxed modifier handling
  - [x] `parse_key_id(key_id)` — splits key ID into (key_name, ctrl, shift, alt, super)
  - [x] `matches_key_name(code, key_name)` — matches KeyCode against key name (Enter, Escape, F-keys, chars, etc.)
  - [x] full modifier support: ctrl, shift, alt, super in all combinations
  - [ ] raw terminal data parsing (`matches_key_data`, `parse_key`, `is_key_release`, `is_key_repeat`, `decode_kitty_printable`, `decode_printable_key`) — deferred to Phase 7 (Kitty protocol integration)

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

- [ ] **Add missing utilities**
  - [x] `normalizeTerminalOutput(line)` — append `\x1b[0m\x1b]8;;\x07` (reset + hyperlink close) after content
  - [ ] `applyBackgroundToLine(line, width, bg_fn)` — pad to width, wrap in bg coloring
  - [ ] `isImageLine(line)` — detect Kitty image sequences (always false stub for non-image)
  - [x] `extractSegments(line, beforeStart, beforeEnd, afterLen, strict)` — split line into before/after segments for overlay compositing
  - [ ] `sliceWithWidth(text, start, len, strict)` — like `sliceByColumn` but returns `{ text, width }`
  - [ ] `cjkBreakRegex` export (for word-wrapping and word navigation)
  - [x] `isWhitespaceChar(grapheme)` — single-char whitespace predicate
  - [ ] width caching for non-ASCII strings (LRU cache, ~512 entries)

- [ ] **Word navigation — align with pi**
  - [ ] `WordNavigationOptions` — custom segmenter + `isAtomicSegment` predicate
  - [ ] `find_word_backward` / `find_word_forward` — use `Intl.Segmenter`-style word segmentation (punctuation boundaries, ASCII punctuation regex)
  - [ ] export `PUNCTUATION_REGEX` constant

### Phase 4 — Component upgrades

- [ ] **Editor — align with pi**
  - [ ] paste-marker system:
    - [ ] `pastes: HashMap<u32, String>`, `paste_counter: u32`
    - [ ] on large paste (>10 lines or >1000 chars): store content, insert `[paste #N +M lines]` marker
    - [ ] `expand_paste_markers(text)` on submit
    - [ ] `get_expanded_text()` for external editor
    - [ ] paste-marker-aware segmentation — `segment_with_markers()` merges marker graphemes into atomic units
  - [ ] bracketed paste handling in `handle_input` — buffer `\x1b[200~` … `\x1b[201~`, decode CSI-u-encoded control bytes, filter non-printables, prepend space for file paths
  - [ ] undo coalescing (pi fish-style):
    - [ ] consecutive word chars coalesce into one undo unit
    - [ ] `last_action: Option<"type_word" | "kill" | "yank">`
    - [ ] space captures state before itself (undo removes space + following word together)
  - [ ] sticky column for vertical movement — `preferred_visual_col: Option<usize>`, `snapped_from_cursor_col`, `compute_vertical_move_column()` decision table match
  - [ ] character jump mode — `jump_mode: Option<"forward" | "backward">`, await next printable char, `jump_to_char()`
  - [ ] page up/down — `page_scroll(delta)`, move cursor to first/last visible line after scroll
  - [ ] history draft — save pre-history state so Down after Up restores it exactly
  - [ ] autocomplete integration:
    - [ ] `AutocompleteProvider` trait (`get_suggestions`, `apply_completion`, `should_trigger_file_completion`)
    - [ ] auto-trigger on `/`, `@`, `#`, and trigger characters at token boundaries
    - [ ] auto-trigger on letter typing in slash command context (`/commandName` or after space in slash command)
    - [ ] auto-update on typing/backspace when autocomplete already active
    - [ ] cancel on navigation away from completable context
    - [ ] Tab triggers completion in non-autocomplete state
  - [ ] `snapped_from_cursor_col` — snap cursor to atomic segment boundaries (paste markers), resolve pre-snap position on next vertical move
  - [ ] `segment()` method — paste-marker-aware grapheme/word segmentation via `segment_with_markers()`
  - [ ] `wordWrapLine`-compatible layout with paste-marker-aware chunks
  - [ ] `normalize_text()` — `\r\n`/`\r` → `\n`, tabs → 4 spaces
  - [ ] keybinding-based input dispatch (use `getKeybindings().matches()`)
  - [ ] `border_color` — mutable public field for dynamic styling

- [ ] **Input — align with pi**
  - [ ] bracketed paste buffering — same as Editor but single-line
  - [ ] undo coalescing — `last_action` tracking per pi pattern
  - [ ] horizontal scroll with smart centering — `half_width` centering, reserve column for cursor at end
  - [ ] Kitty CSI-u printable decode — `decodeKittyPrintable` instead of control char filter
  - [ ] keybinding-based dispatch

- [ ] **SelectList — align with pi**
  - [ ] `SelectListLayoutOptions` — `min_primary_column_width`, `max_primary_column_width`, `truncate_primary` callback
  - [ ] primary column width calculation — clamp between min/max bounds, measure widest item
  - [ ] two-column layout (value + description) when width > 40 and description exists
  - [ ] `normalize_to_single_line()` for description
  - [ ] `on_selection_change` callback
  - [ ] `get_selected_item()` — return `SelectItem` (not just value string)
  - [ ] `set_filter()` — prefix-based filter (simpler than fuzzy for user-typed single char)
  - [ ] keybinding-based dispatch

- [ ] **SettingsList — align with pi**
  - [ ] submenu support — `SettingItem.submenu: Option<Box<dyn Fn(&str, Box<dyn Fn(Option<String>)>) -> Box<dyn Component>>>`
  - [ ] `submenu_component` / `submenu_item_index` — delegate all input to submenu when active
  - [ ] on submenu close via `done()`, restore selection to the item that opened it
  - [ ] two-column layout (label aligned left, value right) with max-label-width calculation
  - [ ] description display for selected item (wrapped, padded)
  - [ ] hint line at bottom (dynamic based on search enabled)
  - [ ] keybinding-based dispatch

- [ ] **Loader — align with pi**
  - [ ] Extend `Text` component instead of standalone struct
  - [ ] Frame/message color function fields (`spinner_color_fn`, `message_color_fn`)
  - [ ] Timer-based animation via `start()`/`stop()` with update callback
  - [ ] `render()` returns `["", ...super.render(width)]` — one blank line above for spacing
  - [ ] `indicator` field — `LoaderIndicatorOptions` (custom frames, interval)
  - [ ] `render_indicator_verbatim` flag — when custom frames provided, render without spinner color function

- [ ] **CancellableLoader — align with pi**
  - [ ] `AbortController`-style cancellation — `cancelled: bool`, `on_abort: Option<Box<dyn FnMut()>>`
  - [ ] `handle_input()` — check Escape via keybinding `tui.select.cancel`
  - [ ] `dispose()` — stop animation

- [ ] **Box — align with pi**
  - [ ] render cache — track `child_lines`, `width`, `bg_sample`, compare on render; invalidate on child add/remove/clear
  - [ ] `applyBg()` uses `applyBackgroundToLine` (new utility)

- [ ] **Text — align with pi**
  - [ ] render cache — `cached_text`, `cached_width`, `cached_lines`
  - [ ] empty/whitespace text returns `vec![]` (not `vec![""]`)
  - [ ] tabs → 3 spaces (not 4)

- [ ] **TruncatedText — align with pi**
  - [ ] add `padding_x`, `padding_y` fields
  - [ ] pad to full width with spaces
  - [ ] only first line before newline is used
  - [ ] vertical padding (empty lines above/below)

### Phase 5 — Overlay-aware application layer

- [ ] **Wire up overlay system in app**
  - [ ] migrate all modals/dialogs to use `TUI.showOverlay()` instead of manual compositing
  - [ ] verify focus restore works correctly when overlays are dismissed
  - [ ] add `nonCapturing: true` for non-interactive overlays (e.g. toasts/notifications)

- [ ] **Wire up keybinding system in app**
  - [ ] create `~/.rab/keybindings.json` schema (matching pi's format)
  - [ ] load/merge keybindings on startup
  - [ ] all components use `getKeybindings().matches()`

### Phase 6 — Terminal trait abstraction

- [ ] define `Terminal` trait matching pi's interface: `start(onInput, onResize)`, `stop()`, `drainInput()`, `write()`, `columns`/`rows`, `kittyProtocolActive`, `moveBy()`, `hideCursor()`/`showCursor()`, `clearLine()`/`clearFromCursor()`/`clearScreen()`, `setTitle()`, `setProgress()`
- [ ] `ProcessTerminal` impl — keeps crossterm for raw mode, cursor ops, size queries, clear operations; adds direct escape sequences for Kitty protocol, bracketed paste, progress indicator, etc. where crossterm has no API
- [ ] migrate app code (event loop, Screen) to depend on `Terminal` trait, not crossterm directly

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
