# Todo

## Active / Pi alignment

- [ ] **Scrolling broken in chat screen:** Once you scroll up with the mouse, you can't scroll back down.
- [ ] **Agentic loop freeze:** Sometimes after a few rounds the screen freezes and rab stops responding to any input. Only recoverable via `pkill -9 rab` from another terminal.
- [ ] **Autocomplete of `/` commands — align to pi:** In pi, `/q<enter>` closes pi. In rab, `/q<enter>` autocompletes to `/quit` and needs a second enter. Should close immediately like pi.
- [ ] **Model thinking settings reset + editor borders:** Thinking setting gets reset to off intermittently. Editor border colors don't reflect the thinking setting. Should behave like pi.
- [ ] **Markdown indentation inside code blocks:** Indentation compounds on each render, not matching pi.
- [ ] **Write tool output:** Lines don't match screen width, styling/wrapping differ from pi. Needs 1:1 alignment.
- [ ] **Edit tool diff:** Should be line-based, not character-based. Current diff is ugly.
- [ ] **Bash tool duration:** All show 1.0s — duration not properly updated during streaming.
- [ ] **Welcome message:** Doesn't look 1:1 identical with pi.
- [ ] **Slash command autocomplete:** Doesn't show hints like pi. Needs 1:1 alignment.
- [ ] **`/new` command:** Needs alignment with pi behavior.
- [ ] **`/session` command:** Needs alignment with pi behavior.

## Remaining (not rendering-related)

These are feature gaps in the agent/tool functionality, not rendering:

- **Missing slash commands** (14 of 22 pi built-ins): /settings, /export, /import, /copy, /compact, /changelog, /scoped-models, /fork, /clone, /trust, /login, /logout, /share, /tree (8 implemented: quit, model, hotkeys, reload, new, resume, session, name)
- **Multi-backend provider**: genai adapter currently only supports OpenCode Go.
- **Context window compaction**: `compact` field exists but no actual compaction/summarization logic.
- **`~/.rab/models.json`**: Not implemented.
- **WASM plugin system**: Not started (Phase 2).
- **MCP adapter**: Not started (Phase 2).
- **Dynamic hot-reload**: Not started.
- **Disable grep, ls, find tools**: These should be removed/disabled as built-in tools.
- **Check unused dependencies**: Run `cargo-udeps` or equivalent to find and remove unused crates.

## Image system gaps (7)

| # | Gap | Status |
|---|-----|--------|
| C4 | TUI `Image` component (Kitty + iTerm2 + fallback) | ⬜ Basic Kitty protocol support in `src/tui/image.rs` (data URL encoding, Kitty sequences) but no `Component` impl |
| C5 | Terminal capabilities detection (`getCapabilities()`) | ❌ Open |
| C6 | Cell dimension tracking for pixel-accurate sizing | ❌ Open |
| C7 | Image resize utility | ❌ Open |
| C8 | Image convert utility | ❌ Open |
| C9 | Clipboard image paste | ❌ Open |
| C10 | Show images selector UI | ❌ Open |

## UI component gaps (10)

| # | Gap | Status |
|---|-----|--------|
| C12 | Session selector + search | ❌ Open (CommandResult::OpenSessionSelector exists, no UI impl) |
| C13 | Theme selector overlay | ❌ Open |
| C14 | Thinking level selector | ❌ Open |
| C15 | Extension editor / input / selector | ❌ Open |
| C16 | Config / settings selector | ❌ Open |
| C17 | Model selector improvements | ❌ Open (basic ModelSelector exists via SelectList overlay) |
| C18 | OAuth login dialog | ❌ Open |
| C19 | Trust selector | ❌ Open |
| C20 | First-time setup | ❌ Open |
