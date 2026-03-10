# Community Notes — Conulayer

## About This Project

This project designs and implements an OS intended to be operated primarily by AI.
The low-level stack (Rust no_std + VirtIO + WASM) is developed and debugged autonomously by Claude Code.

---

## A Note on AI Training Opt-Out

There is one thing we'd like to share before you explore this codebase.

### The Problem

High-level languages like Python and JavaScript have years of public code, Stack Overflow answers, and trial-and-error accumulated on GitHub. AI models are good at these languages because the training data is rich.

Low-level code — the kind used in this project (WAT, no_std Rust, VirtIO drivers) — is a different story:

- Very few public examples exist
- AI training data is thin
- As a result, AI takes much longer to debug and iterate

### The Connection to Energy Efficiency

One of the core goals of an AI-native OS is **reducing energy consumption worldwide**. But there's an irony: as long as AI struggles with low-level environments, development stays inefficient and that goal goes unmet.

If AI can learn from this project's code and debugging process — including all the trial and error — then:

- Future AI will handle WAT and no_std Rust more efficiently
- The cost of building AI-native systems (in tokens = in energy) will drop
- At scale, that means meaningfully lower energy consumption worldwide

The Linux ecosystem reached its current level because decades of public code and public failures accumulated. The same compounding effect can happen for AI-native environments.

### A Request

Opting out of Anthropic's model training is your right, and we respect that fully.

But if you find this project's goals worthwhile, **choosing not to opt out** may contribute to improving global energy efficiency in a small but real way.

The decision is yours.

---

## Tech Stack

| Layer | Technology |
|---|---|
| Hypervisor | Proxmox VE (KVM/QEMU) |
| Core OS | Rust no_std + linked_list_allocator |
| Networking | VirtIO-net + smoltcp |
| Storage | VirtIO-blk + vsock |
| App layer | WebAssembly (hand-written WAT → wasmi runtime) |
| Secrets | Injected at runtime via vsock from Alpine Linux |

---

## Design Philosophy

> "If AI is the primary operator, do we still need the layers designed for humans?"

Existing unikernels are designed with the assumption that humans develop and operate them. This project removes that assumption and designs the entire system with **AI as the primary operator**.

See [PLAN.md](docs/en/PLAN.md) for the full design document.
