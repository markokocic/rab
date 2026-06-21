## Markdown rendering — planned

### Approach
- **Parser**: `pulldown-cmark` — fast, zero-copy, event-based iterator, used by cargo doc
- **Syntax highlighting**: `syntect` — same engine as bat/delta/broot, ~250 grammars, ~1 MiB binary cost

### New: `src/tui/components/markdown.rs`
- `Markdown` component impl (analogous to pi's `packages/tui/src/components/markdown.ts`)
  - Parses with pulldown-cmark event iterator
  - Two-phase: (1) render tokens → styled ANSI lines, (2) wrap + pad + bg
  - Style reapplication: emit parent style prefix after inline resets (matching pi)
  - Cache rendered output by text+width
- `MarkdownTheme` struct with `Arc<dyn Fn(&str) -> String>` fields for each element type (heading, link, code, etc.)
- `DefaultTextStyle` — base fg color + decorations (bold/italic/etc.)
- `MarkdownOptions` — `preserve_ordered_list_markers`
- `get_markdown_theme()` factory in `src/agent/ui/theme.rs` using RabTheme colors + syntect

### Supported elements (1/1 with pi)
- **Block**: headings (h1–h6), paragraphs, fenced code blocks (with syntax highlighting), lists (ordered/unordered, nested, task items), blockquotes (nested block tokens, "│ " prefix), tables (width-aware column sizing, cell wrapping, box-drawing borders), horizontal rules, HTML (plain text)
- **Inline**: bold, italic, codespan, links (OSC 8 hyperlinks where supported, else inline URL), strikethrough, line breaks

### Integration in `src/agent/ui/messages.rs`
| Current message type | New rendering |
|---|---|
| `DisplayMsg::User` | `Box(1,1, userMessageBg)` → `Markdown(0,0, mdTheme, {color: userMessageText})` |
| `DisplayMsg::AssistantText` | `Markdown(1,0, mdTheme)` — no bg, left padding only |
| `DisplayMsg::Thinking` | `Markdown(1,0, mdTheme, {color: thinkingText, italic: true})` |

### Phases

- [x] **Phase 1**: Core Markdown component with pulldown-cmark ✅
- [x] **Phase 2**: Theme integration ✅
- [x] **Phase 3**: Syntax highlighting with syntect (optional feature gate) ✅
- [x] **Phase 4**: Integrate into messages.rs ✅
- [x] **Phase 5**: Tables ✅
- [x] **Phase 6**: Tests ✅

---

## Chat/UX gaps vs pi

### ✅ Completed — Missing app actions (all 10 implemented)

| Action | Key | Status |
|--------|-----|--------|
| `app.clear` | Ctrl+C | ✅ Clear editor, double-press exits |
| `app.suspend` | Ctrl+Z | ✅ Forwarded to shell |
| `app.thinking.cycle` | Shift+Tab | ✅ Cycles: off → low → medium → high → xhigh |
| `app.model.cycleForward` | Ctrl+P | ✅ Cycles forward through available models |
| `app.model.cycleBackward` | Shift+Ctrl+P | ✅ Cycles backward through available models |
| `app.tools.expand` | Ctrl+O | ✅ Toggles all tool output expansion |
| `app.editor.external` | Ctrl+G | ✅ Opens $VISUAL/$EDITOR, restores content on exit |
| `app.message.followUp` | Alt+Enter | ✅ Queues message while streaming |
| `app.message.dequeue` | Alt+Up | ✅ Restores queued messages to editor |
| `app.thinking.toggle` | Ctrl+T | ✅ Keep existing toggle thinking visibility |

### ✅ Completed — Message rendering polish

| Item | Status |
|------|--------|
| Tool output expand/collapse (BashExecution) | ✅ Preview truncation, first N lines when collapsed |
| Visual truncation of long output lines | ✅ Each line capped at 200 chars |
| Expand/collapse toggle (Ctrl+O) | ✅ Toggles all tool outputs |
| OSC 133 zone markers | ✅ Already present in messages.rs |

### ✅ Completed — Chat scrolling

| Item | Status |
|------|--------|
| PageUp | ✅ Scroll up (increase scroll_offset) |
| PageDown | ✅ Scroll down (decrease scroll_offset) |
| Scroll indicator | ✅ "↑ N more" shown when scrolled |
| Reset on submit | ✅ scroll_offset reset to 0 on new message |

### 🟡 Deferred — Session management (complex, needs more architecture)

- [ ] `app.session.new` — Start a new session
- [ ] `app.session.tree` — Open session tree selector
- [ ] `app.session.fork` — Fork current session
- [ ] `app.session.resume` — Resume a session
- [ ] `app.session.toggleNamedFilter` — Toggle named session filter

### 🟡 Deferred — Image support (complex, scoped out)

- [ ] `app.clipboard.pasteImage` — Paste clipboard image as attachment
- [ ] Image support in multimodal payload

### 🟡 Deferred — Overlays (all missing)

- [ ] `config-selector` — pick from stored configs
- [ ] `theme-selector` — pick theme
- [ ] `session-selector` — tree view of sessions
- [ ] `first-time-setup` — guided setup on first run
- [ ] `changelog` — what's new since last version
- [ ] `login-dialog` — OAuth login
- [ ] `oauth-selector` — pick OAuth provider

### ✅ Completed — Footer improvements

- [x] Auto-compact toggle (`app.compact.toggle`, Ctrl+Shift+C) with styled ⚡ indicator
- [x] Narrow terminal protection — graceful truncation with priority: dot > model > stats
- [x] Extension status line — verified working, truncated to width

### ✅ Completed — Editor & input

- [x] Auto-trigger slash commands on `/` — shows autocomplete as soon as `/char` is typed
- [x] Check autocomplete after external editor restore and dequeue restore

### 🟡 Deferred — Editor & input (image-blocked)

- [ ] Paste image from clipboard (blocked on image support)

### 🟡 Deferred — Other

- [ ] Suspend/resume (Ctrl+Z → `kill -CONT`) — needs TTY save/restore
- [ ] Debug key (Shift+Ctrl+D)
- [ ] Keybinding hints in header (dynamic, based on context)
- [ ] Proper chat scrolling with viewport management (terminal natural scrolling)

---

## tools
- [ ] check tool execution modes in pi, parallel, sequence, ... and compare with rab

---

## Agent framework
- [ ] `adapter/genai.rs` — multiple backends (Anthropic, OpenAI, Google, Ollama)
- [ ] `compaction.rs` — context window compaction
- [ ] Hook pipeline — `before_tool_call`, `after_tool_call`, `CancellationToken`
- [ ] Steering / follow-up queues — runtime message injection
- [ ] Tool execution modes — sequential mode
- [ ] `~/.rab/models.json` — custom provider/model definitions
- [ ] Image support — multimodal payload
- [ ] `rab plugin new` — scaffold extension crate

---

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

---

## ✅ Done
- [x] System prompt (AGENTS.md/CLAUDE.md, SYSTEM.md, APPEND_SYSTEM.md, project context)
- [x] Context file discovery
- [x] Skills loading and `/skill:name` expansion
- [x] CLI flags (`--no-context-files`, `--system-prompt`, `--append-system-prompt`)
- [x] Startup resource listing
- [x] Built-in tools (bash, read, write, edit) — behavioral 1/1 with pi
- [x] Thinking message rendering with per-level colors
- [x] **Complete pi-tui alignment** — 27 modules, 429 tests, all 6 phases
- [x] **Missing app actions (10)** — clear, suspend, thinking cycle, model cycle, tools expand, external editor, follow-up, dequeue
- [x] **Message rendering polish** — tool output preview truncation, visual line truncation, expand/collapse
- [x] **Chat scrolling** — PageUp/PageDown, scroll indicator, reset on submit
