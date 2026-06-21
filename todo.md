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
  - [x] Add `pulldown-cmark` dependency to `Cargo.toml`
  - [x] Create `src/tui/components/markdown.rs` with `Markdown` struct implementing `Component`
  - [x] Define `MarkdownTheme`, `DefaultTextStyle`, `MarkdownOptions` types
  - [x] Implement `get_style_prefix()` helper (sentinel pattern matching pi)
  - [x] Implement `render_token()` for all block elements (no syntax highlight): heading, paragraph, code (plain mdCodeBlock), list, blockquote, hr, html, space
  - [x] Implement `render_inline_tokens()` for inline elements: bold, italic, codespan, link, strikethrough, line breaks, text, html
  - [x] Style reapplication: emit parent style prefix after inline resets (matching pi's `stylePrefix` pattern)
  - [x] Implement caching by `(text, width)`
  - [x] Two-phase: (1) render tokens → styled ANSI lines, (2) wrap + pad + bg
  - [x] Replace tabs with 3 spaces for consistent rendering
  - [x] Handle nested list rendering with depth-based indentation
  - [x] Handle task list items (`[x]` / `[ ]`)
  - [x] Handle `preserve_ordered_list_markers` option (simplified — pulldown-cmark doesn't expose raw markers)
  - [x] Export `Markdown` from `src/tui/components/mod.rs`

- [x] **Phase 2**: Theme integration ✅
  - [x] Add `get_markdown_theme()` factory in `src/agent/ui/theme.rs` using `RabTheme` colors
  - [x] Wire all existing `md*` colors: mdHeading, mdLink, mdLinkUrl, mdCode, mdCodeBlock, mdCodeBlockBorder, mdQuote, mdQuoteBorder, mdHr, mdListBullet
  - [x] Wire text decorations via `MarkdownTheme`: bold, italic, strikethrough, underline
  - [x] Restore underline reset at line ends in wrapping (matching pi's `line_end_reset`)

- [x] **Phase 3**: Syntax highlighting with syntect (optional feature gate) ✅
  - [x] Add `syntect` dependency behind feature flag
  - [x] Add `highlightCode` field to `MarkdownTheme`
  - [x] Initialize syntect once (lazy static) with ~250 grammars
  - [x] Integrate with code block rendering: detect language, apply syntax colors
  - [x] Use `codeBlockIndent` prefix for each code block line (default: `"  "`)
  - [x] Feature-gate the import and all syntect usage

- [x] **Phase 4**: Integrate into messages.rs ✅
  - [x] Replace `DisplayMsg::User` rendering with `Markdown(0,0, mdTheme, {color: userMessageText})` inside `TuiBox(userMessageBg)`
  - [x] Replace `DisplayMsg::AssistantText` rendering with `Markdown(1,0, mdTheme)` — no bg, left padding only
  - [x] Replace `DisplayMsg::Thinking` rendering with `Markdown(1,0, mdTheme, {color: thinkingText, italic: true})` inside `TuiBox(thinking_bg)`
  - [x] Keep OSC 133 zone markers around messages

- [x] **Phase 5**: Tables ✅
  - [x] Implement `render_table()` with width-aware column sizing
  - [x] Calculate natural column widths + minimum word widths
  - [x] Distribute available width proportionally across columns
  - [x] Handle cell wrapping (multi-line cells)
  - [x] Render box-drawing borders (┌─┬─┐, │, ├─┼─┤, └─┴─┘)
  - [x] Bold header row
  - [x] Fallback to raw markdown when terminal too narrow

- [x] **Phase 6**: Tests ✅
  - [x] Headings: h1–h6 styling, spacing between heading and next non-space token (tests: `test_heading_h1`, `test_heading_h3_marker`, `test_heading_h4_marker`, `test_heading_h5_marker`, `test_heading_h6_marker`, `test_heading_h2_spacing`)
  - [x] Bold/italic: strong and em rendering with style prefix reapplication (`test_bold_italic`, `test_bold_italic_style_restore`)
  - [x] Codespan: inline code with mdCode color + style prefix restoration (`test_codespan`, `test_inline_code_style_restore`)
  - [x] Code blocks: fence rendering, plain mdCodeBlock color (`test_code_block`, `test_fenced_code_with_language`, `test_code_block_markers`)
  - [x] Links: fallback to inline URL (`test_link_inline`, `test_link_with_dest`, `test_autolink`)
  - [x] Lists: ordered, unordered, nested, task items (`test_unordered_list`, `test_ordered_list`, `test_nested_list`, `test_task_list`)
  - [x] Blockquotes: nested block tokens, "│ " prefix, italic quote style (`test_blockquote`, `test_blockquote_nested`)
  - [x] Strikethrough: del rendering with strict regex (`test_strikethrough`, `test_strikethrough_markers`)
  - [x] Wrapping: long lines wrap at content width (`test_wrap_long_text`)
  - [x] Padding: paddingX, paddingY, background application (`test_padding_x`, `test_padding_y`, `test_default_text_style`)
  - [x] Caching: same text+width returns cached lines (`test_cache_hit`, `test_cache_invalidation`, `test_cache_different_width`)
  - [x] Tables: width-aware rendering with box-drawing borders (`test_table_basic`, `test_table_narrow_fallback`)

---

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
