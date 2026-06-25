# Todo

## Session management — pi parity

### Storage & repository layer

- [ ] **Extract `SessionStorage` trait** — abstract over in-memory vs JSONL storage (pi has `InMemorySessionStorage` + `JsonlSessionStorage` impls)
- [ ] **Extract `SessionRepo` trait** — unified `create`/`open`/`list`/`delete`/`fork` interface (pi has `JsonlSessionRepo` + `InMemorySessionRepo`)
- [ ] **`SessionRepo.list` with progress callback** — concurrent session-file loading with progress reporting and cwd filtering
- [ ] **`SessionRepo.list_all`** — list sessions across all project directories with concurrency limit (pi uses 10 concurrent loads)

### Event-driven persistence & lifecycle

- [ ] **Add event-driven auto-persistence** — subscribe to `AgentEvent` stream and persist messages on `message_end` instead of batch-append after loop
- [ ] **Create `AgentSession` lifecycle layer** — bridges agent loop and session manager, handling model/thinking/tool changes automatically
- [ ] **Persist model change entries** (`append_model_change`) when model is switched at runtime
- [ ] **Persist thinking level change entries** (`append_thinking_level_change`) when level changes at runtime
- [ ] **Persist active tools change entries** (`append_active_tools_change`) when tool set changes at runtime

### Compaction

- [ ] **Token estimation** — estimate token counts for messages (input/output/cache) needed for compaction decisions
- [ ] **Context overflow detection** — detect when context window is near capacity (pi checks `isContextOverflow`)
- [ ] **`shouldCompact()` logic** — thresholds for auto-compaction (reserve tokens, keep recent tokens)
- [ ] **`prepareCompaction()`** — identify first kept entry, messages to summarize, split-turn handling
- [ ] **`compact()` engine** — summarize messages via LLM, append compaction entry, return summary
- [ ] **Auto-compaction on agent end** — check and run compaction after each assistant response
- [ ] **Manual compaction** — expose as `/compact` command

### Branch summarization

- [ ] **`collectEntriesForBranchSummary()`** — gather entries from abandoned branch for summarization
- [ ] **`generateBranchSummary()`** — summarize abandoned branch via LLM
- [ ] **`branchWithSummary()` integration** — when user navigates to a different branch point, summarize the abandoned path

### Session switching at runtime

- [ ] **`/new` command** — create a new empty session at runtime (already listed below, needs agent-session lifecycle)
- [ ] **`/fork` command** — fork the current session at a specific point
- [ ] **`/session` command** — switch to a different existing session (needs session picker)
- [ ] **Session resource cleanup** — `cleanupSessionResources()` on session switch/dispose
- [ ] **Parent session tracking** — display fork chain (parent session path) in session info

### Session picker / UI

- [ ] **Session picker component** — interactive listing of all sessions with search, cwd filter, name display
- [ ] **Session picker progress** — show loading progress when scanning many session files
- [ ] **Session info display** — show message count, first message preview, creation/modified time, parent session

### Session export

- [ ] **HTML export** — export session to HTML with tool result rendering (pi has `exportSessionToHtml`)

### Compaction — supporting data

- [ ] **`details` & `fromHook` fields** — `CompactionEntry` and `BranchSummaryEntry` need `details` and `fromHook` support (currently missing in serialization/UI)
- [ ] **Compaction settings** — per-session config for `enabled`, `reserveTokens`, `keepRecentTokens`

## Active / Pi alignment

- [ ] **Markdown indentation inside code blocks:** Indentation compounds on each render, not matching pi.
- [ ] **Write tool output:** Lines don't match screen width, styling/wrapping differ from pi. Needs 1:1 alignment.
- [ ] **Welcome message:** Doesn't look 1:1 identical with pi.
- [ ] **Slash command autocomplete:** Doesn't show hints like pi. Needs 1:1 alignment.
- [ ] **`/session` command:** doesn't work. Says "no session". Needs alignment with pi behavior.
- [ ] **`/new` command:** Needs alignment with pi behavior.
- [ ] **Check unused dependencies**: All dependencies verified as used. No unused crates found.
