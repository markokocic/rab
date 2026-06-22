# Pi Message Rendering — Implementation Plan ✅ Complete

All phases of the rendering architecture migration are complete.
Rab's TUI rendering matches pi's architecture 1:1.

## Architecture

```
┌─ TUI.render(width, height, stdout) ────────┐
│  recursive Component tree                   │
│                                             │
│  TUI.root (Container)                       │
│    ├─ HeaderComponent (logo + hints)        │
│    ├─ chat_container (RefContainer)         │
│    │   ├─ UserMessageComponent (box+md+bg)  │
│    │   ├─ RcRefCellComponent (streaming)    │
│    │   │   └─ AssistantMessageComponent     │
│    │   ├─ ToolExecComponent (bg trans.)     │
│    │   ├─ BashExecutionComponent (borders)  │
│    │   └─ ...                               │
│    ├─ pending_section (DynamicLines)        │
│    ├─ status_section (DynamicLines)         │
│    ├─ queued_section (DynamicLines)         │
│    ├─ working_section (DynamicLines)        │
│    ├─ EditorComponent (border color)        │
│    └─ FooterComponent (model, tokens, git)  │
│                                             │
│  handle_agent_event() →                     │
│    chat_container.addChild(component)       │
│    streaming_component.upgrade().append()   │
│    pending_tools.get(id).set_result()       │
│    bash_component.upgrade().append_chunk()  │
│                                             │
│  Overlay compositing → Screen diff render   │
└─────────────────────────────────────────────┘
```

## Phases — All Complete

| Phase | Description | Status |
|-------|-------------|--------|
| 1 | TUI as Container, component tree setup | ✅ |
| 2 | ToolExecutionComponent, per-tool renderCall/renderResult | ✅ |
| 3 | Message type Components (User, Assistant, Tool, Bash, Info, Header) | ✅ |
| 4 | Expand/collapse, editor border color, spacers, OSC133 | ✅ |
| 5 | Streaming — progressive assistant + bash output | ✅ |
| 6 | Syntax highlighting (syntect) + diff rendering | ✅ |
| 7 | Theme completeness, git branch, loaded resources | ✅ |

## Key Components

| Component | pi source | rab file | Features |
|-----------|-----------|----------|----------|
| ToolExecComponent | `tool-execution.ts` | `tool_messages.rs` | bg transitions, per-tool formatting, preview truncation, syntax highlight |
| BashExecutionComponent | `bash-execution.ts` | `bash_execution.rs` | borders, spinner, streaming output, duration |
| UserMessageComponent | `user-message.ts` | `user_message.rs` | Box + bg + markdown + OSC133 |
| AssistantMessageComponent | `assistant-message.ts` | `assistant_message.rs` | markdown + thinking blocks |
| HeaderComponent | `ExpandableText` | `header.rs` | logo + expandable keybinding hints |
| RcRefCellComponent | — | `rc_ref_cell_component.rs` | shared ownership wrapper for streaming |
| InfoMessageComponent | — | `info_message.rs` | dim text status messages |
| Diff renderer | `diff.ts` | `diff.rs` | unified diff + intra-line character-level inverse |
| DynamicLines | — | `dynamic_lines.rs` | updateable text sections (pending, status, queued, working) |

## Agent events (rendering-related)

| Event | Purpose | Progress |
|-------|---------|----------|
| TextDelta | Progressive assistant text | ✅ streaming component |
| ThinkingDelta | Progressive thinking blocks | ✅ streaming component |
| ToolProgress | Intermediate bash output | ✅ streamed via tokio async reads |
| ToolCall | Tool invocation | ✅ ToolExecComponent via Rc/Weak |
| ToolResult | Tool completion | ✅ updates pending_tools[id] |
| Aborted | Stream error/abort | ✅ inline error in component |
| TurnEnd/AgentEnd | Stream completion | ✅ clear Weak references |
