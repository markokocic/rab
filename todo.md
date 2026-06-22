# Todo

## Active / Pi alignment

- [ ] **Tool renderers — align rab to pi:** In pi, expand shows `pi: ... (5 earlier lines, ctrl+o to expand)` / `rab: ... 10 earlier lines, (C-O) to expand`. Rab should match this format exactly (variable N, same text/layout).
- [x] **Read tool title — no space between filename and `:`:** Should be like pi (no `" "` between filename and colon).
- [ ] **Scrolling broken in chat screen:** Once you scroll up with the mouse, you can't scroll back down.
- [ ] **Agentic loop freeze:** Sometimes after a few rounds the screen freezes and rab stops responding to any input. Only recoverable via `pkill -9 rab` from another terminal.
- [ ] **Welcome message:** Render exactly the same content and using the same UI as pi's welcome message.
- [ ] **Autocomplete of `/` commands — align to pi:** In pi, `/q<enter>` closes pi. In rab, `/q<enter>` autocompletes to `/quit` and needs a second enter. Should close immediately like pi.
- [ ] **Thinking visibility — `Ctrl+t` only:** Should be toggled exclusively by `Ctrl+t`, matching pi behavior.
- [ ] **Expand/collapse states — `Ctrl+o` only:** Should be controlled exclusively by `Ctrl+o`, matching pi behavior.
- [ ] **Model thinking settings reset + editor borders:** Thinking setting gets reset to off intermittently. Editor border colors don't reflect the thinking setting. Should behave like pi.

---

# Pi vs Rab - Message Rendering ✅ All Gaps Closed

This document tracked gaps between pi's message rendering and rab's implementation.
**All gaps have been closed.** Rab's rendering pipeline matches pi's architecture 1:1.

## Current state

```
TUI.render()
  └── root Container:
      ├── HeaderComponent (logo + expandable keybinding hints)
      ├── chat_container (RefContainer - all messages as Components)
      │   ├── UserMessageComponent (Box + userMessageBg + markdown + OSC133)
      │   ├── RcRefCellComponent (streaming assistant message, updated in-place)
      │   │   └── AssistantMessageComponent (markdown + thinking blocks)
      │   ├── ToolExecComponent (bg transitions: pending→success/error)
      │   │   ├── Per-tool formatting via ToolRenderer: read (docs/resource labels, syntax)
      │   │   │                                              bash ($ command, timeout, duration, truncation)
      │   │   │                                              write (path + content preview, empty on success)
      │   │   │                                              edit (renderShell: self, diff rendering)
      │   │   └── Live duration from started_at for all tool calls
      │   ├── BashExecutionComponent (borders, spinner, streaming output)
      │   ├── InfoMessageComponent (dim text)
      │   └── ...
      ├── pending_section, status_section, queued_section, working_section
      ├── EditorComponent (border color: thinking level / bash mode)
      └── FooterComponent (model, tokens, git branch, streaming, auto-compact)
```

## Key architectural features (matching pi)

| Feature | Implementation |
|---------|---------------|
| Component tree | TUI has `root: Container`, recursive `render()` |
| Shared ownership | `RcRefCellComponent`, `RefContainer`, `RcToolExec` |
| Streaming updates | `Weak<RefCell<AssistantMessageComponent>>` for in-place text/thinking |
| Expand/collapse | `set_expanded()` on `Component` trait, global toggle via `handle_tools_expand()` |
| Editor border color | `update_border_color()` - thinking level (`thinkingOff`..`thinkingXhigh`) or `bashMode` |
| Spacers | `chat_add()` helper - adds `Spacer(1)` before each component when non-empty |
| OSC133 zones | `UserMessageComponent` + `AssistantMessageComponent` emit them in `render()` |
| Per-tool formatting | `format_tool_call_header()` - bash (`$ command`), read (compact docs/resource), write, edit, ls |
| Diff rendering | `render_diff()` - unified diff, colored +/lines, intra-line character-level inverse |
| Syntax highlighting | syntect enabled by default, `highlight_code()` + `path_to_language()` |
| Bash streaming | `AgentEvent::ToolProgress` - tokio async reads, progressive component updates |
| Duration display | "Elapsed X.Xs" / "Took X.Xs" in BashExecutionComponent |
| Error/abort | `AgentEvent::Aborted` - inline error text in streaming component |
| Write success | No output text, just bg transition (pending→success) |
| Git branch | `refresh_git_branch()` on each `AgentStart` |
| codeBlockIndent | Already in `MarkdownTheme` (default `"  "`) |

## Remaining (not rendering-related)

These are feature gaps in the agent/tool functionality, not rendering:

- **Missing slash commands**: /settings, /export, /import, /compact, /changelog, etc.
- **Multi-backend provider**: genai adapter
- **Context window compaction**
- **Hook pipeline** (before/after tool call)
- **Extensions**: follow pi's extension system
- **MCP adapter**

## Image system gaps (7)

| # | Gap | Status |
|---|-----|--------|
| C4 | TUI `Image` component (Kitty + iTerm2 + fallback) | ❌ Open |
| C5 | Terminal capabilities detection (`getCapabilities()`) | ❌ Open |
| C6 | Cell dimension tracking for pixel-accurate sizing | ❌ Open |
| C7 | Image resize utility | ❌ Open |
| C8 | Image convert utility | ❌ Open |
| C9 | Clipboard image paste | ❌ Open |
| C10 | Show images selector UI | ❌ Open |

## UI component gaps (10)

| # | Gap | Status |
|---|-----|--------|
| C11 | Visual truncate utility | ✅ `tui::visual_truncate.rs` |
| C12 | Session selector + search | ❌ Open |
| C13 | Theme selector overlay | ❌ Open |
| C14 | Thinking level selector | ❌ Open |
| C15 | Extension editor / input / selector | ❌ Open |
| C16 | Config / settings selector | ❌ Open |
| C17 | Model selector improvements | ❌ Open |
| C18 | OAuth login dialog | ❌ Open |
| C19 | Trust selector | ❌ Open |
| C20 | First-time setup | ❌ Open |
