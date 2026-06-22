# Pi vs Rab — Message Rendering ✅ All Gaps Closed

This document tracked gaps between pi's message rendering and rab's implementation.
**All gaps have been closed.** Rab's rendering pipeline matches pi's architecture 1:1.

## Current state

```
TUI.render()
  └── root Container:
      ├── HeaderComponent (logo + expandable keybinding hints)
      ├── chat_container (RefContainer — all messages as Components)
      │   ├── UserMessageComponent (Box + userMessageBg + markdown + OSC133)
      │   ├── RcRefCellComponent (streaming assistant message, updated in-place)
      │   │   └── AssistantMessageComponent (markdown + thinking blocks)
      │   ├── ToolExecComponent (bg transitions: pending→success/error)
      │   │   ├── Per-tool formatting: read (docs/resource labels, syntax)
      │   │   │                            bash ($ command, timeout, duration)
      │   │   │                            write, edit (diff with intra-line highlight)
      │   │   │                            ls (path + limit)
      │   │   └── Truncation preview + syntax highlighting (read)
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
| Editor border color | `update_border_color()` — thinking level (`thinkingOff`..`thinkingXhigh`) or `bashMode` |
| Spacers | `chat_add()` helper — adds `Spacer(1)` before each component when non-empty |
| OSC133 zones | `UserMessageComponent` + `AssistantMessageComponent` emit them in `render()` |
| Per-tool formatting | `format_tool_call_header()` — bash (`$ command`), read (compact docs/resource), write, edit, ls |
| Diff rendering | `render_diff()` — unified diff, colored +/lines, intra-line character-level inverse |
| Syntax highlighting | syntect enabled by default, `highlight_code()` + `path_to_language()` |
| Bash streaming | `AgentEvent::ToolProgress` — tokio async reads, progressive component updates |
| Duration display | "Elapsed X.Xs" / "Took X.Xs" in BashExecutionComponent |
| Error/abort | `AgentEvent::Aborted` — inline error text in streaming component |
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
