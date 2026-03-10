# Conulayer — Design Document

> This document clearly separates **what is currently implemented** from the **future roadmap**.
> For the full original design notes (in Japanese), see `original_plan.md`.

---

## 0. Why We Built This

### Background

Today's AI-powered applications run on top of layers designed for humans.

```
Intent / Goal
    ↓
AI Agent (Claude Code, etc.)
    ↓
Python / Linux / Shell / Filesystem  ← layers designed for humans
    ↓
What we actually want to do
```

Python and Linux were built for humans to read, write, and operate.
When AI becomes the primary operator, these layers become unnecessary overhead.

### The Question

> **"If AI is the primary operator, do we still need the layers designed for humans?"**

### The Answer (this project's hypothesis)

```
Intent / Goal
    ↓
AI Agent (translates intent directly into code)
    ↓
Unikernel (only what's needed runs)
```

By eliminating human-oriented intermediate layers, we can reduce computational resources at the root level.

### System Architecture: Two Layers

A unikernel alone has one fundamental constraint:

```
Unikernel constraint:
  Changing code = full recompile + restart
  → For AI to dynamically add/replace components,
    a separate layer is needed that allows "hot-swapping without recompilation"
```

So the architecture is split into two layers:

```
┌──────────────────────────────────────────────────┐
│  Layer B: Dynamic WASM Layer (changes freely)    │
│  · Everything Linux + Python can do              │
│  · AI develops and adds modules on demand        │
│  · Hot-swappable via network, no reboot needed   │
├──────────────────────────────────────────────────┤
│  Layer A: Bare Metal Fixed Layer (never changes) │
│  · Boot / Memory / Networking fundamentals       │
│  · Design is fixed here — not touched afterward  │
│  · This is the "unikernel core"                  │
└──────────────────────────────────────────────────┘
```

#### Why WASM for Layer B?

Not for UX reasons (faster restarts). The reason is **energy efficiency**.

```
Rust binary:  separate builds for x86_64 and ARM64
              → compilation cost per architecture

WASM:         compile once, runs on any architecture
              → the same binary can be distributed to any node worldwide
              → minimizes reuse cost across a component registry
```

### Energy Impact

A typical web app needs only a few MB of actual work, but human-oriented layers inflate that massively:

```
Linux kernel resident memory:  ~50MB
Python runtime:                ~30MB
Frameworks:                    ~50MB
App logic:                     ~ 5MB  ← what we actually want
──────────────────────────────────────
Total:                        ~135MB

With a unikernel:              ~2–5MB
```

This runs across millions of instances in data centers worldwide.
**By eliminating waste in AI-operated systems, we can reduce the energy cost of AI adoption.**

### Why Engineers Find This Hard to See

Engineers have built professional value around Linux and Python.
Changes that challenge those assets are met with unconscious resistance.

**This question was only possible to ask from a perspective without those stakes.**

---

## 1. Current Implementation

### System Structure

```
┌─────────────────────────────────────────┐
│  [Human Layer]                          │
│  Alpine Linux (minimal Linux VM)        │
│  · API keys and secret management       │
│  · Personal data management             │
│  · Filesystem (human-readable/writable) │
│  · Access control and audit logging     │
│  · vsock server (always running)        │
└──────────────────┬──────────────────────┘
                   │ vsock (only permitted data/keys passed)
                   ↓
┌─────────────────────────────────────────┐
│  unikernel (KVM guest)                  │
│  Layer A: Rust no_std (boot/net/memory) │
│  Layer B: WASM runtime (wasmi)          │
│    ├── app.wasm  (earthquake monitor)   │
│    └── bbs.wasm  (bulletin board)       │
│  No keys, no data, no filesystem        │
└─────────────────────────────────────────┘
```

### Implemented Routes

```
GET /                  → eq_ui.html (static) Leaflet map
GET /api/quake         → app.wasm (557 bytes) fetches JMA JSON
GET /bbs               → bbs_ui.html (static)
GET /bbs/api/messages  → bbs.wasm (1224 bytes)
POST /bbs/post         → bbs.wasm
```

### Secret Management Design

The industry has no complete solution for plaintext secret storage.
This project's approach: **separate the human layer from the AI layer.**

- Secrets are managed in Alpine Linux (a Linux VM that humans control)
- The unikernel holds no secrets — it requests them from Alpine
- Communication is via vsock (a virtual socket accessible only to the host OS)
- "Not everything needs to be a unikernel" is a key design decision

```
Secret retrieval flow (implemented):
  Request via vsock to Alpine (preferred)
      ↓ if Alpine is not running
  Fall back to fw_cfg (direct QEMU injection)
```

---

## 2. Tech Stack

### Primary Language: Rust `no_std` + `alloc`

**Reason**: Direct LLVM IR is hard to maintain and poorly suited for the iterative AI-driven development loop. Rust is well-represented in AI training data, and its type system catches bugs early.

### Hypervisor-Independent Design

**This project is not tied to Proxmox.** Proxmox is the starting point, not a constraint.
It runs on any hypervisor that supports VirtIO.

| Layer | Dependency |
|---|---|
| App / Network / Key management | Fully shared (arch-independent) |
| VirtIO protocol | Fully shared (industry standard) |
| Boot / Paging | Arch-specific (only this part changes per arch) |

### WASM Runtime Selection

| Runtime | no_std support | Decision |
|---|---|---|
| **wasmi** | ✅ | **Selected.** no_std + alloc, proven track record |
| tinywasm | ✅ | Considered but wasmi chosen |
| wasmtime | ❌ | Requires std — not usable |

### Ecosystem Maturity

| Component | Crate / Tool | Maturity |
|---|---|---|
| CPU abstraction | `x86_64` crate | High |
| VirtIO-net | `virtio-drivers` (rcore-os) | Medium–High |
| TCP/IP | `smoltcp` | High |
| Memory management | `linked_list_allocator` | High |
| Serial output | `uart_16550` | High |
| WASM runtime | `wasmi` | High |

---

## 3. Implementation Phases

### Phase 0: Dev Environment Setup ✅ Complete

**Deliverable**: Minimal unikernel that outputs a string to serial port

**Boot design change**:
> Originally planned to use QEMU's `-kernel` option with Multiboot2.
> QEMU 10.x rejects 64-bit ELF with `-kernel`, so we switched to **GRUB ISO via `-cdrom`**.

```
Current boot sequence:
BIOS → GRUB (inside ISO) → Multiboot2 header detected
→ ELF segments loaded into memory
→ Jump to kernel entry point (32-bit protected mode)
→ Set up GDT → enable paging → switch to 64-bit long mode
→ Call Rust kernel_main()
```

---

### Phase 1: Memory Management ✅ Complete

**Deliverable**: Dynamic memory (`alloc::vec![]` etc.) working

- `linked_list_allocator` + custom BumpAllocator
- Identity mapping only (virtual address == physical address)

---

### Phase 2: Secret Retrieval ✅ Complete

**Deliverable**: Read strings from QEMU via fw_cfg (I/O ports 0x510/0x511)

**Design change**:
> Phase 2 used fw_cfg. After migrating to Alpine + vsock architecture,
> changed to vsock-first with fw_cfg fallback.
> Upper layers unchanged thanks to the SecretProvider trait abstraction.

---

### Phase 3: PCI Enumeration + VirtIO-net Init ✅ Complete (2026-03-05)

**Deliverable**: VirtIO-net device detected, initialized, and sending/receiving packets

**Key bugs fixed**:
- VirtIO QUEUE_SIZE was hardcoded — device reported a different value → read dynamically via `inw(REG_QUEUE_SIZE)`
- QUEUE_NOTIFY issued before DRIVER_OK → VirtIO spec violation → moved after DRIVER_OK

---

### Phase 4: TCP/IP Stack Integration ✅ Complete (2026-03-05)

**Deliverable**: HTTP server listening on port 80, returning responses

**Key bugs fixed**:
- smoltcp 0.11 has no `proto-arp` feature (merged into `proto-ipv4`) → removed
- TX buffer race: smoltcp sends multiple packets per poll, overwriting a single buffer → changed to QUEUE_BUFS(16) independent buffers
- **Critical**: same QUEUE_SIZE hardcoding issue as Phase 3

---

### Phase 5: Build Pipeline Automation ✅ Complete (2026-03-05)

**Deliverable**: `run_test.sh` fully automates build → ISO → QEMU → HTTP check → PASS/FAIL

```bash
# run_test.sh flow:
# 1. cargo build --target x86_64-unknown-none
# 2. i686-elf-grub-mkrescue -o boot.iso isodir/
# 3. qemu-system-x86_64 -cdrom boot.iso ... &
# 4. Wait for [HTTP READY] then run curl test
# 5. [PASS] if response contains expected string
```

---

### Phase 6: Hypervisor Deployment ✅ Complete

**Deliverable**: Running on Proxmox VE

```bash
# Convert GRUB ISO to qcow2
qemu-img convert -f raw -O qcow2 boot.iso app.qcow2

# Transfer to Proxmox and create VM
scp app.qcow2 root@proxmox:/var/lib/vz/images/200/
qm create 200 --name conulayer --memory 128 --cores 1 \
  --net0 virtio,bridge=vmbr0
```

> **Design change note**: Originally planned to pass ELF directly via `-kernel`.
> Switched to GRUB ISO due to QEMU 10.x constraints.

---

### Layer B + SPA Migration ✅ Complete (2026-03-08)

**Deliverable**: wasmi WASM runtime + earthquake monitor and BBS as SPA

**Implementation notes**:
- When WASM is mounted at `/`, the path passed has no leading slash (`api/quake` not `/api/quake`)
- JMA data is 173KB → smoltcp TX buffer needs to be 256KB
- Measure HTTP header length precisely (measured: 102 bytes)

---

## 4. Technical Details

### 4.1 Memory Layout

Identity mapping only (virtual == physical).

```
Physical memory layout (-m 128M):
0x0000_0000 - 0x0009_FFFF: BIOS / VGA reserved
0x0010_0000 - 0x07FF_FFFF: kernel + heap (~127MB)
```

- Global allocator: `linked_list_allocator`
- Heap region: 8MB (from `0x0200_0000`)

### 4.2 fw_cfg

```
I/O ports: 0x510 (selector) / 0x511 (data)
Directory: selector=0x0019 lists all entries
Arbitrary file: search by name → get selector → read
```

### 4.3 smoltcp Configuration

```toml
smoltcp = { features = ["proto-ipv4", "socket-tcp", "medium-ethernet"] }
```

- Static IP: 10.0.2.15/24 (QEMU user network)
- Default GW: 10.0.2.2
- HTTP/1.1 server on TCP port 80

### 4.4 WASM Runtime (wasmi)

- On boot, fetches WASM modules via HTTP from Alpine (10.0.2.2:8888)
- Dispatches by route prefix (`/` → app.wasm, `/bbs` → bbs.wasm)
- Host functions exposed to WASM: read HTTP request, write HTTP response

---

## 5. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| smoltcp breaking API changes | Version pinned via Cargo.lock |
| WASM module memory boundary violations | wasmi sandbox detects automatically |
| VirtIO spec violations causing undefined behavior | Verified on every run via run_test.sh |
| fw_cfg unavailable in some environments | vsock fallback implemented |

---

## 6. AI Agent Autonomous Implementation Guidelines

This project is designed with the assumption that AI autonomously implements and debugs.

### 6.1 Hard Constraints

- Never hardcode secrets — always use vsock or fw_cfg
- Serial output must follow `[PHASE] message` format (so AI can parse logs and self-correct)
- Implementation is complete only when `run_test.sh` passes

### 6.2 Phase Transition Checklist

Each phase is complete when:
1. `run_test.sh` returns `[PASS]`
2. No unexpected `[ERROR]` in serial logs
3. Prerequisites for the next phase are satisfied (memory allocated, device initialized, etc.)

### 6.3 Known Error Patterns

| Error | Cause | Fix |
|---|---|---|
| QEMU rejects `-kernel` | QEMU 10.x + 64-bit ELF | Use GRUB ISO instead |
| No packets after TCP connect | VirtIO QUEUE_SIZE mismatch | Read dynamically via `inw(REG_QUEUE_SIZE)` |
| smoltcp TX overwrite | Single TX buffer | Use QUEUE_BUFS independent buffers |
| HTTP response truncated | smoltcp TX buffer too small | Increase buffer to 256KB |

---

## 7. Prior Art

Survey as of March 2026.

| Element | Status |
|---|---|
| Unikernel technology | Solved (MirageOS 2013–) |
| Rust no_std + VirtIO + smoltcp | Solved (hermit-os, etc.) |
| AI generating unikernel code | Partial (research stage) |
| **Entire system designed with AI as primary operator** | **Unsolved — this project's original question** |

### Notable Related Projects

| Project | Overview | Difference |
|---|---|---|
| hermit-os/kernel | Rust no_std unikernel | Designed for human operation |
| Unikraft | Unikernel construction framework | Does not assume AI-driven generation |
| AIOS | AI-oriented OS paper (2024) | Implemented as a software layer on Linux |
| Solid Project | Data sovereignty | Unrelated to unikernels |

**No prior work combines AI × unikernel × data sovereignty in this way.**

---

## 8. Roadmap

> The following are not yet implemented.

### Phase 7: ARM64 Support

- Design allows adding `src/arch/aarch64/` without changing upper layers
- Native execution on Apple Silicon Mac
- x86_64 shared code remains unchanged

### MCP Server Integration (Extended)

unikernel operations (deploy/logs/status) are already implemented via MCP.
Next: extend MCP to cover Alpine secret management operations.

### DHCP Support / Distributable ISO

Currently using static IP (10.0.2.15).
DHCP support would enable "download the ISO and boot anywhere."

---

## 9. References

- [hermit-os/kernel](https://github.com/hermit-os/kernel) — Rust no_std unikernel (closest reference implementation)
- [syswonder/RuxOS](https://github.com/syswonder/ruxos) — Rust unikernel
- [Writing an OS in Rust](https://os.phil-opp.com/) — phil-opp (learning resource)
- [smoltcp](https://github.com/smoltcp-rs/smoltcp) — no_std TCP/IP stack
- [wasmi](https://github.com/wasmi-labs/wasmi) — no_std WASM runtime
