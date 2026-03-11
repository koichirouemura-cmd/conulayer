# Conulayer — Setup Manual

The core experience of this project is:

> **Tell Claude Code what you want → it writes WASM → deploys to the unikernel instantly, no reboot.**

This manual explains how to set up that connection.

---

## You don't need to be an engineer

This project was built by a non-engineer, with Claude Code.

The setup involves things like creating a VM, enabling KVM, and configuring network settings — but you don't need to know what any of that means. **Claude Code can walk you through every step.**

**First, get Claude Code on your Mac:** https://claude.ai/download — install the desktop app, then run `claude` in your terminal. That's the AI you'll work with throughout this setup.

Once you have Claude Code, just start a conversation:

> "I want to install Conulayer. I have Proxmox running. Can you guide me through setting up a VM and running the install script?"

Claude Code knows what KVM passthrough is, how to configure Proxmox VM settings, and how to troubleshoot SSH connections. The creator of this project set everything up this way.

Once the MCP server is running (after install.sh completes), Claude Code connects directly to your unikernel and you can manage everything through conversation — no terminal commands needed.

---

## How It Works

```
Your Mac (Claude Code)
    ↓ same local network (LAN)
Alpine Linux — IP: 192.168.x.x
    ↓ MCP protocol (:8090)
MCP Server (running on Alpine)
    ↓ HTTP POST /update
unikernel (KVM guest inside Alpine)
    ↓ hot-swap WASM module
Your app is live (:8080)
```

The MCP server is the bridge between Claude Code and the unikernel. Once set up, you never touch the unikernel directly.

**Network requirement:** Your Mac and the Alpine Linux machine must be on the same local network (LAN). Claude Code on your Mac connects to Alpine's IP address directly — for the browser (`http://[ALPINE_IP]:8080/`) and for MCP (`http://[ALPINE_IP]:8090/sse`).

**Tip: fix Alpine's IP address.** By default Alpine gets a DHCP address that can change after reboot. If the IP changes, your Claude Code MCP config breaks. It's worth setting a static IP — ask Claude Code: "How do I set a static IP on Alpine Linux?"

---

## System Requirements

| Resource | Minimum | Recommended |
|---|---|---|
| CPU | 1 core (KVM-capable x86_64) | 2 cores |
| RAM | 512MB | 1GB |
| Disk | 4GB | 8GB |

**KVM is required.** The unikernel runs as a KVM guest — hardware virtualization must be available.

**Bare metal (recommended):** Any x86_64 PC or server purchased after ~2010 supports VT-x (Intel) or AMD-V (AMD). KVM works automatically — no configuration needed. Just install Alpine Linux and run the install script.

**Proxmox VM:** KVM is not passed through to nested VMs by default. You must explicitly enable it in the VM settings (see [Step 1](#step-1-create-an-alpine-linux-vm-on-proxmox)). The unikernel will still run without KVM (software emulation), but will be significantly slower.

Check KVM availability:
```sh
grep -c vmx /proc/cpuinfo   # Intel: any number > 0 = KVM available
grep -c svm /proc/cpuinfo   # AMD:   any number > 0 = KVM available
```

---

## Option A: Quick Install (Recommended)

If you already have Alpine Linux running (on Proxmox or bare metal), run:

```sh
curl -fsSL https://raw.githubusercontent.com/koichirouemura-cmd/conulayer/main/install.sh | sh
```

This automatically:
1. Installs required packages (QEMU, Python, socat)
2. Downloads the unikernel ISO from GitHub Releases
3. Sets up all services (vsock, MCP server, unikernel)
4. Starts everything and prints the connection info

When complete, you'll see:
```
============================================================
  Conulayer installation complete!

  Earthquake Monitor: http://192.168.x.x:8080/
  BBS:                http://192.168.x.x:8080/bbs

  Claude Code MCP config (~/.claude.json):
  {
    "mcpServers": {
      "unikernel": {
        "type": "sse",
        "url": "http://192.168.x.x:8090/sse"
      }
    }
  }
============================================================
```

Then jump to [Step 5: Connect Claude Code](#step-5-connect-claude-code).

---

## Option B: Manual Setup

### Step 1: Create an Alpine Linux VM on Proxmox

> **Not sure about any of this?** Ask Claude Code:
> "Help me create a Proxmox VM for Conulayer. I need KVM enabled."
> It will guide you through each setting.

1. Download Alpine Linux ISO from https://alpinelinux.org/downloads/
   - Choose **x86_64** → **Standard**

2. In Proxmox web UI, click **Create VM**:

   | Setting | Value |
   |---|---|
   | OS | Alpine Linux ISO (uploaded to Proxmox) |
   | CPU | 2 cores |
   | RAM | 1024 MB |
   | Disk | 8 GB |
   | Network | VirtIO, bridge to your LAN |
   | **Enable KVM** | ✅ must be checked |

3. Start the VM and run Alpine setup:

```sh
setup-alpine
```

Follow the prompts:
- Keyboard: `us` (or your layout)
- Hostname: `conulayer`
- Network: `eth0` → DHCP
- Root password: set something
- Disk: `sda` → `sys` (full install)
- Reboot when done

4. After reboot, note the IP address:

```sh
ip addr show eth0
```

---

### Step 2: Install Conulayer on Alpine

SSH into Alpine from your Mac:

```sh
ssh root@[ALPINE_IP]
```

Run the install script:

```sh
apk add curl
curl -fsSL https://raw.githubusercontent.com/koichirouemura-cmd/conulayer/main/install.sh | sh
```

---

### Step 3: Verify Services

```sh
# Check all services are running
rc-status

# Check unikernel booted
grep '\[HTTP READY\]' /var/log/unikernel.log

# Check HTTP response
curl http://localhost:8080/
```

---

### Step 4: Test from your Mac

```sh
curl http://[ALPINE_IP]:8080/
# Should return the earthquake monitor HTML
```

---

## Step 5: Connect Claude Code

Add the MCP server to Claude Code's configuration.

Edit `~/.claude.json` on your Mac and add under your project's `mcpServers`:

```json
{
  "projects": {
    "/path/to/your/project": {
      "mcpServers": {
        "unikernel": {
          "type": "sse",
          "url": "http://[ALPINE_IP]:8090/sse"
        }
      }
    }
  }
}
```

Restart Claude Code. You should now see `mcp__unikernel__*` tools available.

---

## Using It

Once connected, you can talk to Claude Code naturally:

**Check status:**
> "Is the unikernel running?"

**Read logs:**
> "Show me the last 20 lines of the unikernel log"

**Deploy a new app:**
> "Write a WASM module that returns the current time as JSON and deploy it to /time"

Claude writes the WAT source, calls `deploy_wasm(wat_source, route="/time")`, and the module is live instantly — no reboot.

---

## Available MCP Tools

| Tool | Description |
|---|---|
| `deploy_wasm(wat_source, route)` | Compile WAT and hot-deploy to unikernel |
| `get_logs(last_n_lines)` | Read unikernel serial log |
| `get_status()` | Check HTTP connectivity and running state |

---

## Serial Log Format

The unikernel writes structured logs. Key markers:

| Marker | Meaning |
|---|---|
| `[BOOT OK]` | Kernel started |
| `[NET OK]` | VirtIO-net initialized |
| `[HTTP READY]` | HTTP server listening on port 80 |
| `[WASM] loaded` | WASM module loaded |
| `[SWAP]` | WASM hot-swap completed |
| `[ERROR] ...` | Something went wrong |

---

## Troubleshooting

| Symptom | Check |
|---|---|
| MCP tools not appearing in Claude Code | Verify `~/.claude.json` config and restart Claude Code |
| `get_status()` returns connection error | Check unikernel log for `[HTTP READY]`; check Alpine port 8080 |
| `deploy_wasm` fails with wat2wasm error | Check WAT syntax |
| Unikernel not booting | `tail -f /var/log/unikernel.log` on Alpine |
| KVM not available | Check CPU virtualization is enabled in BIOS/Proxmox VM settings |

### Stuck on something?

Tell Claude Code what's happening. For example:

> "The install script finished but I can't access http://[IP]:8080/"

> "I see `[WARN] KVM not detected` in the output — what does that mean?"

> "MCP tools aren't showing up in Claude Code after I edited ~/.claude.json"

Claude Code can read your logs, diagnose issues, and fix them — the same way this project was built.
