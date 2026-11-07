<div align="center">

<img src="assets/mooring-1024.png" alt="Mooring" width="120" />

# Mooring

### Keep every coding agent moored: running, in sight, and never lost.

**A native macOS workbench for coding agents. Every session is a real terminal; Mooring shows which one needs you, keeps them running when the window closes, and brings back the conversation after a crash.**

[![GitHub stars](https://img.shields.io/github/stars/majiayu000/mooring?style=flat-square&color=ffcc00)](https://github.com/majiayu000/mooring)
[![License](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)

[Build from source](#build-from-source) | [What it does](#what-it-does) | [FAQ](#faq) | [中文](README_CN.md)

</div>

---

> **Status:** in daily use but not released yet: there is no download, build it from source. Mooring was called Keepline and lived in the `next/` directory of [majiayu000/keepline](https://github.com/majiayu000/keepline), which keeps the previous generation (Bun CLI, web dashboard, Tauri menu bar).

## Build from source

You need macOS, Xcode, [Zig](https://ziglang.org/download/) 0.16.x and [rustup](https://rustup.rs) (the Rust version is pinned in `rust-toolchain.toml`).

```sh
git clone https://github.com/majiayu000/mooring.git
cd mooring
./bootstrap.sh            # fetches the pinned GPUI and libghostty sources
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer ./run.sh
```

The first build also downloads the Ghostty source and takes a while. Run `claude` or `codex` in a Mooring terminal and the session shows up in the sidebar.

## What it does

### 01 · See which agent needs you

Each session has one state, shown the same way in the sidebar, tabs, panes and the board: **waiting for you** (a question or an approval), **running**, **ready for review** (a turn ended), **done**. A turn ending is not the work being done: only you mark a session complete. The board (鸟瞰) lays every session out in columns, and the status bar shows your Codex, Claude and Grok quota.

### 02 · Close it, they keep working

Terminals live in a background host (`mooring serve`), not in the window. Quit, update or crash the app and every terminal is still there when it opens again. If the host itself goes (a reboot), sessions that were cut off show **resume**: one click reopens the same conversation with `claude --resume` or `codex resume`, with the flags it was started with.

### 03 · Any agent CLI

Claude Code, Codex, Grok Build, Cursor Agent, Qoder CLI, Hermes, Factory Droid, Pi, oh-my-pi and DeepSeek Harness are built in. Mooring reads the session records and events each tool provides; see the [integration boundaries](docs/decisions/2026-11-07-more-agent-inputs.md) for supported formats, child relationships and state signals. Others are added with a JSON plugin (OpenCode ships as an example). Any program can report its own state with one `printf`, using the OSC 7501 status protocol:

```sh
printf '\033]7501;state=working:app=deploy:progress=30\033\\'
```

### 04 · Agents drive Mooring, and each other

Inside a Mooring terminal, an agent can read and drive the others with the `mooring` command. `mooring run` starts another agent with a task; each turn it ends comes back into the caller's input as a `<mooring from=…>` report, so nobody polls.

```sh
mooring list
mooring run codex "review the diff on this branch and list real bugs only"
mooring --skill          # the full guide, written for agents
```

### 05 · Change it with plugins, or ask your agent to

Plugins are plain files in `~/.config/mooring/plugins/`: commands for the pane menu and ⌘P, sidebar layouts, facts on each pane, hooks on state changes (notifications, webhooks), verification rules, the completion animation, new agent CLIs. Ask the agent in your terminal, for example "add a menu item that runs npm test beside this pane". It reads `mooring --skill plugins`, writes the plugin and checks it:

```sh
mooring plugins          # which plugins take effect, and why not
```

## FAQ

**An agent does not show up in the sidebar.**
1. It has to run inside a Mooring terminal; sessions started elsewhere appear on the board only.
2. If it runs behind a wrapper (a sandbox, a VM), start it with `MOORING_AGENT=claude` (or `codex`, …) so Mooring knows what it is.
3. `mooring explain <terminal>` shows how Mooring read that screen; `mooring hooks status` shows whether the agent's hooks are installed and current.

**I used Keepline. What changes?**
Nothing to redo. On first start Mooring moves `~/.config/keepline` to `~/.config/mooring` (a link stays at the old place), keeps using the old runtime directory so the old host hands its terminals over, and still answers to `keepline`, `keeplined` and `KEEPLINE_*` variables. Run `mooring hooks install` once to update the agents' hook entries.

**What happens when I quit, update or restart?**
Quitting or updating the app leaves every terminal running. Restarting the machine ends them; each interrupted session then offers to resume its conversation in the same directory.

**Where is my data, and does anything leave my machine?**
Terminals and the session index live in `~/.local/share/mooring-workbench`, settings and plugins in `~/.config/mooring`. Mooring reads the agents' own transcripts and never uploads them. The phone relay is opt-in and end-to-end encrypted.

**Does Mooring change my agent settings?**
Only when you install hooks (`mooring hooks install`), which adds Mooring's entries to the agent's settings file; `mooring hooks uninstall` removes them. Without hooks, state comes from transcripts and the screen.

## Repository

- `crates/mooring-app`: the GPUI window. `crates/mooring-host`: the background host and the `mooring` command. `crates/mooring-core`: the shared model and protocol. `crates/mooring-remote`, `mooring-relay`: the phone link and its relay.
- `plugins/`: example plugins. `mobile/`: the phone chat app (Expo). `eval/`: terminal and protocol checks.

Issues: [GitHub Issues](https://github.com/majiayu000/mooring/issues).

<div align="center">

MIT License

</div>

---

## The Agent Infra Stack

This project is one layer of an open-source stack for running coding agents (Claude Code, Codex) as serious infrastructure. Every piece works standalone; together they close the loop:

`mooring` is the **Keep** layer: see which agent needs you, keep sessions alive, and return to the right conversation.

| Layer | Project | What it does |
|---|---|---|
| Extend | [claude-skill-registry](https://github.com/majiayu000/claude-skill-registry) | Discover and search community Claude Code skills |
| Extend | [spellbook](https://github.com/majiayu000/spellbook) | Cross-runtime skills for Claude Code, Codex, and multi-agent workflows |
| Trust | [argus](https://github.com/majiayu000/argus) | Static install-time scanner for supply-chain attacks (npm / PyPI / crates.io) |
| Trust | [vibeguard](https://github.com/majiayu000/vibeguard) | Rules, hooks, and guards against hallucinated or unverified agent changes |
| Remember | [remem](https://github.com/majiayu000/remem) | Local-first persistent memory for Claude Code and Codex sessions |
| Orchestrate | [harness](https://github.com/majiayu000/harness) | Rust agent orchestration platform — rules, skills, GC, observability |
| Route | [litellm-rs](https://github.com/majiayu000/litellm-rs) | High-performance Rust AI gateway — 100+ LLM APIs via OpenAI format |
| Keep | [mooring](https://github.com/majiayu000/mooring) **◀ you are here** | Session command center — monitor, recover, never lose agent work |
