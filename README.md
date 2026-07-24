# rab

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo.svg" alt="rab logo" width="480">
  </picture>
</p>

> ⚠️ **Work in Progress** — This project is under active development, but already usable with some caveats. APIs, features, and configuration are subject to change.

**rab** is a lightweight, extensible, Rust-based coding agent.

Inspired by [pi coding agent](https://pi.dev).

rab uses [yoagent](https://crates.io/crates/yoagent) as its core agentic loop and provider framework.
Model and provider metadata is fetched from [models.dev](https://models.dev) via the `rab generate-models`
command — see [Generating models](#-generating-models) below.

## Features

- **Multi-provider support** — OpenAI-compatible (OpenCode, DeepSeek, GitHub Copilot, etc.),
  Anthropic Messages API, Google Generative AI, OpenAI Responses API. Auto-detected from model config.
- **Rich TUI** — crossterm-based terminal UI with markdown rendering, syntax highlighting,
  image display (Kitty protocol), themes, and customizable keybindings.
- **Interactive & non-interactive modes** — full TUI for chat-style interaction, or pipe-friendly
  print mode for scripts.
- **Session management** — persistent sessions with JSONL storage, session tree, forking,
  branching, and compaction (context window management).
- **Built-in tools** — read, write, edit, bash, grep, find, ls — all with pluggable operations
  for remote execution support.
- **Tree-sitter AST tools** — `list_symbols`, `find_definition`, `find_callers`, `get_symbol_body`,
  `find_callees` for 20+ languages.
- **MCP support** — Model Context Protocol servers via SSE or WebSocket transport.
- **Slash commands** — 22 built-in commands (`/model`, `/settings`, `/extensions`, `/fork`,
  `/compact`, `/export`, `/import`, etc.).
- **Extension system** — uniform Extension trait for built-in tools and user extensions.
  Hook system for before/after tool call interception.
- **OAuth** — Device code flow (RFC 8628) for headless authentication. GitHub Copilot OAuth
  with auto-model-fetch.
- **Configurable** — JSON settings with deep merge (global + project-local overrides),
  custom system prompts, skills, prompt templates, themes.
- **Session compaction & branch summarization** — automatic and manual compaction to manage
  context window. Branch navigation with abandoned branch summarization.

## 📛 Name

**rab** is an archaic Slavic word for *slave* or *servant*, commonly found in the phrase **Раб Божији** (*Rab Božiji*) — *Servant of God*. It shares the same origin with a **robot**, carrying the same notion of a servant who performs work on behalf of another — a fitting name for an agent broker that orchestrates tireless AI agents. Some call coding agents *clankers* — a term that evokes clumsy, rattling machinery. *rab* is the opposite: a quiet, devoted servant, faithful rather than noisy.

## ⚡ Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable toolchain, edition 2024 requires Rust 1.96.0+)

### Install via cargo from git (recommended)

```bash
cargo install --git https://github.com/markokocic/rab.git
```

### Clone and build locally

```bash
git clone https://github.com/markokocic/rab.git
cd rab
cargo build --release
./target/release/rab
```

Or to install the binary:

```bash
cargo install --path .
rab
```

## 🧩 Generating Models

Model and provider information (base URLs, model costs, capabilities, compat flags) is sourced from [models.dev](https://models.dev). To update the local catalog:

```bash
rab generate-models
```

This fetches provider/model data for GitHub Copilot, OpenCode (Zen), OpenCode Go, and DeepSeek,
applies pi-style corrections (Anthropic Messages compat for Claude models, DeepSeek thinking format,
etc.), and writes to `src/provider/models.json`. User overrides in `~/.rab/agent/models.json`
are preserved and merged at runtime.

## 🚀 Usage

### Interactive mode (TUI)

```bash
rab
```

### Non-interactive / pipe mode

```bash
rab "list all .rs files in the project"
echo "explain this code" | rab --no-session
```

### Session management

```bash
rab -c                          # Continue most recent session
rab -r                          # Open interactive session picker
rab --session 01J...abc         # Open specific session by partial ID
rab --fork path/to/session.jsonl  # Fork a session
rab --no-session                # Ephemeral mode, don't save
```

## ⚙️ Configuration

Settings are stored in `~/.rab/agent/settings.json` with optional project-local overrides
in `.rab/settings.json`. Same schema as pi.

Key settings:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `defaultModel` | string | `"deepseek-v4-flash"` | Default model |
| `defaultProvider` | string | `"opencode_go"` | Default provider |
| `defaultThinkingLevel` | string | `"high"` | Thinking level |
| `theme` | string | `"dark"` | TUI theme |
| `transport` | string | `"sse"` | MCP transport preference |
| `steeringMode` | string | `"one-at-a-time"` | Turn steering mode |
| `followUpMode` | string | `"all"` | Follow-up mode |
| `extensions` | string[] | `[]` | User extension paths |
| `skills` | string[] | `[]` | Additional skill paths |
| `prompts` | string[] | `[]` | Additional prompt paths |

## 📁 Storage Layout

```
~/.rab/
├── agent/
│   ├── settings.json           # Global settings
│   ├── auth.json               # API keys and OAuth credentials
│   ├── models.json             # User model overrides
│   ├── SYSTEM.md               # Custom system prompt
│   ├── APPEND_SYSTEM.md        # Appended system prompt
│   ├── AGENTS.md               # Global context file
│   ├── mcp.json                # MCP server configuration
│   └── prompts/                # Prompt templates (.md files)
├── keybindings.json            # Custom keybindings
├── skills/                     # Agent skills (SKILL.md files)
├── themes/                     # TUI themes
└── sessions/                   # Session files (JSONL)
    └── <cwd-hash>/
        ├── 01J...abc.jsonl
        └── 01J...def.jsonl
```

## 🏗️ Architecture

rab is a two-crate workspace:

- **`rab-agent`** (`src/`) — Core agent logic, CLI, TUI, built-in tools, extensions,
  provider layer, settings, auth. ~52K lines.
- **`rab-tui`** (`tui/`) — Reusable terminal UI framework (components, editor,
  markdown rendering, image display, themes). ~5.6K lines.

Key dependencies: [yoagent](https://crates.io/crates/yoagent) 0.13.1 (agent loop, types,
providers), crossterm 0.29 (TUI), tokio (async), tree-sitter 0.26 (AST tools).

See [arch.md](arch.md) for the full architecture documentation.

## 🧪 Testing

```bash
cargo test          # Run all tests
cargo test --test   # Run integration tests
cargo clippy        # Lint
cargo fmt           # Format
```

## ⚖️ License

Copyright © 2026-present Marko Kocic <marko@euptera.com>

This project is licensed under the **Eclipse Public License 2.0 (EPL-2.0)** — see the [LICENSE](LICENSE) file for details.
