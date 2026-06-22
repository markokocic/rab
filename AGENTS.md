# rab-specific guidelines
- run `cargo fmt && cargo check && cargo clippy` after every code change - all three must pass clean with zero warnings
- reference pi source code in `~/src/cvstree/pi/` for inspiration
- use latest stable version of all dependencies
- agent specific ui components in `src/agent/ui`
- UI should use reusable components from `src/tui/` where possible
- use crossterm and terminal directly only in `src/tui/`

