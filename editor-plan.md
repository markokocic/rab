# Editor Parity Plan: rab → pi 1/1

Comprehensive gap analysis between rab's TUI editor/autocomplete/input
and pi's reference implementation (`packages/tui/src/`).

---

## 🔴 CRITICAL (non-functional — user-facing bugs)

### 1. Paste Completely Broken

| Aspect | pi | rab |
|--------|----|-----|
| Input pipeline | `StdinBuffer` detects bracketed paste `\x1b[200~…\x1b[201~`, emits `paste` event, terminal rewraps with markers and sends through `handleInput` | `poll_key_event` in `terminal.rs` discards `Event::Paste` — returns `Ok(None)` for any non-`Key`, non-`Resize` event |
| Editor entry | `handleInput` sees `\x1b[200~`, buffers, calls `handlePaste` | `Editor::handle_paste` exists but is NEVER called |

**Fix**: Handle `Event::Paste` from crossterm in the input loop, route to `Editor::handle_paste` (or wrap with bracketed markers and let Editor's existing detection handle it).

**Files**: `src/tui/terminal.rs` (`poll_key_event`, `read_key_event`), `src/agent/ui/app.rs` (event loop), `src/tui/components/editor.rs` (paste delivery path).

---

### 2. @ Autocomplete Broken

| Aspect | pi | rab |
|--------|----|-----|
| File search | `fd` binary — fast, respects `.gitignore`, fuzzy across project | `std::fs::read_dir` only — slow, no `.gitignore`, directory-only |
| Fuzzy matching | `fuzzyFilter` + `scoreEntry` — scores by exact/starts/contains | `starts_with` only |
| Quoted prefix | `@"path with spaces"` supported | Not supported (no `@"` parser) |
| Debounce | 20ms for attachment `@` | No debounce |
| Trigger on letters in @ ctx | Checks `autocompleteTriggerPattern` regex: `(?:^\|[\s])[@#]\S*$` | Checks `text_before.contains('@') \|\| text_before.contains('#')` — simpler but misses edge cases |

**Fix**: Add `fd` detection + `walkDirectoryWithFd` equivalent, implement fuzzy file search, support `@"` quoted prefix, add debounce timer.

**Files**: `src/tui/autocomplete.rs` (`CombinedAutocompleteProvider`), `src/tui/components/editor.rs` (`update_autocomplete`).

---

### 3. / Autocomplete Behavior Differences

| Aspect | pi | rab |
|--------|----|-----|
| Argument completion | `SlashCommand.getArgumentCompletions(prefix)` | Not supported |
| Matching | `fuzzyFilter` for names | Case-insensitive `starts_with` |
| Layout | `SLASH_COMMAND_SELECT_LIST_LAYOUT` (min 12, max 32) | Default SelectList layout |
| After Enter select | Clear autocomplete + submit | Same (correct) |

**Fix**: Add `SlashCommand::argument_completions` trait, add fuzzy matching for slash names, add special layout.

**Files**: `src/tui/autocomplete.rs` (`SlashCommand`), `src/tui/components/editor.rs` (autocomplete layout).

---

## 🟡 EDITOR COMPONENT (functional gaps — causes visual/UX bugs)

### 4. Word Wrapping Algorithm

| Aspect | pi | rab |
|--------|----|-----|
| Function | `wordWrapLine` — paste-marker aware via `segmentWithMarkers`, CJK break support, word-boundary backtracking | `wrap_text_with_ansi` + `wrap_single_line` + `split_into_tokens` |
| Paste marker awareness | Merges paste markers into atomic segments so they never split across visual lines | No awareness — `[paste #1` can wrap mid-marker |
| Output shape | Returns `TextChunk[]` with `startIndex`/`endIndex` for cursor mapping | Returns `Vec<String>` — cursor mapping done separately by `layout_text` via `visual_col_to_byte_offset` |

**Impact**: Likely root cause of **line duplication** and **wrapping artifacts** users report. The two algorithms produce fundamentally different chunk boundaries.

**Fix**: Port `segmentWithMarkers` + `wordWrapLine` from pi to Rust, or make `wrap_text_with_ansi` paste-marker aware and return structured chunks with byte offsets.

**Files**: `src/tui/components/editor.rs` (`layout_text`), `src/tui/util.rs` (`wrap_text_with_ansi`).

---

### 5. Paste Markers Not Atomic in Cursor Movement

| Aspect | pi | rab |
|--------|----|-----|
| Cursor left/right | `segment(text, "grapheme")` — paste-marker aware via `segmentWithMarkers`, treats `[paste #1]` as one grapheme | Direct `grapheme_indices(true)` — cursor can land in the middle of `[paste #1]` |
| Word navigation | `findWordForward`/`findWordBackward` receive `{ isAtomicSegment: isPasteMarker }` | `find_word_forward_with`/`find_word_backward_with` support `is_atomic_segment` but Editor never passes it |

**Fix**: Make `move_left`/`move_right` skip paste marker boundaries, pass `is_atomic_segment` to word navigation calls.

**Files**: `src/tui/components/editor.rs` (`move_left`, `move_right`, `delete_word_backward`, `delete_word_forward`).

---

### 6. Sticky Column Vertical Movement

| Aspect | pi | rab |
|--------|----|-----|
| Decision table | Full P/S/T/U table with 7 scenarios | `preferred_col.unwrap_or(cursor_col).min(target_len)` |
| Snap-to-segment | `snappedFromCursorCol` + multi-visual-line resolution for paste markers | Not implemented |
| Preferred column tracking | `preferredVisualCol` set/cleared per decision-table rules | Simple state, no decision table |

**Fix**: Implement the decision table from pi (`computeVerticalMoveColumn`), add snap-to-segment for atomic segments.

**Files**: `src/tui/components/editor.rs` (`move_vertical`, `move_up`, `move_down`).

---

### 7. Max Visible Lines

| Aspect | pi | rab |
|--------|----|-----|
| Formula | `Math.max(5, Math.floor(terminalRows * 0.3))` — dynamic per terminal height | Fixed `max_visible_lines` from `EditorOptions` (default 10) |

**Fix**: Compute dynamically from TUI terminal height.

**Files**: `src/tui/components/editor.rs` (`render`).

---

### 8. Autocomplete Pre-select Best Match

| Aspect | pi | rab |
|--------|----|-----|
| Pre-select | `getBestAutocompleteMatchIndex`: exact match first, then prefix match | Always starts at index 0 |

**Fix**: Implement `get_best_autocomplete_match_index` equivalent.

**Files**: `src/tui/components/editor.rs` (`set_autocomplete` / `trigger_autocomplete`).

---

### 9. Cursor Overflow into Padding

| Aspect | pi | rab |
|--------|----|-----|
| Behavior | When cursor is at end and `paddingX > 0`, cursor can overflow into right padding. Flags `cursorInPadding` and adjusts right padding. | No special handling — cursor stuck at content boundary |

**Fix**: In `render`, detect cursor-at-end overflow and allow it into padding area.

**Files**: `src/tui/components/editor.rs` (`render`).

---

### 10. CSI-u Decoding in Paste Path

| Aspect | pi | rab |
|--------|----|-----|
| Paste handling | Decodes `\x1b[106;5u` → Ctrl+J before filtering. Handles tmux/extended-key terminals that re-encode control bytes as CSI-u. | No CSI-u decoding — control bytes may be lost or leaked |

**Fix**: Add `decode_csi_u` function and apply it in `handle_paste` before filtering.

**Files**: `src/tui/components/editor.rs` (`handle_paste`).

---

### 11. Space Auto-insert Before Pasting Paths

| Aspect | pi | rab |
|--------|----|-----|
| Paste logic | If pasting `/~/.` and char before cursor is a word char, prepends ` ` for readability | No such logic |

**Fix**: Match pi's heuristic.

**Files**: `src/tui/components/editor.rs` (`handle_paste`).

---

### 12. Autocomplete Re-trigger After Backspace Dismissal

| Aspect | pi | rab |
|--------|----|-----|
| Backspace | After `handleBackspace`, if autocomplete was just dismissed, re-triggers if still in completable context | Only re-triggers if `autocomplete_active` is still true |

**Fix**: In `backspace`, check context and call `try_trigger_autocomplete` even when autocomplete was already dismissed.

**Files**: `src/tui/components/editor.rs` (`backspace`).

---

### 13. Autocomplete Debounce

| Aspect | pi | rab |
|--------|----|-----|
| Timing | 20ms for attachment `@` patterns (when `debouncePattern` matches), 0ms for slash commands | No debounce |

**Fix**: Add debounce timer (`setTimeout` equivalent).

**Files**: `src/tui/components/editor.rs` (`try_trigger_autocomplete`, `trigger_autocomplete`).

---

### 14. Autocomplete Dropdown: Slash Command Layout

| Aspect | pi | rab |
|--------|----|-----|
| Layout | `SLASH_COMMAND_SELECT_LIST_LAYOUT` (minPrimaryColumnWidth: 12, maxPrimaryColumnWidth: 32) | Default SelectList layout |

**Fix**: Pass special layout when prefix starts with `/`.

**Files**: `src/tui/components/editor.rs` (`autocomplete_list` creation).

---

### 15. `yankPop` Undo Safety

| Aspect | pi | rab |
|--------|----|-----|
| yankPop | Pushes current state to undo stack, deletes previous yank, rotates ring, inserts new text | Pops undo stack (may undo more than expected if user typed between yank and yank-pop) |

**Fix**: Push undo before deleting yanked text, don't pop.

**Files**: `src/tui/components/editor.rs` (`yank_pop`).

---

### 16. Word Navigation Without Atomic Segments

| Aspect | pi | rab |
|--------|----|-----|
| wordLeft/wordRight | Receives `{ segment: ..., isAtomicSegment: isPasteMarker }` in word nav options | `move_word_backward`/`move_word_forward` in Editor not passing `is_atomic_segment` to word nav |

**Fix**: Pass `is_atomic_segment` callback when calling word navigation.

**Files**: `src/tui/components/editor.rs` (`move_word_backward`, `move_word_forward`), `src/tui/word_nav.rs`.

---

## 🟠 INPUT COMPONENT

### 17. Bracketed Paste Unused in Input

| Aspect | pi | rab |
|--------|----|-----|
| Input | `Input.handleInput` detects `\x1b[200~`, buffers, calls `handlePaste` (strips newlines — single-line) | `paste_buffer` and `is_in_paste` fields exist with `#[allow(dead_code)]`, never wired |

**Fix**: Wire paste events to `Input::handle_paste` (same delivery as Editor).

**Files**: `src/tui/components/input.rs`.

---

### 18. Kill Ring Accumulation

| Aspect | pi | rab |
|--------|----|-----|
| Delete word/line | Passes `accumulate: this.lastAction === "kill"` so consecutive kills chain in ring | All kill operations pass `false` for accumulate (no chaining) |

**Fix**: Track `last_action == "kill"` and pass to `kill_ring.push`.

**Files**: `src/tui/components/input.rs`, `src/tui/components/editor.rs`.

---

### 19. `yankPop` in Input

| Aspect | pi | rab |
|--------|----|-----|
| yankPop | Rotates ring after deleting previous yanked text | Rotates then pops undo — different semantics |

**Fix**: Match pi's yankPop: push undo, delete, rotate, insert.

**Files**: `src/tui/components/input.rs`.

---

## 🟣 AUTOCOMPLETE PROVIDER

### 20. No `fd` Binary Integration

| Aspect | pi | rab |
|--------|----|-----|
| File search | Spawns `fd` with `--base-directory`, `--max-results`, `--type f/d`, `--follow`, `--hidden`, `--exclude .git`. Supports `--full-path` for scoped queries. Parses output. | `std::fs::read_dir` with basic filtering |
| Performance | Fast, respects .gitignore, finds files anywhere in project | Slow in large dirs, no .gitignore |
| Scoping | `resolveScopedFuzzyQuery` resolves `src/foo/` to base+query | Basic path splitting |

**Fix**: Implement `walk_directory_with_fd` equivalent using `std::process::Command`.

**Files**: `src/tui/autocomplete.rs` (`CombinedAutocompleteProvider`).

---

### 21. No Quote-Aware Prefix

| Aspect | pi | rab |
|--------|----|-----|
| `@"path"` | `extractQuotedPrefix` detects unclosed `"` or `@"`, returns full quoted prefix. `parsePathPrefix` splits into `isAtPrefix`, `isQuotedPrefix`, `rawPrefix`. `buildCompletionValue` closes quotes. | Not handled |

**Fix**: Add quote-parsing functions to `CombinedAutocompleteProvider`.

**Files**: `src/tui/autocomplete.rs`.

---

### 22. No Argument Completion

| Aspect | pi | rab |
|--------|----|-----|
| SlashCommand | Has `getArgumentCompletions?(argumentPrefix)` method | No argument completion concept |
| Example | `/model <TAB>` shows model names | Not possible |

**Fix**: Add `argument_completions` trait/field to `SlashCommand`.

**Files**: `src/tui/autocomplete.rs`.

---

## 🔵 AGENT-LEVEL CHAT EDITOR

### 23. Enter/Submit Flow (Escape Priority)

| Aspect | pi | rab |
|--------|----|-----|
| Escape handling | `CustomEditor.handleInput`: checks `app.interrupt` → if autocomplete active, delegates to `super.handleInput` (Editor cancels autocomplete). If not, fires `onEscape` or `actionHandlers.get("app.interrupt")` | `ChatEditor.handle_input`: checks `select.cancel` first → if autocomplete active, delegates to `editor.handle_input`. Then checks `app.escape`. Same result but different action constants |

**Status**: Functionally equivalent. No action needed unless action names diverge.

---

### 24. Paste Image Handler

| Aspect | pi | rab |
|--------|----|-----|
| Handler | `onPasteImage` callback, triggered by `app.clipboard.pasteImage` keybinding | Not implemented |

**Fix**: Add paste image support if needed.

**Files**: `src/agent/ui/chat_editor.rs`.

---

## ⚪ MINOR / COSMETIC

| # | Issue | pi | rab | Priority |
|---|-------|----|-----|----------|
| 25 | `setText` cursor placement | Has `cursorPlacement` param (`start`/`end`) | Always end | P4 |
| 26 | Dynamic editor height | 30% of terminal | Fixed 10 lines | P3 |
| 27 | Mouse support | `wantsKeyRelease`, mouse events | None | P5 |
| 28 | History dedup on add | Skips consecutive duplicates | `add_to_history` pushes unconditionally | P3 |

---

## Implementation Order

```
P0 ─────────────────────────────────────────────────────
 1. Paste delivery (terminal.rs + app.rs event loop)
 2. @ autocomplete: fd+provider fix (autocomplete.rs + editor.rs)

P1 ─────────────────────────────────────────────────────
 3. Word wrapping: port wordWrapLine (editor.rs + util.rs)
 4. Atomic paste markers in cursor move (editor.rs)
 5. Sticky column: decision table (editor.rs)

P2 ─────────────────────────────────────────────────────
 6. Autocomplete debounce (editor.rs)
 7. Autocomplete argument completion (autocomplete.rs)
 8. Autocomplete best-match pre-select (editor.rs)
 9. Slash command layout (editor.rs)
10. CSI-u paste decoding (editor.rs)
11. Space auto-insert before path paste (editor.rs)
12. Cursor overflow into padding (editor.rs)
13. yankPop undo safety (editor.rs + input.rs)

P3 ─────────────────────────────────────────────────────
14. Dynamic editor height (editor.rs)
15. Kill ring accumulation (editor.rs + input.rs)
16. Autocomplete re-trigger after backspace (editor.rs)
17. Quoted @ prefix (autocomplete.rs)
18. History dedup (editor.rs)
19. Word nav atomic segments (editor.rs)

P4 ─────────────────────────────────────────────────────
20. fd integration (autocomplete.rs)
21. setText cursor placement param (editor.rs)
22. Input bracketed paste wiring (input.rs)

P5 ─────────────────────────────────────────────────────
23. Mouse support (tui_core.rs + editor.rs)
24. Paste image (chat_editor.rs)
```
