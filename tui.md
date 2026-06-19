# rab TUI Library Design

This document plans the Rust port of pi-tui — a main-screen, diff-rendering terminal UI library built on crossterm. It separates the **core TUI library** (`src/tui/`) from **rab-specific UI** (`src/ui/`), mirroring how pi splits `@earendil-works/pi-tui` from the coding-agent's app components.

---

## Architecture Overview

```
┌──────────────────────────────────────────────────┐
│  src/ui/           rab-specific UI               │
│  ChatEditor, Messages, Footer, ModelSelector, …  │
│                                                  │
│  src/tui/          core TUI library              │
│  Component trait, diff renderer, text primitives │
│  Editor, SelectList, Loader, Key handling, Utils │
│                                                  │
│  crossterm         terminal I/O                  │
│  unicode-segmentation  grapheme clusters         │
│  unicode-width     character width               │
└──────────────────────────────────────────────────┘
```

`src/tui/` is generic and reusable. `src/ui/` is rab's app. There is no ratatui dependency.

---

## Component Catalog

### Tier 1: Core TUI Library (`src/tui/`)

#### Structural Primitives

| Component | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **Component** (trait) | `tui.ts:64` | `src/tui/component.rs` | Core trait: `render(width) -> Vec<String>`, `handle_input(key) -> bool`, `invalidate()` |
| **Focusable** (trait) | `tui.ts:104` | `src/tui/focusable.rs` | `focused: bool` — enables IME cursor marker emission |
| **Container** | `tui.ts:256` | `src/tui/container.rs` | Extends Component. `children: Vec<Box<dyn Component>>`, `add_child()`, `clear()`. Renders children vertically. |
| **Text** | `components/text.ts` (106 lines) | `src/tui/components/text.rs` | Multi-line text. Word wrapping at width, configurable padding. Optional background color function. |
| **TruncatedText** | `components/truncated-text.ts` (65 lines) | `src/tui/components/truncated_text.rs` | Text truncated to width with configurable ellipsis. |
| **Spacer** | `components/spacer.ts` (28 lines) | `src/tui/components/spacer.rs` | N empty lines of vertical space. |
| **Box** | `components/box.ts` (137 lines) | `src/tui/components/box.rs` | Container with padding and background color function. Children rendered offset inside the box. |

#### Interactive Components

| Component | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **Editor** | `components/editor.ts` (2,307 lines) | `src/tui/components/editor.rs` | **Full port.** Multi-line text editor. Emacs keybindings, word-wrap layout, grapheme-aware cursor, kill-ring (C-y/M-y), undo stack, paste-marker compaction, bracketed-paste handling, autocomplete integration, history recall (up/down), character jump, vertical scroll. Implements `Component + Focusable`. |
| **Input** | `components/input.ts` (447 lines) | `src/tui/components/input.rs` | **Full port.** Single-line text input. `> prompt text` layout. Horizontal scrolling, grapheme-aware cursor, kill-ring (C-w/C-u/C-k/C-y/M-y), undo stack, bracketed paste, `Focusable` (IME marker). Lighter than Editor — no word-wrap, no multi-line, no autocomplete, no character jump. Used by SettingsList for its search box. |
| **Loader** | `components/loader.ts` (92 lines) | `src/tui/components/loader.rs` | Animated spinner. Configurable frames, interval, message text. `start()`/`stop()`/`dispose()`. |
| **CancellableLoader** | `components/cancellable-loader.ts` (40 lines) | `src/tui/components/cancellable_loader.rs` | Loader subclass. Escape-to-cancel with `AbortSignal`. Shows cancel hint. |

#### Selection Components

| Component | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **SelectList** | `components/select-list.ts` (229 lines) | `src/tui/components/select_list.rs` | Scrollable list with fuzzy search. Items have label + optional description. Arrow nav, enter to select, esc to cancel. Themed highlighting. Uses `fuzzy_filter()` internally. |
| **SettingsList** | `components/settings-list.ts` (250 lines) | `src/tui/components/settings_list.rs` | **Full port.** Toggleable settings list. Each item has id, label, description, currentValue, optional `values[]` to cycle, optional submenu (opens a `Component`). Optional fuzzy search (uses `Input` internally). Enter/Space cycles values or opens submenu, Esc cancels. Themed label/value/description/cursor/hint. |

#### Editor Support Modules (core utilities)

| Module | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **KillRing** | `kill-ring.ts` (46 lines) | `src/tui/kill_ring.rs` | Ring buffer for Emacs kill/yank. `push(text, opts)`, `peek()`, `rotate()`, `len()`. Supports prepend/append accumulation for consecutive kills. |
| **UndoStack** | `undo-stack.ts` (28 lines) | `src/tui/undo_stack.rs` | Generic undo stack. `push(snapshot) -> ()`, `pop() -> Option<T>`, `clear()`. Editor snapshots its full state before each mutation. |
| **WordNav** | `word-navigation.ts` (117 lines) | `src/tui/word_nav.rs` | `find_word_backward(text, cursor, opts) -> usize`, `find_word_forward(text, cursor, opts) -> usize`. Handles word boundaries, CJK, paste markers. |

#### Core Infrastructure (non-Component)

| Module | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **Screen** | `tui.ts:doRender()` (~500 lines) | `src/tui/screen.rs` | The diff renderer. Maintains `prev_lines: Vec<String>`, computes changed ranges, emits minimal ANSI (cursor moves + line clears + new text). Handles resize, append, shrink. Wraps output in synchronized output. |
| **Terminal** | `terminal.ts` (531 lines) | `src/tui/terminal.rs` | Wraps crossterm: raw mode, event polling, resize, cursor hide/show, cursor positioning, line clear, synchronized output, window title. |
| **Key** | `keys.ts` (1,400 lines) | `src/tui/keys.rs` | Key identifiers (`Key::Enter`, `Key::Up`, `Key::Ctrl('c')`, `Key::CtrlShift('p')`). `matches_key(event, key) -> bool`. Wraps crossterm's `KeyEvent` — no Kitty protocol parsing needed. |
| **Util** | `utils.ts` (1,188 lines) | `src/tui/util.rs` | `visible_width(s) -> usize` (strip ANSI, measure Unicode). `truncate_to_width(s, w) -> String`. `wrap_text_with_ansi(s, w) -> Vec<String>`. `slice_by_column(s, start, end) -> String`. Also: `cjk_break_char(c)`, `is_whitespace_char(c)`. |
| **Fuzzy** | `fuzzy.ts` (137 lines) | `src/tui/fuzzy.rs` | `fuzzy_match(query, text) -> Option<FuzzyMatch>` with score and match positions. `fuzzy_filter(query, items) -> Vec<FuzzyMatch>`. |
| **Theme** | N/A (pi's theme is in coding-agent) | `src/tui/theme.rs` | Trait for colors. `fg(color: &str, text: &str) -> String`, `bg(color: &str, text: &str) -> String`. Concrete implementation in `src/ui/`. |

#### Deliberately Skipped (not needed for rab)

pi-tui components we are NOT porting:

| Component | Reason |
|---|---|
| Markdown | Rab doesn't render markdown in TUI |
| Image | No terminal image support needed |
| KeybindingsManager | Rab hard-codes keybindings (simpler, matches current approach) |
| StdinBuffer | crossterm's `event::read()` handles escape sequence parsing |
| TerminalImage, TerminalColors | Not needed |

---

### Tier 2: App-Specific UI (`src/ui/`)

These are rab's application components, built on `src/tui/` primitives. They are NOT part of the core TUI library.

| Component | Rust module | Purpose |
|---|---|---|
| **ChatEditor** | `src/ui/chat_editor.rs` | Thin wrapper around `tui::Editor`. Provides rab-specific behaviors: slash command list wired to rab's extension commands, file-path autocomplete (FD-based fallback), submission callback that feeds the agent loop, history persistence. ~200 lines. |
| **MessageList** | `src/ui/messages.rs` | Renders conversation history as styled text lines. Handles: user messages, assistant text, thinking blocks, tool calls, tool results, diff snippets. Respects `hide_thinking`, `collapse_tool_output`. |
| **WorkingIndicator** | `src/ui/working.rs` | Spinner shown during streaming. |
| **Footer** | `src/ui/footer.rs` | Two-line footer: cwd + git branch on line 1, token stats + model on line 2. |
| **ModelSelector** | `src/ui/model_selector.rs` | Full-screen overlay for picking a model. Uses `tui::SelectList`. Searchable. |
| **HelpOverlay** | `src/ui/help.rs` | `/help` display showing available commands and keybindings. |
| **Theme** | `src/ui/theme.rs` | rab's concrete color theme. Implements the `tui::Theme` trait. Port of current `src/theme.rs` but adapted for direct ANSI emission instead of ratatui `Style`. |
| **App** | `src/ui/app.rs` | Main event loop and state. Owns the `tui::Screen`, composes the component tree each tick, dispatches input to focused component, handles agent events (streaming deltas → message list). |

### Pi Reference: Where App Components Live in pi

```
packages/tui/src/                    ← @earendil-works/pi-tui (core library)
├── components/
│   ├── text.ts                      Tier 1: Text
│   ├── spacer.ts                    Tier 1: Spacer
│   ├── box.ts                       Tier 1: Box
│   ├── loader.ts                    Tier 1: Loader
│   ├── cancellable-loader.ts        Tier 1: CancellableLoader
│   ├── select-list.ts               Tier 1: SelectList
│   ├── settings-list.ts             Tier 1: (skipped)
│   ├── editor.ts                    Tier 1: Editor (FULL PORT)
│   ├── input.ts                     Tier 1: (skipped — Editor subsumes)
│   ├── markdown.ts                  Tier 1: (skipped)
│   ├── image.ts                     Tier 1: (skipped)
│   └── truncated-text.ts           Tier 1: TruncatedText
├── tui.ts                           Screen, Component, Container, Focusable
├── terminal.ts                      Terminal
├── keys.ts                          Key
├── utils.ts                         Util
├── fuzzy.ts                         Fuzzy
├── kill-ring.ts                     KillRing
├── undo-stack.ts                    UndoStack
├── word-navigation.ts               WordNav
└── ...

packages/coding-agent/src/modes/interactive/components/
├── bordered-loader.ts               Tier 2: BorderedLoader (→ src/ui/loading.rs)
├── dynamic-border.ts                Tier 2: (→ tui theme border fn)
├── assistant-message.ts             Tier 2: MessageList
├── model-selector.ts                Tier 2: ModelSelector
├── session-selector.ts              Tier 2: (not needed)
├── settings-selector.ts             Tier 2: (not needed)
├── tree-selector.ts                 Tier 2: (not needed)
└── ...
```

---

## File Structure Plan

```
src/
├── tui/                             # Core TUI library (generic, reusable)
│   ├── mod.rs                       # Re-exports, module declarations
│   ├── component.rs                 # Component trait
│   ├── focusable.rs                 # Focusable trait, CURSOR_MARKER constant
│   ├── container.rs                 # Container struct
│   ├── screen.rs                    # Diff renderer (the heart — ~400 lines)
│   ├── terminal.rs                  # Crossterm wrapper: raw mode, events, cursor
│   ├── keys.rs                      # Key identifiers, matches_key()
│   ├── util.rs                      # ANSI-aware width, wrap, truncate, slice
│   ├── fuzzy.rs                     # Fuzzy matching/filtering
│   ├── theme.rs                     # Theme trait (fg, bg color functions)
│   ├── kill_ring.rs                 # KillRing — Emacs kill/yank ring buffer
│   ├── undo_stack.rs                # UndoStack — generic undo
│   ├── word_nav.rs                  # Word boundary navigation
│   │
│   └── components/                  # Built-in Component impls
│       ├── mod.rs
│       ├── text.rs                  # Text
│       ├── truncated_text.rs        # TruncatedText
│       ├── spacer.rs                # Spacer
│       ├── box.rs                   # Box
│       ├── loader.rs                # Loader
│       ├── cancellable_loader.rs    # CancellableLoader
│       ├── select_list.rs           # SelectList
│       ├── settings_list.rs         # SettingsList
│       ├── input.rs                 # Input (single-line)
│       └── editor.rs                # Editor (multi-line, full pi-tui port)
│
├── ui/                              # Rab-specific UI (app layer)
│   ├── mod.rs
│   ├── app.rs                       # Main event loop, App state, run()
│   ├── chat_editor.rs               # ChatEditor — rab-specific Editor wrapper
│   ├── messages.rs                  # MessageList — renders conversation
│   ├── working.rs                   # WorkingIndicator — spinner during streaming
│   ├── footer.rs                    # Footer — cwd, git branch, tokens, model
│   ├── model_selector.rs            # ModelSelector — full-screen model picker
│   ├── help.rs                      # HelpOverlay — /help display
│   └── theme.rs                     # RabTheme — concrete color theme
│
├── lib.rs                           # pub mod tui; pub mod ui;
├── main.rs                          # CLI entry point
├── adapter.rs                       # (existing — unchanged)
├── agent.rs                         # (existing — unchanged)
├── auth.rs                          # (existing — unchanged)
├── builtin/                         # (existing — unchanged)
├── extension.rs                     # (existing — unchanged)
├── provider.rs                      # (existing — unchanged)
├── session.rs                       # (existing — unchanged)
├── settings.rs                      # (existing — unchanged)
└── types.rs                         # (existing — unchanged)

src/rattui/                          # DELETE — replaced by src/tui/ + src/ui/
```

Note: the old `src/theme.rs` (171 lines, ratatui `Style`-based) is deleted — colors move to `src/ui/theme.rs` as direct ANSI emission functions.

---

## Dependency Changes in Cargo.toml

```diff
- ratatui = "0.30"
+ crossterm = "0.28"
  unicode-segmentation = "1"   # keep — needed for grapheme-aware editor cursor
```

Keep `unicode-segmentation` — the editor needs `UnicodeSegmentation::graphemes()` for correct cursor movement through emoji, combining characters, and CJK characters. `unicode-width` alone only measures display width, it doesn't iterate grapheme clusters.

Add `unicode-width` for the Util module's `visible_width()`:

```diff
+ unicode-width = "0.2"
```

---

## Component Trait Design

```rust
// src/tui/component.rs

use crossterm::event::KeyEvent;

/// Every renderable UI element.
pub trait Component {
    /// Render to lines for the given viewport width.
    /// Each returned string MUST NOT exceed `width` in visible width.
    fn render(&self, width: usize) -> Vec<String>;

    /// Handle keyboard input. Return `true` if consumed.
    fn handle_input(&mut self, _key: &KeyEvent) -> bool { false }

    /// Clear cached render state. Called on theme changes or resize.
    fn invalidate(&mut self) {}

    /// Whether this component wants focus (for IME cursor positioning).
    fn is_focusable(&self) -> bool { false }
}

/// Components that display a text cursor and need IME support.
pub trait Focusable: Component {
    fn set_focused(&mut self, focused: bool);
    fn focused(&self) -> bool;
}

/// Zero-width APC sequence marking cursor position for IME.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";
```

---

## Editor: Full pi-tui Port

The Editor is pi-tui's most complex component (2,307 lines). It is ported in full to `src/tui/components/editor.rs`.

### Internal State

```rust
struct EditorState {
    lines: Vec<String>,     // logical lines (no wrapping)
    cursor_line: usize,     // logical line index
    cursor_col: usize,      // byte offset into lines[cursor_line]
}
```

### Public API

```rust
impl Editor {
    // -- Construction --
    fn new(tui: &TUI, theme: &EditorTheme) -> Self;
    fn set_autocomplete_provider(&mut self, provider: Box<dyn AutocompleteProvider>);

    // -- Content --
    fn get_text(&self) -> String;             // lines joined by \n
    fn get_expanded_text(&self) -> String;    // paste markers expanded
    fn get_lines(&self) -> &[String];
    fn get_cursor(&self) -> (usize, usize);   // (line, col)
    fn set_text(&mut self, text: &str);
    fn insert_text_at_cursor(&mut self, text: &str);

    // -- History --
    fn add_to_history(&mut self, text: &str);

    // -- Callbacks --
    fn set_on_submit(&mut self, cb: Box<dyn Fn(String)>);
    fn set_on_change(&mut self, cb: Box<dyn Fn(&str)>);
    fn set_disable_submit(&mut self, disabled: bool);

    // -- Appearance --
    fn set_padding_x(&mut self, padding: usize);
    fn set_border_color(&mut self, color: fn(&str) -> String);

    // -- Autocomplete query --
    fn is_showing_autocomplete(&self) -> bool;
}
```

### Keybindings (Emacs-style, hard-coded)

| Binding | Action |
|---|---|
| Enter | Submit (unless `\` prefix, then literal newline) |
| Shift+Enter | Literal newline |
| Ctrl+C | Let parent handle (abort/exit) |
| Ctrl+Z | Undo |
| Ctrl+Y | Yank (paste from kill ring) |
| Alt+Y | Yank-pop (cycle kill ring after yank) |
| Tab | Trigger completion (slash-command, file, symbol) |
| Escape | Cancel autocomplete if open |
| Up | Move cursor up / history recall (at first visual line) |
| Down | Move cursor down / history recall (at last visual line) |
| Left / Ctrl+B | Move cursor left (grapheme-aware) |
| Right / Ctrl+F | Move cursor right (grapheme-aware) |
| Ctrl+Left / Alt+B | Move to previous word start |
| Ctrl+Right / Alt+F | Move to next word start |
| Home / Ctrl+A | Move to line start |
| End / Ctrl+E | Move to line end |
| PageUp | Scroll page up |
| PageDown | Scroll page down |
| Backspace / Ctrl+H | Delete grapheme before cursor (grapheme-aware) |
| Delete / Ctrl+D | Delete grapheme at cursor |
| Ctrl+W | Delete word backward (kill) |
| Alt+D | Delete word forward (kill) |
| Ctrl+U | Delete to line start (kill) |
| Ctrl+K | Delete to line end (kill) |
| Ctrl+T | Character jump forward (type char to jump to) |
| Ctrl+Shift+T | Character jump backward |
| Shift+Space | Insert literal space |

### Render Layout

```
─── ↑ 2 more ────────────────────   ← top border (scroll indicator if scrolled)
│                                   ← left padding (padding_x spaces)
│  the text cursor is here█more     ← content area (width - 2*padding_x)
│                                   ← right padding
─── ↓ 1 more ─────────────────────   ← bottom border (scroll indicator)
│  autocomplete item 1              ← autocomplete dropdown (below border)
│> autocomplete item 2              ← selected
│  autocomplete item 3
```

The editor computes a **visual line map** from logical lines + word-wrap. It renders only the visible viewport (max 30% of terminal height), with scroll indicators on the borders. The cursor is rendered as an inverted character (`\x1b[7m...\x1b[0m`). When focused, `CURSOR_MARKER` is emitted before the fake cursor for IME positioning.

### Porting Breakdown (2,307 lines → ~1,450 Rust)

| Subsystem | TS lines | Rust est. | Key differences |
|---|---|---|---|
| State + cursor movement | ~300 | ~250 | `UnicodeSegmentation::graphemes()` replaces `Intl.Segmenter` |
| Word-wrap layout | ~150 | ~150 | Direct port; same algorithm |
| Render (visual line map + scroll) | ~200 | ~180 | `Vec<String>` output, no JSX-style composition |
| Input dispatch | ~200 | ~200 | `match` on crossterm `KeyEvent` instead of `matchesKey()` |
| Text mutations (insert, delete, newline) | ~150 | ~120 | Same logic, Rust borrow checker will need care |
| Kill ring + yank/yank-pop | ~80 | ~80 | `Vec<String>` ring with rotation |
| Undo stack | ~40 | ~30 | Generic `Vec<T>` with `pop()` |
| Word navigation | ~50 | ~50 | Direct port of `findWordBackward`/`findWordForward` |
| Character jump | ~40 | ~30 | Simpler in Rust (direct char search) |
| History navigation | ~80 | ~60 | Vec of strings with index |
| Paste handling (bracketed + markers) | ~120 | ~100 | `\x1b[200~...\x1b[201~` → `String` + marker logic |
| Autocomplete integration | ~300 | ~200 | Async → sync with debounce timer. `AutocompleteProvider` trait. |
| **Total** | **~2,300** | **~1,450** | |

Rust reduces line count because: no need for `isPasteMarker()` segmenter wrapping (grapheme iteration is simpler), no `structuredClone()` for undo (just clone the struct), no async autocomplete preamble (use `tokio::spawn` in the app layer).

### What Changes vs pi-tui

1. **Autocomplete is async-simplified.** pi-tui debounces autocomplete requests with `AbortController` and request tokens. In Rust, the app layer (`src/ui/app.rs`) spawns autocomplete tasks and the Editor just holds the current `AutocompleteSuggestions` result.

2. **No `Intl.Segmenter`.** We use `unicode-segmentation` crate for grapheme iteration and `unicode-width` for display width.

3. **Paste markers are simpler.** pi-tui uses them for >10-line or >1000-char pastes to avoid editor slowdown. Same logic, but stored in a `HashMap<usize, String>` (paste ID → content).

4. **No global `getKeybindings()`.** Keybindings are hard-coded in the `handle_input()` match statement. Simple, fast, no config parsing needed.

5. **Theme passed at construction.** Not pulled from a global.

---

## Input: Single-Line Text Input

Port of `components/input.ts` (447 lines → ~300 Rust). Lives at `src/tui/components/input.rs`.

### Difference from Editor

| Aspect | Input | Editor |
|---|---|---|
| Lines | Single line only | Multi-line |
| Rendering | Horizontal scroll within `> prompt text` | Vertical scroll, word-wrap layout, border frames |
| Newline | Submits (or ignored, depending on parent) | Inserts literal newline |
| Autocomplete | None | Full autocomplete integration |
| History | None (parent manages if needed) | Built-in up/down history recall |
| Character jump | None | Ctrl+T jump-to-char |
| Paste markers | None (always inline) | Compaction for >10 line pastes |
| Kill ring | Yes (C-w, C-u, C-k, C-y, M-y) | Yes |
| Undo | Yes | Yes |
| Focusable | Yes | Yes |

### Public API

```rust
impl Input {
    fn new() -> Self;
    fn get_value(&self) -> &str;
    fn set_value(&mut self, value: &str);
    fn set_on_submit(&mut self, cb: Box<dyn Fn(String)>);
    fn set_on_escape(&mut self, cb: Box<dyn Fn()>);
}
```

### Keybindings

Same Emacs-style deletions and cursor movement as Editor, minus multi-line operations:

| Binding | Action |
|---|---|
| Enter | Submit (calls `on_submit`) |
| Escape | Cancel (calls `on_escape`) |
| Ctrl+Z | Undo |
| Ctrl+Y | Yank |
| Alt+Y | Yank-pop |
| Ctrl+W | Delete word backward (kill) |
| Ctrl+U | Delete to start (kill) |
| Ctrl+K | Delete to end (kill) |
| Alt+D | Delete word forward (kill) |
| Backspace | Delete grapheme before cursor |
| Delete | Delete grapheme at cursor |
| Left/Right | Move cursor (grapheme-aware) |
| Ctrl+Left/Right | Move by word |
| Home/End | Move to start/end |

### Render Layout

```
> visible text█padding...
```

Horizontal scrolling: when text exceeds available width, the visible window follows the cursor (centered when possible). The cursor character is rendered with inverse video (`\x1b[7m`). When focused, `CURSOR_MARKER` is emitted before the fake cursor for IME positioning.

### Porting Notes

- 447 TS lines → ~300 Rust lines. Simpler than Editor because no word-wrap, no visual line map, no autocomplete.
- Shares `KillRing`, `UndoStack`, `WordNav` with Editor.
- Uses `unicode-segmentation` for grapheme-aware cursor and backspace.
- Uses `slice_by_column()` from `util.rs` for horizontal scroll window.
- Bracketed paste: strips newlines and tabs (→ 4 spaces), inserts inline.

---

## SettingsList: Toggleable Settings Picker

Port of `components/settings-list.ts` (250 lines → ~200 Rust). Lives at `src/tui/components/settings_list.rs`.

### Purpose

A scrollable list of labeled settings where each item can:
- **Cycle values**: Press Enter/Space to advance through `values[]` (e.g., on/off, light/dark)
- **Open a submenu**: Press Enter to open a child `Component` that fully takes over rendering and input
- **Show a description**: The selected item's description renders below the list

Each item has: `id`, `label`, `description?`, `currentValue`, `values[]?`, `submenu?`.

### Public API

```rust
struct SettingItem {
    id: String,
    label: String,
    description: Option<String>,
    current_value: String,
    values: Option<Vec<String>>,           // cycle through on Enter
    submenu: Option<SubmenuFactory>,        // open nested Component on Enter
}

impl SettingsList {
    fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: &SettingsListTheme,
        on_change: Box<dyn Fn(&str, &str)>,  // (id, new_value)
        on_cancel: Box<dyn Fn()>,
        options: SettingsListOptions,
    ) -> Self;
    fn update_value(&mut self, id: &str, new_value: &str);
}
```

### Render Layout

```
> search query                      ← optional search Input (if enableSearch)
                                    ← blank line
  Label One              off        ← unselected item
> Label Two              on         ← selected item (cursor prefix, highlighted)
  Label Three            auto       ←
  (2/5)                             ← scroll indicator
                                    ←
  Description of selected item...   ← wrapped description
                                    ←
  Enter/Space to change · Esc to cancel  ← hint line
```

When a submenu is active, the submenu `Component` takes over completely — `render()` and `handleInput()` both delegate to it. The submenu receives `done(selectedValue?)` to close itself.

### Keybindings

| Binding | Action |
|---|---|
| Up/Down | Move selection |
| Enter / Space | Activate item (cycle value or open submenu) |
| Escape | Cancel (close list or close submenu if open) |
| Printable chars | Type into search box (if `enableSearch`) |

### Submenu Pattern

The submenu factory receives the current value and a `done` callback:

```rust
type SubmenuFactory = Box<dyn Fn(String, Box<dyn Fn(Option<String>)>) -> Box<dyn Component>>;
```

This allows a SettingsList item to open an arbitrary Component (e.g., a SelectList for model choice) inline. When the user picks or cancels, `done(Some(new_value))` or `done(None)` is called, the submenu closes, and selection returns to the parent item.

### Porting Notes

- 250 TS lines → ~200 Rust lines. Straightforward port.
- The `Input` component is used for the search box when `enableSearch` is true.
- `fuzzy_filter()` from `fuzzy.rs` handles search matching against item labels.
- Submenu uses trait objects (`Box<dyn Component>`) — the factory pattern maps naturally to Rust closures.

---

## Core Diff Renderer Design

```rust
// src/tui/screen.rs

pub struct Screen {
    prev_lines: Vec<String>,
    prev_width: u16,
    prev_height: u16,
    cursor_row: usize,
    // ...
}

impl Screen {
    /// Compare `new_lines` to the previous frame and emit minimal
    /// ANSI to update the terminal. Returns the new hardware cursor row.
    pub fn render(&mut self, new_lines: Vec<String>) -> io::Result<()> {
        // 1. Width/height changed? → full redraw (clear + re-render all)
        // 2. First render? → write all without clearing
        // 3. Content shrunk? → full redraw (clear empty rows)
        // 4. Find first_changed / last_changed indices
        // 5. New lines appended (streaming)? → \r\n for each new line
        // 6. Move cursor to first_changed, [2K-clear, write line] for each
        // 7. Extract CURSOR_MARKER, position hardware cursor for IME
        // 8. Wrap in \x1b[?2026h/l synchronized output
    }
}
```

The algorithm is a direct port of `TUI.doRender()` from `tui.ts` (lines ~1050-1570), minus overlay compositing and Kitty image logic.

---

## Key Design Decisions

1. **No async in Component trait.** `render()` and `handle_input()` are synchronous. Async lives in the app event loop (`src/ui/app.rs`), which feeds events to components and triggers re-renders.

2. **Components own their state.** No global state. `Editor` owns its text buffer, cursor, history. `SelectList` owns its items, selection index, search query. `Loader` owns its frame counter.

3. **Width is passed in, not stored.** Every `render(width)` call receives the current viewport width. Components cache their output for the given width and invalidate on state change.

4. **Theme is a trait, not a global.** Components accept theme via constructor or setter. The app layer provides the concrete theme.

5. **No overlay stack in core library.** pi-tui's overlay compositing adds ~600 lines of complexity to the diff renderer. For rab, overlays (model selector, help) are implemented as full-screen component swaps in the app event loop — much simpler.

6. **Line-level diffing, not cell-level.** pi-tui compares strings. ratatui compares `Cell` structs (char + style). Line-level is simpler and sufficient for a chat UI where most changes are full-line replacements or appends.

7. **Editor lives in tui/components/ not ui/.** The Editor is a general-purpose component (like Text or SelectList). rab's app wrap it with `ChatEditor` in `src/ui/chat_editor.rs` for app-specific behavior (slash commands, file paths, submission hook).

8. **Input is separate from Editor.** The `Input` component provides single-line text entry with horizontal scrolling. It is lighter than `Editor` (no word-wrap, no multiline, no autocomplete, no character jump) but shares kill-ring, undo-stack, grapheme-aware cursor, and `Focusable` support. `SettingsList` uses `Input` for its search box.
