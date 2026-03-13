# Architecture Considerations

This document records design discussions and architectural thinking that emerged during development.
Preserving the reasoning behind decisions helps future contributors — human or AI — understand the intent.

---

## 1. What kinds of applications benefit most from unikernel fast startup?

One of the key properties of a unikernel is fast boot time (~100ms).
However, for applications that run continuously once started, the advantage over Linux is minimal.
The real benefit appears when **startup happens frequently** or **startup latency is user-facing**.

### High impact categories

**Serverless / FaaS (best fit)**

"Boot on request, disappear when done" — the serverless model.
- Linux container cold start: 200ms–1s
- Unikernel: under 100ms

Latency translates directly into user experience here.
Replacing the Linux container layer in AWS Lambda-equivalent infrastructure with unikernels would be a fundamental shift.

**IoT / Edge devices**

Wake on sensor event, process, sleep.
- Battery life changes directly
- Runs on devices with only a few MB of RAM
- Works on hardware that cannot run Linux

**Security-sensitive workloads**

Unikernels contain only what is needed — nothing more.
- No shell → shell injection is structurally impossible
- Drastically reduced kernel attack surface
- Natural fit for financial and medical processing nodes

**Large-scale parallel microservice startup**

When 1,000 services need to start simultaneously, 100ms vs 3s compounds into an order-of-magnitude difference.

### Lower impact categories

- Long-running services (database servers, web servers) — they only boot once
- Applications under active development — lack of debugging tools is painful
- Applications requiring complex state — multiple processes and filesystems are assumed

### Note on the earthquake monitor in this project

The earthquake monitor is a continuously-running service, so it gains little from fast boot.
The more meaningful advantages of this project are **memory efficiency** (105MB → 2MB)
and **the structure that allows AI to autonomously deploy**.

The scenario where unikernels truly shine is:
an AI agent spins up a minimal VM containing only the functions needed for a task,
processes it in 100ms, and tears it down.
That model would fundamentally redefine what serverless means today.

---

## 2. Does the unikernel advantage hold when Alpine Linux is the host?

### Current architecture

```
Hypervisor
  └── Alpine Linux VM  (always running)
        └── unikernel launched as KVM guest
              └── Rust + WASM application  (~2–5 MB, ~100ms boot)
```

Alpine acts as a permanent base layer.

### What is actually being compared

The real comparison is **Alpine + unikernel** vs **Alpine + Python**.

| | Alpine + unikernel | Alpine + Python |
|---|---|---|
| Alpine overhead | identical | identical |
| Application layer | +2–5 MB, ~100ms | +100 MB, ~3s |

Since Alpine's fixed cost cancels out on both sides,
**the application-layer difference remains valid.**

### An honest assessment

Running unikernel without Alpine is the original vision.
Alpine is currently required for:
- File and module management (storing WASM modules)
- Secret management (via vsock)
- Unikernel lifecycle management (start, stop, update)

Delegating these to Alpine is a pragmatic compromise for this stage of the project.
If unikernels could coordinate with each other over a network to handle management tasks,
Alpine would no longer be necessary.

**The accurate statement today:**
- "The advantage holds at the application layer."
- "The full potential of unikernel architecture is only partially realized."

### Alpine as the equivalent of firmware

One way to think about it: Alpine is infrastructure that is never touched,
analogous to BIOS or firmware.
It does not undermine the AI-native philosophy because
the application layer — the unikernel — is autonomously generated and deployed by AI.
Alpine provides the management substrate but plays no role in application execution or updates.

---

## 3. Comparison environment

Two VMs running the same earthquake monitor with identical UI and data sources,
differing only in the implementation layer.

| VM | Implementation | Purpose |
|---|---|---|
| unikernel host | Alpine Linux + unikernel (Rust + WASM) | Production — AI-native architecture |
| linux host | Alpine Linux + Python stdlib | Baseline — pure Linux architecture |

This setup was intentionally constructed to make the differences measurable and discussable with concrete numbers.
