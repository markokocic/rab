# Remaining Gaps — Implementation Plan

---

## Gap 1: Bash spinner uses hardcoded ANSI colors

**File**: `src/agent/ui/components/bash_execution.rs`

**Fix**: Replace hardcoded ANSI codes with `theme.fg_ansi("bashMode")` and `theme.fg_ansi("muted")`.

**Effort**: 10 minutes

---

## Gap 2: Tool call/result background transitions

Pi renders one `ToolExecutionComponent` with background that transitions: `toolPendingBg` → `toolSuccessBg`/`toolErrorBg`. Rab renders two separate components.

**Fix**: Create `ToolExecComponent` combining call + result:

```rust
pub struct ToolExecComponent {
    name: String,
    args: serde_json::Value,
    output: Option<String>,
    is_error: bool,
    is_complete: bool,
    expanded: bool,
}
```

Rendering: if `!is_complete` → pending bg with just header. If complete → success/error bg with header + output.

**Pattern**: Use `Weak<RefCell<ToolExecComponent>>` in App (like pi's `pendingTools` Map):

```rust
// In handle_agent_event:
// ToolCall → create ToolExecComponent, add to chat, store Weak in pending_tools
// ToolResult → find via pending_tools[id], call set_result()
```

**Effort**: 2 hours

---

## Gap 3: Syntax highlighting for read results

Rab's theme.rs has NO syntax highlighting infrastructure. Pi's theme.ts has `highlightCode()`, `getLanguageFromPath()`, `getCliHighlightTheme()` using `cli-highlight` NPM package.

**Need to add to rab**: A syntax highlighting module in `src/tui/` or `src/utils/` that:
1. Maps file extensions to language names (`getLanguageFromPath`)
2. Calls a Rust syntax highlighting library (syntect, which is in Cargo.toml)
3. Produces ANSI-styled strings using theme syntax colors (`syntaxComment`, `syntaxKeyword`, etc.)

**Check if syntect is available**:
```bash
grep syntect Cargo.toml
```

If syntect is a dependency, add a `highlight.rs` module:
```rust
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;

pub fn highlight_code(code: &str, lang: &str) -> Vec<String> {
    // Use syntect with theme colors
}
```

Then in `ToolResult { name: "read" }` handler, call `highlight_code(content, path_extension)`.

**Effort**: 3-4 hours

---

## Gap 4: Result truncation/preview

Pi's tool result rendering shows truncated preview when collapsed ("5 more lines, expand to see"). Rab shows all content.

**Fix**: In `ToolExecComponent::render()` (from Gap 2):
- When `!expanded`: show first 10 lines + "... (N more lines, expand to see)"
- When `expanded`: show all content

**Files**: `src/agent/ui/components/tool_messages.rs` (new ToolExecComponent)

**Effort**: 30 minutes (part of Gap 2)

---

## Gap 5: Edit tool diff rendering

Pi's edit tool uses `renderShell: "self"` to render its own diff preview with intra-line word-level highlighting using `toolDiffAdded`/`toolDiffRemoved`/`toolDiffContext` colors.

**Fix**: 
1. Create `EditDiffComponent` that renders the diff string with colored lines
2. Parse `---`/`+++` diff format and apply `toolDiffRemoved`/`toolDiffAdded`/`toolDiffContext` colors
3. For single-line changes, apply intra-line word diff (inverse highlighting on changed words)

**Files**: 
- New: `src/agent/ui/components/edit_diff.rs`
- Modify: `src/agent/ui/app.rs` — handle edit results differently

**Effort**: 2 days (complex — need diff parsing + word-level diff algorithm)

---

## Gap 6: Progressive streaming

Pi emits `message_start`, `message_update`, `message_end`, `tool_execution_start`, `tool_execution_update`, `tool_execution_end` events. Rab only has `TextDelta`, `ThinkingDelta`, `ToolCall`, `ToolResult`, `TurnEnd`, `AgentEnd`.

**Fix**: Add new events to `AgentEvent` in `src/agent/loop.rs`:

```rust
pub enum AgentEvent {
    // existing...
    MessageStart { role: String },
    MessageUpdate { content: String },
    MessageEnd,
    ToolExecutionStart { id: String, name: String, args: Value },
    ToolExecutionUpdate { id: String, output: String, elapsed_secs: f64 },
    ToolExecutionEnd { id: String, result: String, is_error: bool },
}
```

Then in the event handler, instead of accumulating text and flushing at `TurnEnd`, append to a persistent `AssistantMessageComponent` on each `TextDelta`.

**Files**: `src/agent/loop.rs`, `src/agent/ui/app.rs`, `src/agent/mod.rs`

**Challenge**: The agent loop currently sends `ToolResult` after collecting all output. It needs to send intermediate `ToolExecutionUpdate` events during execution. This requires changes to the streaming provider and agent loop.

**Effort**: 3-5 days (significant refactoring of agent loop)

---

## Gap 7: Header with keybinding hints

Pi shows an `ExpandableText` header with pi logo, keybinding hints (compact/expanded modes), and onboarding text. Rab just shows "rab".

**Fix**: Create `HeaderComponent` in `src/agent/ui/components/header.rs`:

```rust
pub struct HeaderComponent {
    expanded: bool,
}
```

Implement `Component` + `set_expanded`. When expanded show 15+ keybinding hints. When collapsed show 5 compact hints.

**Files**: 
- New: `src/agent/ui/components/header.rs`
- Modify: `src/agent/ui/app.rs` — replace header_section lines with HeaderComponent

**Effort**: 1 hour

---

## Gap 8: Loaded resources listing

Pi shows collapsible sections for loaded context files, skills, prompts, extensions, themes at startup.

**Fix**: In `App::new()`, after setting up startup resources, add `InfoMessageComponent`s listing them to `chat_container`. Use pi's format: `[Context] skills, prompts, ...` with scope info.

**Files**: `src/agent/ui/app.rs` — in `App::new()` constructor, after line ~210

**Effort**: 30 minutes

---

## Gap 9: Footer git branch

Pi watches git branch changes via `FooterDataProvider`. Rab calls `footer.set_git_branch()` from config but never runs git.

**Fix**: In `run()`, spawn a background task that periodically runs `git rev-parse --abbrev-ref HEAD` and updates the footer:

```rust
let footer = app.footer.clone();
let cwd = app.cwd.clone();
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        if let Ok(output) = tokio::process::Command::new("git")
            .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--abbrev-ref", "HEAD"])
            .output().await
        {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !branch.is_empty() {
                footer.borrow_mut().set_git_branch(Some(branch));
            }
        }
    }
});
```

**Effort**: 30 minutes

---

## Implementation Order

| Priority | Gap | Effort | Why this order |
|----------|-----|--------|----------------|
| 1 | Bash spinner colors | 10min | Trivial fix, visible |
| 2 | Tool call/result transitions | 2h | Major visual improvement |
| 3 | Result truncation/preview | (part of #2) | In same component |
| 4 | Header with hints | 1h | Visible at every startup |
| 5 | Loaded resources | 30min | Visible at startup |
| 6 | Footer git branch | 30min | Useful in multi-branch work |
| 7 | Syntax highlighting | 3-4h | Requires syntect integration |
| 8 | Edit diff rendering | 2d | Complex, independent |
| 9 | Progressive streaming | 3-5d | Major refactoring, saved for last |
