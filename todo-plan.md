# Pi Message Rendering — 1:1 Architecture & Visual Plan

## The Core Difference

| Aspect | Pi | Rab |
|--------|----|-----|
| Rendering model | **Component tree** — TUI extends Container, all UI elements are child Components, rendering is recursive | **Imperative string building** — `compose_ui()` builds `Vec<String>` manually |
| Message representation | Inline Component instances added/removed from containers | `DisplayMsg` enum as intermediate representation, then rendered to strings |
| Streaming updates | Mutable references to components (`this.streamingComponent`, `Map<id, ToolExecutionComponent>`) mutated via method calls | Atomic flush when streaming ends |

**To match pi 1:1, the message rendering must become component-based, not string-based.**

This means eliminating `compose_ui()` as a monolithic string builder and instead making the chat area a proper `Container` of `Component` children, exactly like pi's `this.chatContainer`.

---

## Architecture — Current vs Target

### Current (rab)

```
┌─ main.rs ─────────────────────────────────┐
│  tui.render(lines, width, height, stdout)  │
│         ▲                                  │
│         │ Vec<String>                      │
│  compose_ui(app, width, height)            │
│         ▲                                  │
│         │ Vec<String>                      │
│  render_messages(messages, width, ...)     │
│         ▲                                  │
│         │ DisplayMsg[] → strings           │
│  handle_agent_event() → push DisplayMsg    │
└────────────────────────────────────────────┘
```

### Target (matching pi)

```
┌─ main.rs ──────────────────────────────────┐
│  tui.render(width)  // recursive Component  │
│         ▲                                   │
│         │ (Component tree)                  │
│  TUI (extends Container)                    │
│    ├─ headerContainer: Container            │
│    ├─ chatContainer: Container              │
│    │   ├─ UserMessageComponent              │
│    │   ├─ AssistantMessageComponent         │
│    │   │   ├─ Spacer                        │
│    │   │   └─ Markdown (text)               │
│    │   ├─ Spacer                            │
│    │   ├─ ToolExecutionComponent            │
│    │   │   ├─ Box (bg pending/success/error)│
│    │   │   ├─ Text (call renderer)          │
│    │   │   └─ Text (result renderer)        │
│    │   └─ ...                               │
│    ├─ pendingMessagesContainer: Container   │
│    ├─ statusContainer: Container            │
│    ├─ editorContainer: Container            │
│    ├─ footer: FooterComponent               │
│    └─ widgetContainers                      │
│                                            │
│  handle_agent_event() →                     │
│    chatContainer.addChild(component)        │
│    streamingComponent.updateContent(msg)    │
│    pendingTools.get(id).updateResult(...)   │
└─────────────────────────────────────────────┘
```

---

## Phase 1: Make TUI a Container (like pi)

### 1.1 Make `rab::tui::TUI` extend Container

Pi: `class TUI extends Container` — the TUI IS a Container. It inherits `addChild`, `removeChild`, `clear`, and `render()`.

Rab's `tui_core.rs` currently has `TUI` as a standalone struct with a `Screen` and overlay stack. Change it to also be a Container (composition over inheritance in Rust):

```rust
pub struct TUI {
    pub root: Container,  // root container with all children
    screen: Screen,
    overlay_stack: Vec<OverlayEntry>,
    // ...
}
```

TUI's `render()` should:
1. Recursively render `self.root` (the Container tree)
2. Composite overlays on top
3. Pass to Screen for diff rendering

### 1.2 Create the container tree in App

In rab's `App::new()`:

```rust
// Instead of storing individual containers on App:
pub ui: TUI, // TUI owns the root container

// In new():
let mut ui = TUI::new();
let chat_container = Container::new();
let status_container = Container::new();
let editor_container = Container::new();
let footer = FooterComponent::new();

// Build tree: TUI.root has all children
ui.root.add_child(Box::new(chat_container));  // index 0 = chat
ui.root.add_child(Box::new(status_container)); // index 1 = status
ui.root.add_child(Box::new(editor_container)); // index 2 = editor
ui.root.add_child(Box::new(footer));           // index 3 = footer
```

But we need to keep references to the inner containers to add/remove children dynamically. In Rust, this requires `Rc<RefCell<Container>>`:

```rust
pub chat_container: Rc<RefCell<Container>>,
pub status_container: Rc<RefCell<Container>>,
// etc.
```

### 1.3 Replace compose_ui() with TUI.render()

Instead of:
```rust
let lines = compose_ui(app, width, height);
tui.render(lines, width, height, &mut stdout)?;
```

Do:
```rust
tui.set_dimensions(width, height);
tui.render(width, &mut stdout)?;  // TUI renders its component tree
```

The main loop no longer manually builds strings.

---

## Phase 2: Eliminate DisplayMsg Enum

Pi doesn't have a `DisplayMsg` enum. It directly creates component instances and adds them to `chatContainer`. Rab should do the same.

### 2.1 Make message components

Each message type becomes owning its own Component:

```rust
pub struct UserMessageComponent {
    box: Box,  // with userMessageBg
    content: Markdown,
}

pub struct AssistantMessageComponent {
    spacer: Option<Spacer>,
    markdown: Markdown,
    thinking_blocks: Vec<ThinkingBlock>,
    // Mutable reference for streaming updates
}
```

### 2.2 Change agent event handling

Instead of `handle_agent_event()` pushing `DisplayMsg` variants, it directly creates and adds components:

```rust
fn handle_agent_event(app: &mut App, event: AgentEvent) {
    match event {
        AgentEvent::MessageStart { role, content } => {
            let chat = app.chat_container.borrow_mut();
            match role {
                "user" => {
                    chat.add_child(Box::new(UserMessageComponent::new(content)));
                }
                "assistant" => {
                    let comp = AssistantMessageComponent::new();
                    app.streaming_component = Some(Rc::downgrade(&comp));
                    chat.add_child(Box::new(comp));
                }
            }
        }
        AgentEvent::TextDelta { delta } => {
            if let Some(comp) = app.streaming_component.upgrade() {
                comp.append_text(&delta);
                comp.invalidate();
            }
        }
        AgentEvent::ToolCall { name, args, id } => {
            let chat = app.chat_container.borrow_mut();
            let comp = ToolExecutionComponent::new(name, id, args);
            app.pending_tools.insert(id.clone(), Rc::downgrade(&comp));
            chat.add_child(Box::new(comp));
        }
        AgentEvent::ToolResult { id, content, is_error } => {
            if let Some(comp) = app.pending_tools.remove(&id).and_then(|w| w.upgrade()) {
                comp.set_result(content, is_error);
                comp.invalidate();
            }
        }
        // etc.
    }
}
```

### 2.3 Handle streaming with Weak references

Pi uses plain JS references for mutable access to streaming components. In Rust, use:

```rust
// In App:
streaming_component: Option<Weak<RefCell<AssistantMessageComponent>>>,
pending_tools: HashMap<String, Weak<RefCell<ToolExecutionComponent>>>,
```

The `chat_container` holds `Rc<RefCell<dyn Component>>`. The `Weak` references allow mutation without borrowing conflicts.

---

## Phase 3: ToolExecutionComponent (matches pi's exactly)

Port from pi's `tool-execution.ts`:

```rust
pub struct ToolExecutionComponent {
    tool_name: String,
    tool_call_id: String,
    args: serde_json::Value,

    // Render state — matches pi's fields
    content_box: Option<Box>,              // default render shell
    content_text: Text,                    // fallback (no renderer)
    self_render_container: Option<Container>, // "self" render shell
    call_renderer_component: Option<Box<dyn Component>>,
    result_renderer_component: Option<Box<dyn Component>>,
    renderer_state: HashMap<String, Box<dyn Any>>,

    // Image support
    image_components: Vec<Image>,
    image_spacers: Vec<Spacer>,

    // State — matches pi exactly
    expanded: bool,
    is_partial: bool,
    execution_started: bool,
    args_complete: bool,
    result: Option<ToolExecutionResult>,

    // Background function — transitions based on state
    bg_fn: Option<BgFn>,
}
```

Key behaviors matching pi:

1. **Background transitions**: `isPartial` → `toolPendingBg`, `isError` → `toolErrorBg`, else `toolSuccessBg`
2. **Call rendering**: Delegates to tool's `render_call()` or creates fallback `bold(toolTitle, name)`
3. **Result rendering**: Delegates to tool's `render_result()` or calls `createResultFallback()`
4. **Self-render shell**: If tool has `renderShell: "self"`, uses `self_render_container` instead of `content_box`
5. **Image injection**: Images from result content rendered as `Image` + `Spacer` children outside the box
6. **Hide when empty**: When no content and no images, `hideComponent = true`
7. **Expand/collapse**: Controls preview lengths, diff visibility
8. **Invalidation**: `invalidate()` calls `updateDisplay()` to rebuild

### Per-tool renderCall functions

Each tool provides a function `(args, theme, context) -> Box<dyn Component>`:

- **read**: `Text` with `bold(toolTitle, "read")` + path + line range, or compact label for docs/skills/resources
- **write**: `Text` (or custom `WriteCallRenderComponent`) with `bold(toolTitle, "write")` + path + content preview with syntax highlighting
- **edit**: `Box` (self-render) with `bold(toolTitle, "edit")` + path + async diff preview
- **bash**: `Text` with `bold(toolTitle, "$ command")` + timeout suffix
- **ls**: `Text` with `bold(toolTitle, "ls")` + path + limit

### Per-tool renderResult functions

Each tool provides `(result, options, theme, context) -> Box<dyn Component>`:

- **read**: Syntax-highlighted content in `toolOutput`, truncation notices, "X more lines" hint
- **write**: Error text in `toolError`, or empty `Container` on success (bg transition is the indicator)
- **edit**: Diff string rendered with colored + intra-line highlighting, or empty on success
- **bash**: Output in `toolOutput` with preview truncation, duration, truncation warnings. Uses special `BashResultRenderComponent` for width-aware caching
- **ls**: Directory listing in `toolOutput` with "X more entries, Y more lines" hint

---

## Phase 4: Expand/Collapse System

Pi has an `Expandable` interface:

```typescript
interface Expandable {
    setExpanded(expanded: boolean): void;
}
```

Implemented by: `ExpandableText`, all message components, tool execution components.

Global toggle `app.tools.expand` iterates all children:
```typescript
for (const child of this.chatContainer.children) {
    if (isExpandable(child)) {
        child.setExpanded(this.toolOutputExpanded);
    }
}
```

### In rab:

```rust
pub trait Expandable {
    fn set_expanded(&mut self, expanded: bool);
}
```

`ToolExecutionComponent` implements `Expandable`. `ExpandableText` (for startup header) implements it.

The toggle handler iterates `chat_container.children` and calls `set_expanded()` on those that implement `Expandable`.

---

## Phase 5: Streaming — Progressive Updates

Pi's streaming protocol:

1. `message_start` — create `AssistantMessageComponent`, store as `this.streamingComponent`
2. `message_update` — call `this.streamingComponent.updateContent(message)`, iterate tool calls, create/update `ToolExecutionComponent` in `this.pendingTools`
3. `tool_execution_start` — mark tool as `executionStarted`
4. `tool_execution_update` — call `component.updateResult(partialResult, true)`
5. `tool_execution_end` — call `component.updateResult(finalResult, false)`, remove from `pendingTools`
6. `message_end` — finalize, clear `streamingComponent`
7. `agent_end` — clear all pending state

### In rab:

The agent loop (`run_agent_loop`) must emit fine-grained events:
- `AgentEvent::MessageStart { role, message }`
- `AgentEvent::MessageUpdate { message, tool_calls }`
- `AgentEvent::ToolExecutionStart { id, name, args }`
- `AgentEvent::ToolExecutionUpdate { id, partial_result }`
- `AgentEvent::ToolExecutionEnd { id, result, is_error }`
- `AgentEvent::MessageEnd { message }`
- `AgentEvent::AgentEnd`

Currently rab's agent events are:
- `TextDelta`, `ThinkingDelta`, `ToolCall`, `ToolResult`, `AgentEnd`

Need to add the intermediate events.

---

## Phase 6: Markdown with codeBlockIndent

Pi's markdown theme carries `codeBlockIndent` from settings. Rab's `MarkdownTheme` needs this field.

In `src/tui/components/markdown.rs`, add `code_block_indent: usize` to `MarkdownTheme`. During code block rendering, prepend this many spaces to each line.

---

## Phase 7: Editor Border Color

Pi wraps the editor with a border colored by:
- Thinking level: `thinkingOff` → `thinkingXhigh`
- Bash mode: `bashMode` when text starts with `!`

In rab, the editor already has border support. Add method:

```rust
impl Editor {
    pub fn set_border_color_fn(&mut self, f: Option<Box<dyn Fn(&str) -> String>>) { ... }
}
```

Call from `App` when thinking level changes or editor content changes.

---

## Phase 8: Status Message Deduplication

Pi's `showStatus()`:
1. Check if last two children are (Spacer, Text) with matching references
2. If yes, mutate the Text's content instead of adding new children
3. If no, add a new Spacer + Text pair, store references

In rab, with the Container-based chat:
```rust
fn show_status(app: &mut App, msg: &str) {
    let mut chat = app.chat_container.borrow_mut();
    let children = chat.children_mut();
    let len = children.len();

    if let (Some(spacer), Some(text)) = (children.get(len-2), children.get(len-1)) {
        if app.last_status_is_spacer && app.last_status_text_id == ... {
            // Reuse: mutate the text
            if let Some(text) = children[len-1].as_any_mut().downcast_mut::<Text>() {
                text.set_text(theme.fg("dim", msg));
                return;
            }
        }
    }

    // Add new
    chat.add_child(Box::new(Spacer::new(1)));
    let text = Text::new(theme.fg("dim", msg), 1, 0);
    chat.add_child(Box::new(text));
    // Store IDs for next call
}
```

---

## Phase 9: Message Components (Custom, Compaction, Branch, Skill)

Each follows pi's exact design:

### CustomMessageComponent
- Background: `customMessageBg`
- Label: `bold(fg("customMessageLabel", "[type]"))`
- Content: markdown in `customMessageText` color
- Expandable

### CompactionSummaryMessageComponent
- Same purple bg
- Label: `bold(fg("customMessageLabel", "[compaction]"))`
- Expanded: header "Compacted from N tokens" + summary markdown
- Collapsed: single line "Compacted from N tokens (key to expand)"

### BranchSummaryMessageComponent
- Same purple bg
- Label: `bold(fg("customMessageLabel", "[branch]"))`
- Expanded: header "Branch Summary" + summary markdown
- Collapsed: single line "Branch summary (key to expand)"

### SkillInvocationMessageComponent
- Same purple bg
- Label: `bold(fg("customMessageLabel", "[skill]"))` + skill name
- Expanded: header skill name + full content markdown
- Collapsed: single line "[skill] name (key to expand)"

---

## Phase 10: Bash Execution Component (standalone ! commands)

Already exists in rab but needs fixes:
- **Border color**: Use `bashMode` color (green), not hardcoded. `dim` for `!!` commands.
- **Spinner**: Use bashMode color, not hardcoded
- **Hidden thinking bg**: pi doesn't use bg for hidden thinking — remove the thinking_bg wrapping
- **Status line**: Show "(cancelled)" in warning, "(exit N)" in error
- **Truncation**: Show "Output truncated. Full output: path" in warning color

---

## Phase 11: OSC133 Terminal Zones

Pi wraps user and assistant message lines with OSC133 sequences. Already implemented in rab's `messages.rs`. Verify they're applied IN the component, not striped by post-processing. Each component should emit them in its `render()` method (like pi's `AssistantMessageComponent` and `UserMessageComponent` do).

---

## Phase 12: Theme Completeness

Add to `dark.json` and `light.json`:

```json
"customMessageBg": "#2d2838",
"customMessageText": "#d4d4d4",
"customMessageLabel": "#9575cd",
"toolDiffAdded": "#b5bd68",
"toolDiffRemoved": "#cc6666",
"toolDiffContext": "#808080",
"bashMode": "#b5bd68",
"thinkingOff": "#505050",
"thinkingMinimal": "#6e6e6e",
"thinkingLow": "#5f87af",
"thinkingMedium": "#81a2be",
"thinkingHigh": "#b294bb",
"thinkingXhigh": "#d183e8",
"mdCodeBlockBorder": "#808080",
"borderAccent": "#00d7ff",
"mdLinkUrl": "#666666"
```

Add syntax highlighting colors (9 tokens) from pi's dark.json.

---

## Summary: Deviation Points in My Previous Plan

| Previous plan | Pi actual | Fix |
|---------------|-----------|-----|
| 🚫 `compose_ui()` builds strings | ✅ TUI renders Component tree recursively | Make TUI a Container, eliminate compose_ui |
| 🚫 `DisplayMsg` enum → string rendering | ✅ Components added directly to container | Eliminate DisplayMsg, create components in handle_agent_event |
| 🚫 String-based option (Option A) | ✅ Always component-based | No compromise — go full component-based |
| 🚫 `render_messages()` function | ✅ Each component has its own `render()` | Delete `render_messages()`, each component renders itself |
| ⚠️ Mentioned RefCell but didn't detail | ✅ JS has native mutable refs | Use Rc<RefCell<>> + Weak for mutable streaming refs |
| ⚠️ Mentioned Weak refs vaguely | ✅ pi stores direct refs to components | Need concrete Rc/Weak pattern for each mutable reference |
| ✅ ToolExecutionComponent | ✅ Same pattern | Match exactly |
| ✅ Per-tool renderCall/result | ✅ Same pattern | Match exactly |
| ✅ Expandable trait | ✅ Same interface | Match exactly |

### Critical path for 1:1

1. Make TUI a Container (biggest change) — 3 days
2. Port ToolExecutionComponent — 3 days
3. Change agent event handling to create components directly — 4 days
4. Streaming events framework — 2 days
5. All the visual polish (theme, diff, editor border, etc.) — 3 days
