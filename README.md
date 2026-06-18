# 🚧 rab

> ⚠️ **Work in Progress** — This project is under active development. APIs, features, and documentation are subject to change.

**rab** is a lightweight, extensible, Rust-based coding agent.

Inspired by [pi coding agent](https://pi.dev).

## 📛 Name

**rab** is an archaic Slavic word for *slave* or *servant*, commonly found in the phrase **Раб Божији** (*Rab Božiji*) — *Servant of God*. It shares the same origin with a **robot**, carrying the same notion of a servant who performs work on behalf of another — a fitting name for an agent broker that orchestrates tireless AI agents. Some call coding agents *clankers* — a term that evokes clumsy, rattling machinery. *rab* is the opposite: a quiet, devoted servant, faithful rather than noisy.

## ⚡ Quick Start

### Install via cargo (from source)

Since there are no pre-built releases yet, the only way to install **rab** is
straight from the repository using `cargo`.

**Prerequisites:** [Rust](https://rustup.rs/) (latest stable toolchain)

#### Option A: Install directly from git

```bash
cargo install --git https://github.com/markokocic/rab.git
```

#### Option B: Clone and build locally

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

## ⚖️ License

Copyright © 2026-present Marko Kocic <marko@euptera.com>

This project is licensed under the **Eclipse Public License 2.0 (EPL-2.0)** — see the [LICENSE](LICENSE) file for details.
