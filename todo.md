# Remaining work

## chat editor
- [ ] check if pi have separate editor and chat editor components. Where are they defined Does rab do the same?
- [ ] file autocomplete

## Slash command autocomplete
- [ ] in pi, selector appears as soon as user types `/`, in rab you must type `/` + Tab
- [ ] selector for slash command has plain styling, should match pi both visually and behaviourally

## Review reusable TUI components
- [ ] review usage of reusable tui components in app layer (messages.rs, help.rs, footer.rs)
- [ ] assistant text should render markdown (bold, code, headings, links, quotes) with pi theme colors

## Built-in tools
- [x] review each builtin tool: check if behaviour and rendering matches pi 1/1
  - [ ] bash
  - [x] read — line accumulation truncation, firstLineExceedsLimit, trimTrailingEmptyLines, formatSize, compact labels, cancel, prompt guidelines
  - [ ] write
  - [x] edit — BOM, line ending (CRLF/LF), fuzzy matching, input normalization, diff output, better errors, prompt guidelines

## Messages
- [ ] review rendering for thinking messages.
- [ ] check other message types

## Scrolling
- [ ] wire up mouse wheel events (crossterm MouseEvent) to scroll chat
- [ ] wire up Page Up/Down and arrow keys (when editor is not focused) to scroll chat
- [ ] add scrollbar or scroll indicators

## Visual polish
- [ ] per-thinking-level colors (pi has 6 levels: off→xhigh)
- [ ] footer token display padding fix on narrow terminals
- [ ] tool call lines bold tool name (already done via theme.bold)

## Done
- [x] `system_prompt.rs` — AGENTS.md/CLAUDE.md loading, project context, SYSTEM.md, APPEND_SYSTEM.md
- [x] `context_files.rs` — context file discovery (ancestor walk)
- [x] `skills.rs` — load skills, format for prompt, `/skill:name` expansion
- [x] `--no-context-files`, `--system-prompt`, `--append-system-prompt` CLI flags
- [x] Startup resource listing (context files, skills) in welcome message

## Phase 1 remaining
- [ ] `adapter/genai.rs` — multiple backends (Anthropic, OpenAI, Google, Ollama)
- [ ] `compaction.rs` — context window compaction
- [ ] Hook pipeline — `before_tool_call`, `after_tool_call`, `CancellationToken`
- [ ] Steering / follow-up queues — runtime message injection
- [ ] Tool execution modes — sequential mode
- [ ] Compile-time user extensions — `--no-extensions` flag
- [ ] `~/.rab/models.json` — custom provider/model definitions
- [ ] Image support — read tool detects image files, multimodal payload
- [ ] Bash security — command deny-list
- [ ] `rab plugin new` — scaffold extension crate
