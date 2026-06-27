# rab-specific guidelines
- run `cargo fmt && cargo check && cargo clippy` after every code change - all three must pass clean with zero warnings
- reference pi source code in `~/src/cvstree/pi/` for inspiration
- reference yoagent source code in `~/src/cvstree/yoagent/` for reference
- mcp implementation inspired by pi-mcp-adapter
- pi-mcp-adapter source code in `~/src/cvstree/pi-mcp-adapter/`
- agent specific ui components in `src/agent/ui`
- UI should use reusable components from `src/tui/` where possible
- use crossterm and terminal directly only in `src/tui/`
- use latest stable version of all dependencies
