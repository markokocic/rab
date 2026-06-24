# Todo

## Active / Pi alignment

- [ ] When I press ESC while agent loop is active, it displays "Agent loop terminated unexpectedly (panic or abort). Check stderr for details.". Instaed, it should just abort normally, as Pi does.
- [ ] **Enter key in slash command completion doesn't execute:** When selecting a slash command from the autocomplete dropdown, pressing Enter should execute the command immediately (matching pi behaviour), not just insert the text.
- [ ] **Markdown indentation inside code blocks:** Indentation compounds on each render, not matching pi.
- [ ] **Write tool output:** Lines don't match screen width, styling/wrapping differ from pi. Needs 1:1 alignment.
- [ ] **Edit tool diff:** Should be line-based, not character-based. Current diff is ugly.
- [ ] **Bash tool duration:** All show 1.0s - duration not properly updated during streaming.
- [ ] **Welcome message:** Doesn't look 1:1 identical with pi.
- [ ] **Slash command autocomplete:** Doesn't show hints like pi. Needs 1:1 alignment.
- [ ] **`/new` command:** Needs alignment with pi behavior.
- [ ] **`/session` command:** Needs alignment with pi behavior.
- [x] **Check unused dependencies**: All dependencies verified as used. No unused crates found.

## Remaining (not rendering-related)

These are feature gaps in the agent/tool functionality, not rendering:

- [x] **Disable grep, ls, find tools**: Created separate extension tools in `src/extensions/` (grep.rs, ls.rs, find.rs) and removed the special command detection from the bash built-in. These tools are disabled by default (not registered in main.rs).

