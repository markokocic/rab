# rab TUI Library Design

This document plans the Rust port of pi-tui - a main-screen, diff-rendering terminal UI library built on crossterm. It separates the **core TUI library** (`src/tui/`) from **rab-specific UI** (`src/agent/ui/`), mirroring how pi splits `@earendil-works/pi-tui` from the coding-agent's app components.

---

## Architecture Overview

```
┌──────────────────────────────────────────────────┐
│  src/agent/ui/     rab-specific UI               │
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

`src/tui/` is generic and reusable. `src/agent/ui/` is rab's app. There is no ratatui dependency.

---

## Component Catalog

### Tier 1: Core TUI Library (`src/tui/`) ✅ IMPLEMENTED

All Tier 1 components are implemented and tested. **662 tests pass** with zero warnings.

#### Structural Primitives

| Component | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **Component** (trait) | `tui.ts:64` | `src/tui/component.rs` (✅ 21 lines) | Core trait: `render(width) -> Vec<String>`, `handle_input(key) -> bool`, `invalidate()` |
| **Focusable** (trait) | `tui.ts:104` | `src/tui/focusable.rs` (✅ 12 lines) | `focused: bool` - enables IME cursor marker emission |
| **Container** | `tui.ts:256` | `src/tui/container.rs` (✅ 611 lines) | Extends Component. `children: Vec<Box<dyn Component>>`, `add_child()`, `clear()`. Renders children vertically. Manages overlay stack (show/hide/pop, compositing, focus routing). |
| **Text** | `components/text.ts` (106 lines) | `src/tui/components/text.rs` (✅ 142 lines) | Multi-line text. Word wrapping at width, configurable padding. Optional background color function. |
| **TruncatedText** | `components/truncated-text.ts` (65 lines) | `src/tui/components/truncated_text.rs` (✅ 72 lines) | Text truncated to width with configurable ellipsis. |
| **Spacer** | `components/spacer.ts` (28 lines) | `src/tui/components/spacer.rs` (✅ 38 lines) | N empty lines of vertical space. |
| **Box** | `components/box.ts` (137 lines) | `src/tui/components/box.rs` (✅ 113 lines) | Container with padding and background color function. Children rendered offset inside the box. |
| **DynamicLines** | — | `src/tui/components/dynamic_lines.rs` | Dynamically-sized section that returns a fixed number of lines. Used for pending text, status, queued messages, working indicator. |

#### Interactive Components

| Component | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **Editor** | `components/editor.ts` (2,307 lines) | `src/tui/components/editor.rs` (✅ 3170 lines) | **Full port + extras.** Multi-line text editor. Emacs keybindings, word-wrap layout, grapheme-aware cursor, kill-ring (C-y/M-y), undo stack, history recall (up/down), vertical scroll, autocomplete integration, paste markers, character jump. Implements `Component + Focusable`. |
| **Input** | `components/input.ts` (447 lines) | `src/tui/components/input.rs` (✅ 639 lines) | **Full port.** Single-line text input. `> prompt text` layout. Horizontal scrolling, grapheme-aware cursor, kill-ring (C-w/C-u/C-k/C-y/M-y), undo stack, `Focusable` (IME marker). |
| **Loader** | `components/loader.ts` (92 lines) | `src/tui/components/loader.rs` (✅ 109 lines) | Animated spinner. Configurable frames, interval, message text. `start()`/`stop()`/`tick()`. |
| **CancellableLoader** | `components/cancellable-loader.ts` (40 lines) | `src/tui/components/cancellable_loader.rs` (✅ 82 lines) | Loader with escape-to-cancel. Shows cancel hint. |

#### Selection Components

| Component | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **SelectList** | `components/select-list.ts` (229 lines) | `src/tui/components/select_list.rs` (✅ 520 lines) | Scrollable list with fuzzy search. Items have label + optional description. Arrow nav, enter to select, esc to cancel. Themed highlighting. Uses `fuzzy_filter()` internally. |
| **SettingsList** | `components/settings-list.ts` (250 lines) | `src/tui/components/settings_list.rs` (✅ 480 lines) | **Full port.** Toggleable settings list. Each item has id, label, description, currentValue, optional `values[]` to cycle. Optional fuzzy search (uses `Input` internally). Enter/Space cycles values, Esc cancels. |

#### Rendering & Markdown Components

| Component | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **Markdown** | — | `src/tui/components/markdown.rs` (✅ 2103 lines) | Full Markdown renderer using pulldown-cmark. Renders: headings, paragraphs, code blocks (with syntect syntax highlighting), inline code, tables, lists, blockquotes, thematic breaks. Not a pi port — rab-specific. |
| **Diff** | — | `src/tui/components/diff.rs` | Unified diff renderer. Colored +/- lines with intra-line character-level inverse highlighting. |
| **RcRefCellComponent** | — | `src/tui/components/rc_ref_cell_component.rs` | Wrapper for shared ownership components. Bridges `Rc<RefCell<dyn Component>>` into the `Component` trait. Used for in-place streaming updates. |

#### Editor Support Modules (core utilities)

| Module | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **KillRing** | `kill-ring.ts` (46 lines) | `src/tui/kill_ring.rs` (✅ 128 lines) | Ring buffer for Emacs kill/yank. `push(text, opts)`, `peek()`, `rotate()`, `len()`. Supports prepend/append accumulation for consecutive kills. |
| **UndoStack** | `undo-stack.ts` (28 lines) | `src/tui/undo_stack.rs` (✅ 73 lines) | Generic undo stack. `push(snapshot) -> ()`, `pop() -> Option<T>`, `clear()`. Editor snapshots its full state before each mutation. |
| **WordNav** | `word-navigation.ts` (117 lines) | `src/tui/word_nav.rs` (✅ 363 lines) | `find_word_backward(text, cursor) -> usize`, `find_word_forward(text, cursor) -> usize`. Handles word boundaries, CJK, punctuation segments. |

#### Core Infrastructure (non-Component)

| Module | pi-tui src | Rust module | Purpose |
|---|---|---|---|
| **Screen** | `tui.ts:doRender()` (~500 lines) | `src/tui/screen.rs` (✅ 787 lines) | The diff renderer. Maintains `prev_lines: Vec<String>`, computes changed ranges, emits minimal ANSI (cursor moves + line clears + new text). Handles resize, append, shrink. Viewport tracking, cursor marker extraction, hardware cursor positioning. Wraps output in synchronized output. |
| **Terminal** | `terminal.ts` (531 lines) | `src/tui/terminal.rs` (✅ 392 lines) | Wraps crossterm: raw mode, event polling, resize, cursor hide/show, cursor positioning, line clear, synchronized output. |
| **Key** | `keys.ts` (1,400 lines) | `src/tui/keys.rs` (✅ 652 lines) | Key identifiers (`Key::Enter`, `Key::Up`, `Key::Ctrl('c')`, `Key::CtrlAlt('p')`). `matches_key(event, key) -> bool`. Wraps crossterm's `KeyEvent`. |
| **Keybindings** | — | `src/tui/keybindings.rs` (✅ 398 lines) | 27+ action identifiers (e.g. `tui.editor.cursorLeft`), default key-to-action mappings, JSON config loading from `~/.rab/keybindings.json`. Constants for all editor, input, select list, and app-level actions. |
| **Util** | `utils.ts` (1,188 lines) | `src/tui/util.rs` (✅ 1142 lines) | `visible_width(s) -> usize` (strip ANSI, measure Unicode). `truncate_to_width(s, w) -> String`. `wrap_text_with_ansi(s, w) -> Vec<String>`. `slice_by_column(s, start, end) -> String`. |
| **Fuzzy** | `fuzzy.ts` (137 lines) | `src/tui/fuzzy.rs` (✅ 263 lines) | `fuzzy_match(query, text) -> FuzzyMatch` with score and match positions. `fuzzy_filter(query, items) -> Vec<usize>`. Supports swapped alphanumeric tokens. |
| **Autocomplete** | `autocomplete.ts` (300 lines) | `src/tui/autocomplete.rs` (✅ 864 lines) | `AutocompleteProvider` trait, `CombinedAutocompleteProvider` (slash commands + file paths), `AutocompleteItem`, `AutocompleteSuggestions`, `SlashCommand`. File path completion via `std::fs::read_dir`. |
| **Theme** | `src/tui/theme.rs` (✅ 364 lines) | Trait for colors. `fg(color: &str, text: &str) -> String`, `bg(color: &str, text: &str) -> String`, `bold(text: &str) -> String`. Concrete implementation in `src/agent/ui/` with JSON configs, variable resolution, truecolor+256 fallback. |
| **Image** | `components/image.ts` | `src/tui/image.rs` (✅ ~100 lines) | Basic image support: data URL encoding, MIME type detection, Kitty protocol sequence generation. **No TUI Component** yet. |
| **Overlay** | `tui.ts` (compositing) | `src/tui/overlay.rs` (✅ ~150 lines) | `OverlayAnchor`, `OverlayMargin`, `OverlayOptions`, `OverlayEntry`, `OverlayLayout`. Full overlay compositing (anchor positioning, sizing, margins). Used by `Container` for overlay stack management. |
| **VisualTruncate** | — | `src/tui/visual_truncate.rs` | `truncate_to_visual_lines()` - visual-line-aware truncation shared by multiple renderers. |

#### Deliberately Skipped (not needed for rab)

pi-tui components we are NOT porting:

| Component | Reason |
|---|---|
| Image Component | Rab has basic data URL / Kitty protocol in `image.rs` but no full TUI `Image` component (future work) |
| KeybindingsManager | Rab has `Keybindings` struct with defaults, JSON load/save — covers pi's functionality |
| StdinBuffer | crossterm's `event::read()` handles escape sequence parsing |
| TerminalImage, TerminalColors | Not needed |
| EditorComponent | Interface for custom editors — not needed (rab uses Editor directly) |

---

### Tier 2: App-Specific UI (`src/agent/ui/`) ✅ IMPLEMENTED

These are rab's application components, built on `src/tui/` primitives. They are NOT part of the core TUI library.

| Component | Rust module | Purpose |
|---|---|---|
| **ChatEditor** | `src/agent/ui/chat_editor.rs` (✅ 763 lines) | Thin wrapper around `tui::Editor`. Provides rab-specific behaviors: slash command list, theme integration, autocomplete provider setup. |
| **MessageList** | `src/agent/ui/messages.rs` (✅ 568 lines) | Renders conversation history as styled text lines. Handles: user messages, assistant text, thinking blocks, tool calls, tool results. Respects `hide_thinking`, `collapse_tool_output`. **All lines padded to `width`** via `pad_to_width()`; `pad_to_width()` truncates via `truncate_to_width()` when `visible_width > width` to prevent terminal overflow. |
| **WorkingIndicator** | `src/agent/ui/working.rs` (✅ 73 lines) | Spinner shown during streaming. **Always rendered** (returns 1 empty line when inactive) to keep the composition line count stable and avoid full-screen clears on streaming state changes. |
| **Footer** | `src/agent/ui/footer.rs` (✅ 912 lines) | Two-line footer: cwd + git branch on line 1, token stats + model on line 2. |
| **ModelSelector** | `src/agent/ui/model_selector.rs` (✅ ~80 lines) | Full-screen overlay for picking a model. Uses `tui::SelectList`. Searchable. |
| **HelpOverlay** | `src/agent/ui/help.rs` (✅ ~100 lines) | Help display showing available commands and keybindings. Renders as full-screen overlay. |
| **Theme** | `src/agent/ui/theme.rs` + `themes/dark.json`, `themes/light.json` (✅ 698 lines) | Full JSON-based theme system. Loads `dark.json`/`light.json` embedded + custom `~/.rab/themes/*.json`. Variable resolution, truecolor + 256 fallback via cube mapping, `COLORFGBG` terminal detection, global singleton (`init_theme()`/`current_theme()`/`set_theme()`), convenience helpers (`accent()`, `dim()`, `muted()`, `bold_accent()`, etc.). |
| **App** | `src/agent/ui/app.rs` (✅ 3016 lines) | Main event loop and state. Owns the `tui::TUI`, composes the component tree each tick, dispatches input, handles agent events (streaming deltas → message list). **Pi-style header**, **queued messages** (submitted while streaming, displayed between chat and editor), **streaming text** (`pending_text`/`pending_thinking` rendered inline), **message queuing** (no concurrent loops), **working indicator always rendered** (empty line when inactive). |

#### App-Specific Message Components

| Component | Rust module | Purpose |
|---|---|---|
| **UserMessageComponent** | `src/agent/ui/components/user_message.rs` | User message in a colored box with `userMessageBg` background + markdown rendering + OSC133 zone markers. |
| **AssistantMessageComponent** | `src/agent/ui/components/assistant_message.rs` | Streaming assistant message with thinking blocks (expandable/collapsible), markdown text, syntax-highlighted code. |
| **ToolExecComponent** | `src/agent/ui/components/tool_messages.rs` | Tool execution with background color transitions (pending→success/error), per-tool formatted header (via `ToolRenderer` trait), syntax-highlighted or diff-highlighted results. |
| **ToolResultComponent** | `src/agent/ui/components/tool_messages.rs` | Non-pending tool results with fixed background color. |
| **BashExecution** | `src/agent/ui/components/bash_execution.rs` | Styled bash command rendering. Top/bottom borders in status-aware color, command header with `$`, output in `toolOutput` color, preview truncation, expand/collapse, live duration display ("Elapsed X.Xs"). `BashStatus` (Running/Complete/Cancelled/Error). |
| **EditorComponent** | `src/agent/ui/components/editor_component.rs` | Thin wrapper placing `ChatEditor` in a border frame. Border color reflects thinking level (`thinkingOff`..`thinkingXhigh`) or `bashMode`. |
| **FooterComponent** | `src/agent/ui/components/footer_component.rs` | Two-line footer (cwd + git branch, tokens + model) rendered as a Component. |
| **HeaderComponent** | `src/agent/ui/components/header.rs` | "rab" logo + expandable keybinding hints (toggled via Ctrl+O). |
| **InfoMessageComponent** | `src/agent/ui/components/info_message.rs` | Dimmed info messages (slash command feedback, status). |

### Pi Reference: Where App Components Live in pi

```
packages/tui/src/                            ← @earendil-works/pi-tui (core library)
├── components/
│   ├── text.ts                      → text.rs ✅
│   ├── spacer.ts                    → spacer.rs ✅
│   ├── box.ts                       → box.rs ✅
│   ├── loader.ts                    → loader.rs ✅
│   ├── cancellable-loader.ts        → cancellable_loader.rs ✅
│   ├── select-list.ts               → select_list.rs ✅
│   ├── settings-list.ts             → settings_list.rs ✅
│   ├── editor.ts                    → editor.rs ✅
│   ├── input.ts                     → input.rs ✅
│   ├── markdown.ts                  → markdown.rs ✅ (rab-specific, not pi port)
│   ├── image.ts                     → image.rs ⬜ (basic utility, no Component impl)
│   └── truncated-text.ts           → truncated_text.rs ✅
├── tui.ts                           → screen.rs + component.rs + focusable.rs + container.rs + tui_core.rs ✅
├── terminal.ts                      → terminal.rs ✅
├── keys.ts                          → keys.rs ✅
├── utils.ts                         → util.rs ✅
├── fuzzy.ts                         → fuzzy.rs ✅
├── kill-ring.ts                     → kill_ring.rs ✅
├── undo-stack.ts                    → undo_stack.rs ✅
├── word-navigation.ts               → word_nav.rs ✅
└── ...

packages/coding-agent/src/modes/interactive/components/
├── bordered-loader.ts               (skipped - not needed in rab)
├── dynamic-border.ts                (skipped - theme handles borders)
├── assistant-message.ts             → assistant_message.rs ✅
├── model-selector.ts                → model_selector.rs ✅
├── session-selector.ts              (skipped - not needed)
├── settings-selector.ts             (skipped - not needed)
├── tree-selector.ts                 (skipped - not needed)
└── ...
```

---

## File Structure (✅ implemented)

```
src/
├── tui/                             # Core TUI library
│   ├── mod.rs                       # ✅ Re-exports, module declarations
│   ├── component.rs                 # ✅ Component trait
│   ├── focusable.rs                 # ✅ Focusable trait, CURSOR_MARKER
│   ├── container.rs                 # ✅ Container struct (children + overlay stack)
│   ├── screen.rs                    # ✅ Diff renderer (787 lines)
│   ├── terminal.rs                  # ✅ Crossterm wrapper (392 lines)
│   ├── tui_core.rs                  # ✅ TUI struct - root container + screen + overlay routing (369 lines)
│   ├── keys.rs                      # ✅ Key identifiers, matches_key() (652 lines)
│   ├── keybindings.rs               # ✅ Action IDs, defaults, JSON config loading (398 lines)
│   ├── util.rs                      # ✅ ANSI-aware width, wrap, truncate, slice (1142 lines)
│   ├── fuzzy.rs                     # ✅ Fuzzy matching/filtering (263 lines)
│   ├── autocomplete.rs              # ✅ AutocompleteProvider trait, CombinedAutocompleteProvider (864 lines)
│   ├── theme.rs                     # ✅ Theme trait (fg, bg, bold) (364 lines)
│   ├── overlay.rs                   # ✅ Overlay types, anchor, sizing, compositing (150 lines)
│   ├── kill_ring.rs                 # ✅ KillRing (128 lines)
│   ├── undo_stack.rs                # ✅ UndoStack (73 lines)
│   ├── word_nav.rs                  # ✅ Word boundary navigation (363 lines)
│   ├── image.rs                     # ✅ Data URL encoding, Kitty protocol sequences (100 lines)
│   ├── visual_truncate.rs           # ✅ Visual-line-aware truncation
│   │
│   └── components/                  # ✅ Built-in Component impls
│       ├── mod.rs
│       ├── text.rs                  # ✅ Text
│       ├── truncated_text.rs        # ✅ TruncatedText
│       ├── spacer.rs                # ✅ Spacer
│       ├── box.rs                   # ✅ Box (TuiBox)
│       ├── loader.rs                # ✅ Loader
│       ├── cancellable_loader.rs    # ✅ CancellableLoader
│       ├── select_list.rs           # ✅ SelectList
│       ├── settings_list.rs         # ✅ SettingsList
│       ├── input.rs                 # ✅ Input
│       ├── editor.rs                # ✅ Editor (3170 lines)
│       ├── markdown.rs              # ✅ Markdown renderer (2103 lines)
│       ├── diff.rs                  # ✅ Diff renderer
│       ├── rc_ref_cell_component.rs # ✅ Rc/RefCell wrapper
│       └── dynamic_lines.rs         # ✅ DynamicLines section
│
├── agent/                           # ✅ Agent framework
│   ├── mod.rs                       # ✅ Re-exports
│   ├── loop.rs                      # ✅ AgentEvent, LoopConfig, run_agent_loop() (1962 lines)
│   ├── extension.rs                 # ✅ AgentTool, Extension, CommandHandler, ToolRenderer traits
│   ├── types.rs                     # ✅ AgentMessage, Role, ToolCall, Usage, PendingMessageQueue, QueueMode
│   ├── provider.rs                  # ✅ Provider trait, StreamEvent, ToolDef
│   ├── settings.rs                  # ✅ Settings load/save
│   ├── session.rs                   # ✅ SessionManager (1985 lines)
│   ├── skills.rs                    # ✅ Skill loading, frontmatter, prompt formatting (825 lines)
│   ├── system_prompt.rs             # ✅ SystemPromptBuilder (428 lines)
│   ├── context_files.rs             # ✅ AGENTS.md/CLAUDE.md discovery
│   └── ui/                          # ✅ Interactive mode
│       ├── mod.rs
│       ├── app.rs                   # ✅ Main event loop, App state, run() (3016 lines)
│       ├── chat_editor.rs           # ✅ ChatEditor (763 lines)
│       ├── messages.rs              # ✅ MessageList (568 lines)
│       ├── working.rs               # ✅ WorkingIndicator (73 lines)
│       ├── footer.rs                # ✅ Footer (912 lines)
│       ├── model_selector.rs        # ✅ ModelSelector
│       ├── help.rs                  # ✅ HelpOverlay
│       ├── theme.rs                 # ✅ RabTheme (698 lines)
│       └── components/              # ✅ Message Components (9 files)
│           ├── mod.rs
│           ├── user_message.rs
│           ├── assistant_message.rs
│           ├── tool_messages.rs     # ✅ ToolExecComponent, ToolResultComponent (975 lines)
│           ├── bash_execution.rs    # ✅ BashExecution (636 lines)
│           ├── editor_component.rs
│           ├── footer_component.rs
│           ├── header.rs
│           ├── info_message.rs
│           └── message_components.rs
│
├── builtin/                         # ✅ Tool implementations
│   ├── mod.rs
│   ├── read.rs                      # ✅ Read tool (939 lines)
│   ├── write.rs                     # ✅ Write tool
│   ├── edit.rs                      # ✅ Edit tool (1079 lines)
│   ├── bash.rs                      # ✅ Bash tool (1395 lines)
│   ├── commands.rs                  # ✅ 8 slash commands: quit, model, hotkeys, reload, new, resume, session, name
│   └── file_mutation_queue.rs       # ✅ File mutation queue for safe writes
│
├── adapter.rs                       # ✅ GenaiProvider (top-level - external adapter)
├── auth.rs                          # ✅ Auth storage (JSON load/save)
├── lib.rs                           # ✅ pub mod agent; pub mod adapter; pub mod tui;
└── main.rs                          # ✅ CLI entry point (470 lines)

src/tui/     — 30 files, ~15,000 lines total
src/agent/   — 18 files, ~15,000 lines total
src/builtin/ — 6 files, ~4,500 lines total
Total:       — 74 source files, ~34,000 lines, 662 tests
```

---

## Dependency Changes in Cargo.toml

```diff
- ratatui = "0.30"
+ crossterm = "0.29"
  unicode-segmentation = "1"   # keep - needed for grapheme-aware editor cursor
+ unicode-width = "0.2"
+ pulldown-cmark = "0.13.4"    # markdown parsing
+ syntect = { version = "5.3.0", optional = true }  # syntax highlighting
+ diff = "0.1"                 # diff algorithm for edit tool
+ base64 = "0.22.1"            # image data URL encoding
+ genai = "0.6"                # LLM provider adapter
```

Keep `unicode-segmentation` - the editor needs `UnicodeSegmentation::graphemes()` for correct cursor movement through emoji, combining characters, and CJK characters. `unicode-width` alone only measures display width, it doesn't iterate grapheme clusters.

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

The Editor is pi-tui's most complex component (2,307 lines). It is ported in full to `src/tui/components/editor.rs` (3,170 lines).

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
    fn new(theme: &EditorTheme) -> Self;
    fn set_autocomplete_provider(&mut self, provider: Box<dyn AutocompleteProvider>);

    // -- Content --
    fn get_text(&self) -> String;
    fn get_expanded_text(&self) -> String;
    fn get_lines(&self) -> &[String];
    fn get_cursor(&self) -> (usize, usize);
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

    // -- Slash commands --
    fn set_slash_commands(&mut self, commands: Vec<&str>);
    fn trigger_autocomplete(&mut self) -> bool;

    // -- Autocomplete query --
    fn is_showing_autocomplete(&self) -> bool;
}
```

### Keybindings (hard-coded, match pi)

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

### Porting Breakdown

| Subsystem | TS lines | Rust lines | Key differences |
|---|---|---|---|
| State + cursor movement | ~300 | ~400 | `UnicodeSegmentation::graphemes()` replaces `Intl.Segmenter` |
| Word-wrap layout | ~150 | ~200 | Direct port; same algorithm |
| Render (visual line map + scroll) | ~200 | ~500 | `Vec<String>` output, no JSX-style composition |
| Input dispatch | ~200 | ~350 | `match` on crossterm `KeyEvent` instead of `matchesKey()` |
| Text mutations (insert, delete, newline) | ~150 | ~250 | Same logic, Rust borrow checker will need care |
| Kill ring + yank/yank-pop | ~80 | ~80 | `Vec<String>` ring with rotation |
| Undo stack | ~40 | ~40 | Generic `Vec<T>` with `pop()` |
| Word navigation | ~50 | ~50 | Direct port of `findWordBackward`/`findWordForward` |
| Character jump | ~40 | ~40 | Simpler in Rust (direct char search) |
| History navigation | ~80 | ~100 | Vec of strings with index |
| Paste handling (bracketed + markers) | ~120 | ~150 | `\x1b[200~...\x1b[201~` → `String` + marker logic |
| Autocomplete integration | ~300 | ~400 | `AutocompleteProvider` trait, `CombinedAutocompleteProvider` (slash commands + file paths). `check_autocomplete_trigger()` on `/`, `@`, `#`, letters in slash context. |
| Slash command completion | — | ~200 | rab-specific: auto-trigger on `/`, suggestion list |
| **Total** | **~2,300** | **~3,170** | Rust is sometimes longer due to explicit state management, trait implementations, and rab-specific features |

---

## Input: Single-Line Text Input

Port of `components/input.ts` (447 lines). Lives at `src/tui/components/input.rs` (639 lines).

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

---

## SettingsList: Toggleable Settings Picker

Port of `components/settings-list.ts` (250 lines). Lives at `src/tui/components/settings_list.rs` (480 lines).

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

When a submenu is active, the submenu `Component` takes over completely - `render()` and `handleInput()` both delegate to it. The submenu receives `done(selectedValue?)` to close itself.

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
    pub fn render(&mut self, new_lines: Vec<String>, width: u16, height: u16,
                  writer: &mut dyn Write) -> io::Result<()> {
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

The algorithm is a direct port of `TUI.doRender()` from `tui.ts` (lines ~1050-1570), minus overlay compositing and Kitty image logic (overlay compositing is handled by `Container`).

---

## TUI Core (`src/tui/tui_core.rs`)

The `TUI` struct wraps a `Screen` diff renderer, a `Container` (root), and provides:
- **Overlay management**: `show_overlay()`, `hide_overlay()`, `pop_overlay()`, `has_overlays()`
- **Input routing**: `route_input()` delegates to overlays first, then root
- **Rendering**: `render()` renders root container, extracts cursor markers, positions hardware cursor
- **Screen delegation**: `screen_mut()`, `full_redraw_count()`, `set_clear_on_shrink()`, `set_dimensions()`, `finalize()`

---

## Key Design Decisions

1. **No async in Component trait.** `render()` and `handle_input()` are synchronous. Async lives in the app event loop (`src/agent/ui/app.rs`), which feeds events to components and triggers re-renders.

2. **Components own their state.** No global state. `Editor` owns its text buffer, cursor, history. `SelectList` owns its items, selection index, search query. `Loader` owns its frame counter.

3. **Width is passed in, not stored.** Every `render(width)` call receives the current viewport width. Components cache their output for the given width and invalidate on state change.

4. **Theme is a trait, not a global.** Components accept theme via constructor or setter. The app layer provides the concrete theme. `RabTheme` also has a global singleton for convenience.

5. **Overlay compositing in Container, not Screen.** pi-tui's overlay compositing adds ~600 lines of complexity to the diff renderer. Rab handles overlays through `Container`'s overlay stack: overlays are rendered separately and composited with the main content at the Container level.

6. **Line-level diffing, not cell-level.** pi-tui compares strings. ratatui compares `Cell` structs (char + style). Line-level is simpler and sufficient for a chat UI where most changes are full-line replacements or appends.

7. **Editor lives in tui/components/ not ui/.** The Editor is a general-purpose component (like Text or SelectList). rab's app wraps it with `ChatEditor` in `src/agent/ui/chat_editor.rs` for app-specific behavior (slash commands, file paths, submission hook).

8. **Input is separate from Editor.** The `Input` component provides single-line text entry with horizontal scrolling. It is lighter than `Editor` (no word-wrap, no multiline, no autocomplete, no character jump) but shares kill-ring, undo-stack, grapheme-aware cursor, and `Focusable` support. `SettingsList` uses `Input` for its search box.

9. **Keybindings are hard-coded with action IDs, not dynamic.** 27+ action identifiers with default key mappings. Can be overridden via `~/.rab/keybindings.json`. No runtime keybinding editor.

10. **Markdown rendering is rab-specific, not a pi port.** Pi doesn't have markdown rendering in TUI. Rab uses `pulldown-cmark` for parsing and `syntect` for syntax highlighting.
