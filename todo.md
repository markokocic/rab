# Pi vs Rab — Message Rendering Gap Analysis

This document catalogs all gaps between pi's message rendering and rab's current implementation. The goal is 1:1 parity.

---

## 1. Message Types — DisplayMsg Enum

### Missing Message Types

| Pi Type | Pi Component / Style | Rab Status |
|---------|---------------------|------------|
| **CustomMessage** | Purple bg (`customMessageBg`), `[customType]` label in `customMessageLabel`, body in `customMessageText`, optional custom renderer | ❌ Missing entirely |
| **CompactionSummaryMessage** | Same purple bg, collapsible, `[compaction]` label, token count, summary text | ❌ Missing entirely |
| **BranchSummaryMessage** | Same purple bg, collapsible, `[branch]` label, summary text | ❌ Missing entirely |
| **SkillInvocationMessage** | Same purple bg, collapsible, `[skill]` label, skill name + content | ❌ Missing entirely |
| **BashExecution** (standalone `!` command) | Border with `bashMode` color, `$ command` header, spinner, expand/collapse preview, exit code, cancellation, truncation warnings | ❌ Missing entirely (different from bash tool calls) |

### Existing Message Types — Gaps

| Pi Type | Pi Rendering Detail | Rab Gap |
|---------|-------------------|---------|
| **UserMessage** | `Box` with `userMessageBg` background, markdown in `userMessageText` color, `preserveOrderedListMarkers` option, OSC133 zones | ✅ Mostly matched. Check OSC133 compatibility |
| **AssistantText** | `Markdown` component with `MarkdownTheme`, no background, paddingY=0, OSC133 zones | ✅ Mostly matched. Check OSC133 |
| **Thinking** (expanded) | `Markdown` in `thinkingText` color + italic, rendered inside `Box` with `thinking_bg` | ⚠️ Rab uses `thinking_bg` (derived) but pi uses inline `Markdown` with per-block style overrides. Rab puts content in Box; pi puts Markdown without Box but applies italic+color |
| **Thinking** (hidden) | Single line: `italic(fg("thinkingText", label))`, no background | ⚠️ Rab uses `thinking_bg` background on hidden label; pi does NOT use background for hidden thinking |
| **ToolCall** | Per-tool custom renderer via `renderCall()` — different for read, write, edit, bash, ls | ❌ Generic rendering only |
| **ToolResult** | Per-tool custom renderer via `renderResult()` — changes bg color based on `isPartial`, `isError`, has expand/collapse | ❌ Generic rendering only |

---

## 2. Per-Tool Rendering

### read tool

| Aspect | Pi | Rab |
|--------|----|-----|
| Compact labels | `read docs docs/README.md`, `read skill my-skill`, `read resource to/AGENTS.md` with `dim` expand hint | ❌ Not rendered to UI (compact label returned in ToolOutput but not rendered with proper styling) |
| Syntax highlighting | Full syntax highlight with theme colors | ❌ Not rendered |
| Line range | `path:1-50` in `warning` color after path | ❌ Not rendered |
| Result content | Syntax-highlighted lines, trimmed trailing empties, truncation notices | ❌ Shown as plain text |
| Expand/collapse | "X more lines" with key hint | ❌ Not implemented |

### write tool

| Aspect | Pi | Rab |
|--------|----|-----|
| Syntax highlighting | Full incremental syntax highlighting during streaming | ❌ Not implemented |
| Line count | Shows total lines, preview lines | ❌ Not rendered |
| Result (success) | No output (green bg transition) | ❌ Not implemented |
| Expand/collapse | "X more lines" with key hint | ❌ Not implemented |

### edit tool

| Aspect | Pi | Rab |
|--------|----|-----|
| Diff preview | Async computed diff with `renderShell: "self"`, shown inline while waiting for execution | ❌ Not implemented |
| Intra-line diff | Word-level diff with inverse highlighting on changed tokens | ❌ Not implemented |
| Color | Added lines in `toolDiffAdded`, removed in `toolDiffRemoved`, context in `toolDiffContext` | ❌ Not implemented |
| Status bg | Pending → success (green) → error (red) bg transition | ❌ Not implemented |

### bash tool

| Aspect | Pi | Rab |
|--------|----|-----|
| Tool call display | `$ command` with `toolTitle` + bold, timeout suffix in `muted` | ❌ Renders as generic tool call |
| Streaming output | Preview truncation (last 5 lines when collapsed), elapsed timer updates every 1s | ❌ Not rendered to UI during streaming |
| Result display | Syntax-highlighted (no), output in `toolOutput` color, preview truncation with width-aware visual truncation | ❌ Generic rendering |
| Duration | "Took 2.3s" or "Elapsed 5.1s" during streaming | ❌ Not rendered |
| Truncation warnings | Full output path, truncated line/byte counts | ❌ Not rendered |

### ls tool

| Aspect | Pi | Rab |
|--------|----|-----|
| Call display | `ls path` with `toolTitle` + bold, optional `(limit N)` | ❌ Not implemented |
| Result display | `toolOutput` color, preview truncation, entry limit warning | ❌ Not implemented |

---

## 3. Rendering Features

### OSC133 Terminal Zones

Pi wraps user and assistant messages with `\x1b]133;A\x07` (start) and `\x1b]133;B\x07\x1b]133;C\x07` (end) sequences for terminal selection integration (iTerm2, Kitty, etc.).

**Rab**: ✅ Matched in `messages.rs` for User and AssistantText DisplayMsgs. Check that BashExecution, ToolCall, ToolResult, Thinking also need them (pi only applies to user + assistant).

### Thinking Block — Hide/Show Toggle

Pi toggles between:
- **Visible**: Markdown in `thinkingText` + italic, optional background
- **Hidden**: Single line `italic(fg("thinkingText", label))` (no background)

**Rab**: Uses `thinking_bg` background on the hidden label — pi does NOT. Rab needs to match pi's exact hidden style.

### Tool Expand/Collapse

Pi has a global `toolOutputExpanded` toggle (keybinding `app.tools.expand`) that:
1. Toggles all `Expandable` components in the chat
2. Changes the header between compact/expanded states
3. Affects tool call renderers (`options.expanded`)
4. Persists to settings
5. Affects read/write/edit/bash/ls preview lengths
6. Affects diff preview display

**Rab**: Has `collapse_tool_output` / `tools_expanded` but doesn't propagate to component-level expand/collapse. No per-component expandable interface.

### Editor Border Color

Pi sets editor border color to:
- `thinkingOff`..`thinkingXhigh` based on current thinking level
- `bashMode` color when editor starts with `!`

**Rab**: ❌ Not implemented. Editor border is always default color.

### Status Line Deduplication

Pi's `showStatus()`:
- Checks if the last two children are a Spacer + a status Text
- If so, **mutates** the last Text instead of appending
- This prevents consecutive status messages from accumulating

**Rab**: Has `status_text` field but it's a single text, cleared after each render. Not quite the same — pi tracks pair of (spacer, text) and reuses them.

### Queued Messages Display

Pi renders queued messages between chat and editor as dim text with ◷ prefix and "↳ queued" hint.

**Rab**: ✅ Implemented in `compose_ui`.

### Markdown — codeBlockIndent

Pi passes `codeBlockIndent` from settings through `MarkdownTheme`:
```typescript
codeBlockIndent: this.settingsManager.getCodeBlockIndent(),
```

**Rab**: ❌ Not in `MarkdownTheme`. Need to add.

### Loaded Resources Header

Pi shows an `ExpandableText` header with startup info: logo, keybinding hints, compact/expanded expansion state, "Pi can explain..." onboarding text.

**Rab**: Shows simple "rab" logo header. No keybinding hints, no expansion, no onboarding.

### Loaded Resources Listing

Pi shows loaded context files, skills, prompts, extensions, themes in the chat as collapsible sections.

**Rab**: ❌ Not implemented.

---

## 4. Missing Theme Colors / Tokens

Pi dark.json has 44 color tokens. Rab's theme covers most but is missing:

| Token | Pi Usage | Rab |
|-------|----------|-----|
| `customMessageBg` | Background for custom/compaction/branch/skill messages (#2d2838) | ❌ Missing |
| `customMessageText` | Text color for custom messages | ❌ Missing |
| `customMessageLabel` | Label color for `[skill]`, `[compaction]`, `[branch]` (#9575cd) | ❌ Missing |
| `toolDiffAdded` | Added lines in diff (green) | ❌ Missing |
| `toolDiffRemoved` | Removed lines in diff (red) | ❌ Missing |
| `toolDiffContext` | Context lines in diff (gray) | ❌ Missing |
| `bashMode` | Bash command border color (green) | ❌ Missing |
| `thinkingOff` | Thinking level border color (darkGray) | ❌ Missing |
| `thinkingMinimal` | Thinking level border color (#6e6e6e) | ❌ Missing |
| `thinkingLow` | Thinking level border color (#5f87af) | ❌ Missing |
| `thinkingMedium` | Thinking level border color (#81a2be) | ❌ Missing |
| `thinkingHigh` | Thinking level border color (#b294bb) | ❌ Missing |
| `thinkingXhigh` | Thinking level border color (#d183e8) | ❌ Missing |
| `syntaxComment` | Syntax highlighting (#6A9955) | ❌ Missing |
| `syntaxKeyword` | Syntax highlighting (#569CD6) | ❌ Missing |
| `syntaxFunction` | Syntax highlighting (#DCDCAA) | ❌ Missing |
| `syntaxVariable` | Syntax highlighting (#9CDCFE) | ❌ Missing |
| `syntaxString` | Syntax highlighting (#CE9178) | ❌ Missing |
| `syntaxNumber` | Syntax highlighting (#B5CEA8) | ❌ Missing |
| `syntaxType` | Syntax highlighting (#4EC9B0) | ❌ Missing |
| `syntaxOperator` | Syntax highlighting (#D4D4D4) | ❌ Missing |
| `syntaxPunctuation` | Syntax highlighting (#D4D4D4) | ❌ Missing |
| `mdCodeBlockBorder` | Code block border color (gray) | ❌ Missing |
| `borderAccent` | Accent border (cyan) | ❌ Missing (has `border` only) |
| `mdLinkUrl` | Link URL color (dimGray) | ❌ Missing |

## 5. Streaming Gaps

| Gap | Description |
|-----|-------------|
| Progressive assistant message | Pi uses a persistent `StreamingComponent` (AssistantMessageComponent) that gets updated via `message_update` events. Rab flushes text as atomic `AssistantText` blocks, losing ability to update in-place |
| Pending thinking rendering | Pi renders thinking content with background color during streaming. Rab renders as simple text without background when flushed |
| Tool execution progress | Pi updates tool execution components via `tool_execution_update` events. Rab's tool results arrive as final results only |
| Elapsed timer | Pi's bash tool has a 1-second interval timer updating elapsed time during execution. Rab doesn't track elapsed time |

## 6. Architecture Gaps

| Gap | Pi | Rab |
|-----|----|-----|
| `Expandable` interface | Components implement `setExpanded(boolean)` for global toggle | ❌ No such interface |
| `renderShell: "self"` | Edit tool controls its own framing (box, borders) | ❌ Tool renderers can't self-frame |
| `ToolRenderContext` | Rich context: `args`, `toolCallId`, `cwd`, `executionStarted`, `argsComplete`, `isPartial`, `expanded`, `showImages`, `isError`, `state`, `invalidate()` | ❌ Not available in rab's ToolOutput model |
| `renderCall()` + `renderResult()` | Each tool provides two render functions returning `Component` objects | ⚠️ Rab has `render_call()` / `render_result()` on `AgentTool` but they return plain strings, not components |
| Component reuse | Pi reuses `Text` components across renders (passes `lastComponent`), enabling incremental updates (write tool, bash timer) | ❌ Components not reused between renders |
| Async diff rendering | Edit tool computes diff asynchronously and renders result in-place via `invalidate()` | ❌ Not implemented |

---

## Summary Priority

### High (visible to user, day-to-day interaction)
1. Per-tool rendering: read (compact labels, syntax), bash (`$ command`, elapsed, timeout), edit (diff preview), write (syntax), ls
2. Proper tool bg transitions: pending→success(green)→error(red)
3. Editor border color: thinking level + bash mode
4. Status line dedup (no accumulation of "Cleared", "Tool output: collapsed", etc.)
5. Missing theme tokens: `toolDiffAdded/Removed/Context`, `bashMode`, `customMessage*`, thinking level colors, syntax colors
6. Thinking block expanded/collapsed toggle (hide → label without bg)

### Medium (nice-to-have, completeness)
7. Custom message component (for extensions)
8. Compaction/branch/skill message components
9. OSC133 zones on all message types (or ensure they're applied correctly)
10. Expandable interface with global toggle for all message types
11. Loaded resources header with expand/collapse
12. Streaming assistant message progressive updates (not atomic flush)

### Low (polish)
13. codeBlockIndent in markdown theme
14. Async edit diff rendering
15. Elapsed timer during bash execution
16. Per-component `lastComponent` reuse for incremental updates
