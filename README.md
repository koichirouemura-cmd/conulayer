# Conulayer

> A non-engineer built an AI-native unikernel with Claude Code.

---

## What is this?

This project asks a fundamental question:

> **"If AI is the primary operator, do we still need the layers designed for humans?"**

Today's AI-powered apps run on top of Python, Linux, and shell — layers designed for humans to read and write. When AI is the primary agent, these become unnecessary overhead.

**This project eliminates that overhead entirely.**

Instead of:
```
Intent → AI Agent → Python → Linux → Hardware
```

We build:
```
Intent → AI Agent → Unikernel (only what's needed runs)
```

---

## Architecture

The system uses a two-layer design:

```
┌──────────────────────────────────────────────────┐
│  Layer B: Dynamic WASM Layer (changes freely)    │
│  · Everything Linux + Python can do              │
│  · AI develops and deploys modules on demand     │
│  · Hot-swappable via network, no reboot needed   │
├──────────────────────────────────────────────────┤
│  Layer A: Bare Metal Fixed Layer (never changes) │
│  · Boot / Memory / Networking fundamentals       │
│  · Rust no_std — no OS, no libc, no shell        │
│  · VirtIO-net + smoltcp TCP/IP stack             │
└──────────────────────────────────────────────────┘
```

### Why WASM for Layer B?

Not for UX reasons — for **energy efficiency**.

A WASM module compiled once runs on x86_64, ARM64, and any future architecture without recompilation. This maximizes reuse across a component registry: one JWT validator, deployed everywhere.

### Tech Stack

| Layer | Technology |
|---|---|
| Hypervisor | Proxmox VE (KVM/QEMU) |
| Core OS | Rust `no_std` + `linked_list_allocator` |
| Networking | VirtIO-net + smoltcp |
| Storage | VirtIO-blk + vsock |
| App Layer | WebAssembly (hand-written WAT → wasmi runtime) |
| Secrets | Injected at runtime via vsock from Alpine Linux |

---

## Live Demo

This unikernel runs two applications:

- **Earthquake Monitor** (`GET /`) — Real-time Japan seismic data from JMA, rendered on a Leaflet map
- **BBS** (`GET /bbs`) — Simple message board

Both are implemented as WASM modules (~557 bytes and ~1224 bytes respectively), loaded and executed by the unikernel at runtime.

![Earthquake Monitor Screenshot](eq_monitor.png)

---

## Linux Reference Implementation

`docker-editor/` contains a Linux + Docker version of the same text editor app, built with Python (Flask) + nginx.

This exists as a **direct comparison target**:

| | Docker version | Unikernel version |
|---|---|---|
| Runtime | Python + Flask + nginx | WASM (hand-written WAT) |
| Memory | ~135MB | ~2–5MB |
| Boot time | ~3s | ~100ms |
| OS layer | Linux | none |

The same functionality, with and without the human-oriented layers.
See [comparison_report.md](docs/comparison_report.md) for full benchmark details.

---

## Quick Start

On a fresh Alpine Linux (physical or Proxmox VM):

```sh
curl -fsSL https://raw.githubusercontent.com/koichirouemura-cmd/conulayer/main/install.sh | sh
```

This installs everything and prints the Claude Code MCP config. See [MANUAL.md](docs/MANUAL.md) for full setup instructions including Proxmox VM creation.

---

## Why Open Source?

See [COMMUNITY_NOTES.md](COMMUNITY_NOTES.md) for the full story.

**Short version:** Low-level code (WAT, `no_std` Rust, VirtIO drivers) is underrepresented in AI training data. Publishing this project — including all trial-and-error — helps future AI work more efficiently in bare-metal environments. That directly advances the energy efficiency goals this project was built for.

---

## Docs

- [PLAN.md](docs/PLAN.md) — Design philosophy, architecture, and implementation record
- [MANUAL.md](docs/MANUAL.md) — Setup and Claude Code integration
- [comparison_report.md](docs/comparison_report.md) — Benchmark comparisons vs. Linux baseline
- [COMMUNITY_NOTES.md](COMMUNITY_NOTES.md) — Notes on AI training, opt-out, and energy efficiency

**日本語ドキュメント:**
- [PLAN.md（日本語）](docs/PLAN_ja.md)
- [MANUAL.md（日本語）](MANUAL.md)

---

## 日本語補足 / Japanese Summary

このプロジェクトは、エンジニアではない人間が Claude Code（AI）と共同で構築した AI-native unikernel です。

**問い：** AIが主体であるなら、人間用に設計されたPython・Linuxという中間レイヤーはそもそも必要か？

**答え：** unikernel + WASMの2層構造で、人間用レイヤーを排除する。

設計の詳細は [docs/PLAN.md](docs/PLAN.md)、公開の背景は [COMMUNITY_NOTES.md](COMMUNITY_NOTES.md) を参照してください。

---

## License

[MIT](LICENSE)
