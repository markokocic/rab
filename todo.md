# Remaining work

## Slash command autocomplete
- [ ] in pi, selector appears as soon as user types `/`, in rab you must type `/` + Tab
- [ ] selector for slash command has plain styling, should match pi both visually and behaviourally

## Review reusable TUI components
- [ ] review usage of reusable tui components in app layer (messages.rs, help.rs, footer.rs)
- [ ] assistant text should render markdown (bold, code, headings, links, quotes) with pi theme colors

## Built-in tools
- [ ] review each builtin tool: check if behaviour and rendering matches pi
  - [ ] bash
  - [ ] read
  - [ ] write
  - [ ] edit

## Scrolling
- [ ] wire up mouse wheel events (crossterm MouseEvent) to scroll chat
- [ ] wire up Page Up/Down and arrow keys (when editor is not focused) to scroll chat
- [ ] add scrollbar or scroll indicators

## Visual polish
- [ ] per-thinking-level colors (pi has 6 levels: off→xhigh)
- [ ] footer token display padding fix on narrow terminals
- [ ] tool call lines bold tool name (already done via theme.bold)

## Phase 1 remaining
- [ ] `adapter/genai.rs` — multiple backends (Anthropic, OpenAI, Google, Ollama)
- [ ] `system_prompt.rs` — AGENTS.md, CLAUDE.md loading, project context
- [ ] `compaction.rs` — context window compaction
- [ ] Hook pipeline — `before_tool_call`, `after_tool_call`, `CancellationToken`
- [ ] Steering / follow-up queues — runtime message injection
- [ ] Tool execution modes — sequential mode
- [ ] Compile-time user extensions — `--no-extensions` flag
- [ ] `~/.rab/models.json` — custom provider/model definitions
- [ ] Image support — read tool detects image files, multimodal payload
- [ ] Bash security — command deny-list
- [ ] `rab plugin new` — scaffold extension crate
