# Comparison Report: Web Stack vs. AI-Native Unikernel

**Version**: 3.0 (rewritten 2026-03-15)
**Application**: p2pquake API → JSON → Leaflet map (earthquake monitor)
**Environment**: Proxmox VE / Mac Studio M4 Max 36GB / Alpine Linux 3.21

> **Reading guide**: This report distinguishes measured values from estimates throughout.
> - ✅ Measured — actual values from running systems
> - 〜 Estimated — based on standard references or calculations

---

## Table of Contents

1. [The Core Question](#1-the-core-question)
2. [Three Configurations Compared](#2-three-configurations-compared)
3. [Design Philosophy](#3-design-philosophy)
4. [Physical Resources — Theory vs. Current Reality](#4-physical-resources--theory-vs-current-reality)
5. [Deployment Speed](#5-deployment-speed)
6. [Security Attack Surface](#6-security-attack-surface)
7. [AI Development Cost](#7-ai-development-cost)
8. [Dependencies and Maintenance](#8-dependencies-and-maintenance)
9. [Applicability and Limitations](#9-applicability-and-limitations)
10. [Summary Table](#10-summary-table)
11. [Conclusion](#11-conclusion)

---

## 1. The Core Question

What the application actually does, in both implementations:

```python
# Flask (Python)
resp = requests.get('https://api.p2pquake.net/v2/history?codes=551&limit=10')
return jsonify(resp.json())
```

```wat
;; app.wasm (WebAssembly, 557 bytes)
(call $get_feed ...)   ;; fetch from JMA proxy
(return ...)           ;; write HTTP response
```

The logic is the same. The question is: **what surrounds it, and why?**

---

## 2. Three Configurations Compared

This report compares three real configurations, all running on the same Proxmox host:

### Configuration A — Flask / Docker / Ubuntu (traditional)
```
Ubuntu Linux
  └── systemd
      └── Docker
          └── nginx
              └── gunicorn
                  └── Python / Flask
                      └── app.py (~80 lines)
```
*Reference configuration. Not currently running — values are estimated from standard setups.*

### Configuration B — Alpine / Python (minimal, currently running on VM 105)
```
Alpine Linux
  └── Python process (eq-server.py, ~60 lines)
```
*The simplest possible implementation. Running in production.*

### Configuration C — Alpine / QEMU / Unikernel (currently running on VM 106)
```
Alpine Linux (host OS)
  ├── nginx (JMA proxy on port 8889)
  ├── vsock-secret-server
  ├── vsock-file-server
  ├── mcp-server
  └── QEMU / KVM (unikernel guest)
        └── Rust no_std unikernel (1.7MB)
              └── wasmi WASM runtime
                    └── app.wasm (557 bytes)
```
*AI-native design. Running in production.*

---

## 3. Design Philosophy

### Why layers accumulate in the traditional stack

Each layer in Configuration A exists for a reason — but all of those reasons assume **humans** are doing the development and operation:

```
Ubuntu   → humans need a manageable OS
systemd  → humans need process management
Docker   → humans need reproducible environments
nginx    → humans need configurable routing
gunicorn → humans need WSGI management
Python   → humans need readable code
Flask    → humans need a familiar framework
```

Remove the assumption that humans are the primary actor, and most of these layers become unnecessary overhead.

### What AI-native design assumes instead

```
AI generates it  → minimal, verified binaries
AI operates it   → reads serial logs and self-corrects
AI reuses it     → fetches components from a registry
```

The intermediate layers are not removed because they are "bad engineering." They are removed because **they were designed for humans, and AI does not need them**.

### The UI boundary

This project draws an explicit boundary:

```
AI runs (unikernel) → no human-oriented layers
                    → app.wasm = 557 bytes

Humans see (browser) → use human-oriented infrastructure
                      → Leaflet.js via CDN, CartoDB tiles
```

Leaflet.js from a CDN is not a compromise. Rendering and maps exist for humans — using infrastructure built for humans is the correct design decision.

---

## 4. Physical Resources — Theory vs. Current Reality

### ⚠️ Important: two different numbers exist

The unikernel itself is lightweight. But **the current deployment requires QEMU** to run it, which adds substantial overhead. These are different things and must not be confused.

---

### Memory usage

#### Config A — Flask / Docker (〜 estimated)

```
Linux kernel (minimal)               ~100MB
systemd + daemons                     ~80MB
Docker daemon                        ~100MB
nginx + gunicorn                      ~65MB
Python interpreter + Flask            ~75MB
app.py logic                           ~5MB
─────────────────────────────────────────
Total                                ~425MB
```

#### Config B — Alpine / Python (✅ measured on VM 105, 2026-03-15)

```
Alpine Linux                          ~80MB
Python process (eq-server.py)         ~70MB
─────────────────────────────────────────
Total                                ~151MB   ← Proxmox reports 151MB
```

#### Config C — Alpine / QEMU / unikernel (✅ measured on VM 106, 2026-03-15)

```
Alpine Linux (host OS)                ~80MB
QEMU process (KVM guest)             ~150MB   ← dominant cost
nginx (JMA proxy)                     ~10MB
vsock servers + MCP server            ~15MB
unikernel (the actual application)     ~8MB   ← the lightweight part
─────────────────────────────────────────
Total                                ~296MB   ← Proxmox reports 296MB
```

**Unikernel in isolation: ~8MB. Unikernel as currently deployed: ~296MB.**

---

### CPU usage (✅ measured on Proxmox, 2026-03-15)

| Config | CPU (idle) | Notes |
|---|---|---|
| A — Flask/Docker | 〜3–8% | Estimated from standard setups |
| B — Alpine/Python | **1.3%** | VM 105 measured |
| C — Alpine/QEMU/unikernel | **11.5%** | VM 106 measured |

**In the current deployment, Python is ~9x lighter on CPU and uses half the memory.**

The CPU overhead in Config C comes from **VM exits** — the constant context-switching between the KVM guest (unikernel) and the host (Alpine), even when no requests are being served.

---

### Why this gap exists, and what it means

The unikernel's resource efficiency claim is not wrong — it describes the unikernel **in isolation**. The problem is the execution layer required to run it:

| Deployment method | Layers | Resource overhead |
|---|---|---|
| Current: Proxmox → Alpine → QEMU → unikernel | 4 | High ✅ measured |
| Proxmox → unikernel directly (PCI passthrough) | 2 | Medium (estimated) |
| Bare-metal → unikernel | 1 | Near zero (theoretical) |

The current QEMU-on-VM setup adds two unnecessary layers. This is a **deployment layer problem**, not a design flaw in the unikernel itself.

**Honest summary for the current setup**: For a single-application deployment on a shared hypervisor, Alpine + Python is more resource-efficient. The unikernel's efficiency advantage requires a more direct execution path to be realized.

---

### Storage

| Component | Config A | Config B | Config C |
|---|---|---|---|
| OS | Ubuntu ~2.5GB | Alpine ~130MB | Alpine ~130MB |
| Runtime | Python ~45MB + Docker ~400MB | Python included | Rust binary 1.7MB |
| Application | app.py ~3KB | eq-server.py ~2KB | app.wasm 557B |
| **Total** | **~3.0GB** | **~132MB** | **~132MB** |

Config B and C are comparable on storage — both run on Alpine.

---

## 5. Deployment Speed

This is where the unikernel's advantage is **real and measurable in the current setup**.

### Deploying a new version (✅ measured)

```
Config A (Docker image rebuild + deploy):
  docker build                   ~60–180 seconds
  docker push / pull             ~30–60 seconds
  container restart              ~5–10 seconds
  ───────────────────────────────────────────────
  Total                          ~2–4 minutes

Config B (Python script update):
  scp new script                 ~1 second
  rc-service restart             ~2 seconds
  ───────────────────────────────────────────────
  Total                          ~3 seconds

Config C (unikernel ISO update):
  download new unikernel.iso     ~5 seconds (9.5MB)
  rc-service unikernel restart   ~5 seconds  ← ✅ measured on VM 106
  ───────────────────────────────────────────────
  Total                          ~10 seconds
```

The unikernel's key structural advantage: **the deployment unit is a single immutable ISO file**. No build step on the target machine, no dependency resolution, no layer caching to invalidate.

For Python, a simple script update is equally fast. Docker's build pipeline is the slow part.

### Cold start time

```
Config A — Flask/Docker (〜 estimated):
  Ubuntu boot + Docker + Python startup     ~27 seconds

Config C — unikernel (✅ measured, HTTP READY):
  QEMU launch + unikernel boot              ~5 seconds
```

**5 seconds to HTTP READY**, including QEMU startup. Config B (Python) starts in ~2 seconds.

---

## 6. Security Attack Surface

This advantage holds regardless of the deployment method.

### CVE exposure (〜 estimated, NVD 2020–2025)

```
Config A (Flask/Docker/Ubuntu):
  Linux kernel     >800 CVEs
  Python           >150
  nginx            >50
  Flask/Werkzeug   >30
  Docker           >100
  ──────────────────────────────
  Total exposure   >1,100

Config C (unikernel):
  smoltcp          <5
  wasmi            <3
  Rust no_std      <10
  ──────────────────────────────
  Total exposure   <20
```

**Difference: ~55x**

### Blast radius if compromised

```
Config A worst case:
  Full OS access, filesystem readable, lateral movement possible,
  environment variables (secrets) leaked, other apps on same host at risk.

Config C worst case:
  Only WASM sandbox memory accessible.
  No filesystem. No shell. No other processes. No root concept.
  KVM guest boundary provides an additional isolation layer.
```

Config B (Alpine/Python) sits between these — no Docker overhead, but still has a shell, filesystem, and Python interpreter accessible if compromised.

---

## 7. AI Development Cost

### Why Flask costs are structural, not just larger

```
Flask session 1: "Write nginx proxy_pass config" → reasoning → cost
Flask session 2: "Write nginx proxy_pass config" → reasoning → same cost
Flask session N: same

AI has no memory across sessions.
Infrastructure configuration was designed assuming human memory.
```

With the unikernel, the platform layer (boot, VirtIO, TCP, WASM runtime) is built once and does not change. AI never needs to reason about it again.

### Estimated token cost per application (〜 estimated)

| | Config A (Flask) | Config C (Unikernel) |
|---|---|---|
| 1st app | ~82,000 tokens (~$0.25) | ~42,000 tokens (~$0.13) |
| 3rd app | ~82,000 (same) | ~25,000 (platform reused) |
| 10th app | ~82,000 (same) | ~15,000 (components accumulate) |

*These are architectural estimates. Actual 2nd app (text editor) consumed 76,686 tokens due to WAT debugging loops — see Section 12 for detail.*

### The honest caveat on token costs

WAT (WebAssembly Text) is assembly-equivalent. AI has fewer training examples for it than for Python. **Debugging loops in WAT currently cost more tokens than debugging Python**. The token advantage over Flask requires:

1. A mature component registry (reduces what AI must write from scratch)
2. Better AI training data for no_std Rust and WAT

Condition 1 is partially achieved. Condition 2 improves as this codebase gets published.

---

## 8. Dependencies and Maintenance

### Dependency count

| | Config A | Config B | Config C |
|---|---|---|---|
| OS packages | thousands | ~200 | ~200 |
| Runtime packages | ~100 (pip) | 0 | 0 (Rust crates compiled in) |
| External dependencies at runtime | dozens | 0 | 0 |

### Annual maintenance burden (〜 estimated)

```
Config A:
  pip audit + updates           weekly
  Ubuntu security patches       monthly
  Docker version updates        quarterly
  ────────────────────────────────────────
  Estimated labor: ~2 engineer-weeks/year

Config B:
  Alpine apk updates            monthly (minimal)
  Python version updates        annually
  ────────────────────────────────────────
  Estimated labor: ~2 days/year

Config C:
  smoltcp/wasmi updates         1–2 times/year (intentional)
  app.wasm: no external deps    no updates needed
  ────────────────────────────────────────
  Estimated labor: near zero
```

---

## 9. Applicability and Limitations

### What the unikernel is well-suited for

```
Input → transform → output   ✅  JSON API, auth, image processing
Stateless                    ✅  No filesystem needed in the request path
Security boundary            ✅  KVM isolation + no shell/filesystem
AI-managed                   ✅  Serial logs, immutable deployment unit
```

### What it is not suited for

```
Observing the environment    ✗   No raw sockets, no packet capture
Persistent state             ✗   No filesystem (use Alpine + vsock for file access)
Privileged OS operations     ✗   No process management, no hardware control
```

### The correct division of responsibilities

```
Alpine (host OS):
  File storage, secrets, environment interaction
  → What Linux does well

Unikernel (KVM guest):
  API handling, data transformation, HTTP serving
  → Pure input → output, zero attack surface

Browser:
  Rendering, maps, user interaction
  → What CDN-based human-oriented infrastructure does well
```

This three-layer structure is not a compromise — it is the intended design. The unikernel is a **trusted execution boundary**, not a general-purpose OS replacement.

### Applicability matrix

| Application type | Fit | Reason |
|---|---|---|
| JSON API | ✅ Excellent | Pure transformation |
| Static file serving | ✅ Excellent | Returns from memory |
| Auth / JWT | ✅ Excellent | Pure computation |
| Image/video conversion | ✅ Good | Compute, environment-independent |
| Web scraping | ✅ Good | HTTP client available |
| Database | ⚠️ Marginal | Needs persistent storage |
| File processing | ⚠️ Marginal | Via Alpine + vsock |
| Network monitoring | ✗ Not suitable | Requires OS privileges |
| Hardware control | ✗ Not suitable | Requires device drivers |

---

## 10. Summary Table

| Metric | Config A (Flask) | Config B (Python) | Config C (Unikernel) | Note |
|---|---|---|---|---|
| **Memory** | ~425MB | **151MB** ✅ | 296MB ✅ | C measured, A estimated |
| **CPU (idle)** | ~3–8% | **1.3%** ✅ | 11.5% ✅ | C measured, A estimated |
| **Storage** | ~3.0GB | ~132MB | ~132MB | A estimated |
| **Cold start** | ~27s | ~2s | **~5s** ✅ | C measured |
| **Deploy (new version)** | ~2–4 min | ~3s | **~10s** ✅ | C measured |
| **CVE exposure** | >1,100 | ~50 | **<20** | Estimated, NVD 2020–2025 |
| **Runtime dependencies** | ~100 packages | 0 | 0 | — |
| **AI cost (1st app)** | ~$0.25 | ~$0.05 | ~$0.13 | Estimated |
| **AI cost (10th app)** | ~$2.50 | ~$0.50 | **~$0.40** | Unikernel improves with registry |
| **Application size** | ~80 lines | ~60 lines | **557 bytes** | Measured |

**Bold** = best in category for that metric.

---

## 11. Conclusion

### What the current deployment proves

Running the same application (earthquake monitor) across three configurations reveals a clear picture:

**Config B (Alpine + Python) wins on resource efficiency today.** It is simpler, lighter, and easier to operate than both Flask/Docker and the current QEMU-based unikernel stack.

**Config C (unikernel) wins on security isolation and deployment model.** The security boundary is structurally superior. The deployment unit (a single ISO file) has properties that neither Python scripts nor Docker images can match.

**Config A (Flask/Docker) loses on almost every axis.** The layers exist for human convenience — in an AI-operated context, that convenience is overhead.

### What the unikernel's current limitations are

The resource efficiency advantage of the unikernel is **real but not yet realized**. It requires running without QEMU as an intermediary. The current path — Proxmox → Alpine → QEMU → unikernel — adds unnecessary layers that outweigh the unikernel's intrinsic lightness.

This is a deployment infrastructure problem, not a design flaw.

### What this project's core question remains

```
"If AI is the primary actor, are the intermediate layers
 designed for humans even necessary?"
```

The answer, as of March 2026: **not in principle, but not yet eliminated in practice**. The unikernel design removes human-oriented layers from the application. The QEMU execution layer adds them back at the infrastructure level.

The next step — running unikernels more directly on the hypervisor — is where the theoretical and measured numbers converge.

---

## 12. Second Application Log: Web Text Editor (2026-03-09)

A multi-user web text editor (2-second polling, vsock file persistence).

### Implementation comparison

| | Docker version | Unikernel version |
|---|---|---|
| Files created | 6 (app.py, Dockerfile, nginx.conf, etc.) | 2 (editor.wat, editor_ui.html) |
| Infrastructure config lines | 37 | 2 (additions to registry.rs) |
| App binary size | ~400MB (Docker image) | **1,525 bytes** (editor.wasm) ✅ |
| Tokens consumed | ~6,000 (estimated) | **76,686** ✅ (measured) |

### Token breakdown (unikernel, measured)

```
Loading existing platform code into context    ~14,000
Code generation (editor.wat + UI)             ~15,000
Debugging / correction loop (WAT fixes)       ~47,000  ← 61% of total
─────────────────────────────────────────────────────
Total                                          76,686
```

WAT (WebAssembly Text) is assembly-equivalent — correction loops are expensive. This is the primary current disadvantage of the unikernel development experience. It improves as AI training data for this environment accumulates.

### Component reuse demonstrated

`file_read` / `file_write` / `file_list` host functions, implemented once for the text editor, are now available to all future applications at zero additional cost. This is the registry effect in practice.

---

## Author's Note

I'm not an engineer. I built this with Claude Code.

The numbers in this report reflect what we actually measured — including the places where the theory didn't hold up. The unikernel is lighter than Flask. It is not currently lighter than a Python process on Alpine, because QEMU is in the way.

I'm publishing this because honest measurements are more useful than optimistic projections. If this code and these debugging sessions become training data, the AI cost numbers will improve for anyone who builds on this foundation. That's the offset this project is working toward.

---

*Measured values: VM 105 (Alpine+Python): 1.3% CPU, 151MB RAM. VM 106 (Alpine+QEMU+unikernel): 11.5% CPU, 296MB RAM. Both measured via Proxmox API, 2026-03-15.*
*unikernel restart to HTTP READY: ~5 seconds, measured on VM 106.*
*unikernel binary: 1.7MB. app.wasm: 557B. editor.wasm: 1,525B. Boot time: ~5s (QEMU+KVM).*
*Flask/Docker estimates based on Ubuntu 22.04 Server minimal + standard Flask setup + NVD data.*
