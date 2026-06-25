# Todo

## Session management — pi parity

### Storage & repository layer

- [x] **Extract `SessionStorage` trait** — abstract over in-memory vs JSONL storage (pi has `InMemorySessionStorage` + `JsonlSessionStorage` impls)
- [x] **Extract `SessionRepo` trait** — unified `create`/`open`/`list`/`delete`/`fork` interface (pi has `JsonlSessionRepo` + `InMemorySessionRepo`)
- [x] **`SessionRepo.list` with progress callback** — concurrent session-file loading with progress reporting and cwd filtering
- [x] **`SessionRepo.list_all`** — list sessions across all project directories with concurrency limit (pi uses 10 concurrent loads)

### Event-driven persistence & lifecycle

- [x] **Add event-driven auto-persistence** — subscribe to `AgentEvent` stream and persist messages on `message_end` instead of batch-append after loop
- [x] **Create `AgentSession` lifecycle layer** — bridges agent loop and session manager, handling model/thinking/tool changes automatically
- [x] **Persist model change entries** (`append_model_change`) when model is switched at runtime
- [x] **Persist thinking level change entries** (`append_thinking_level_change`) when level changes at runtime
- [x] **Persist active tools change entries** (`append_active_tools_change`) when tool set changes at runtime

### Compaction

- [x] **Token estimation** — estimate token counts for messages (input/output/cache) needed for compaction decisions
- [x] **Context overflow detection** — detect when context window is near capacity (pi checks `isContextOverflow`)
- [x] **`shouldCompact()` logic** — thresholds for auto-compaction (reserve tokens, keep recent tokens)
- [x] **`prepareCompaction()`** — identify first kept entry, messages to summarize, split-turn handling
- [x] **`compact()` engine** — summarize messages via LLM, append compaction entry, return summary
- [x] **Auto-compaction on agent end** — check and run compaction after each assistant response
- [x] **Manual compaction** — expose as `/compact` command

### Branch summarization

- [x] **`collectEntriesForBranchSummary()`** — gather entries from abandoned branch for summarization
- [x] **`generateBranchSummary()`** — summarize abandoned branch via LLM
- [x] **`branchWithSummary()` integration** — `AgentSession::set_branch()` now automatically summarizes the abandoned path when moving to a different branch point (if a provider is configured)

### Session switching at runtime

- [x] **`/new` command** — create a new empty session at runtime
- [x] **`/fork` command** — fork the current session at a specific point
- [x] **`/session` command** — show session info and stats
- [x] **`cleanupSessionResources()`** — `AgentSession::cleanup_session_resources()` clears persisted message tracking
- [x] **Parent session tracking** — display fork chain (parent session path) in session info

### Session picker / UI

- [x] **Session picker component** — `SessionPicker` state machine built with search, filter, selection, progress
- [x] **Session picker overlay** — integrated inline with keyboard navigation (↑↓ Enter Esc /)
- [x] **Session info display** — show message count, first message preview, creation/modified time, parent session

### Session export

- [ ] **HTML export** — export session to HTML with tool result rendering (pi has `exportSessionToHtml`)

### Compaction — supporting data

- [x] **`details` & `fromHook` fields** — `CompactionEntry` and `BranchSummaryEntry` now support `details` and `fromHook` in serialization and API
- [x] **Compaction settings** — per-session config for `enabled`, `reserveTokens`, `keepRecentTokens` persisted via `settings.json`

## Active / Pi alignment

- [ ] **Markdown indentation inside code blocks:** Indentation compounds on each render, not matching pi.
- [ ] **Write tool output:** Lines don't match screen width, styling/wrapping differ from pi. Needs 1:1 alignment.
- [ ] **Welcome message:** Doesn't look 1:1 identical with pi.
- [ ] **Slash command autocomplete:** Doesn't show hints like pi. Needs 1:1 alignment.
- [x] **`/session` command:** Works in interactive mode — shows session info, stats, and parent session. Also works in print mode now.
- [x] **`/new` command:** Works — creates new empty session and clears conversation.
- [ ] **Check unused dependencies**: All dependencies verified as used. No unused crates found.
