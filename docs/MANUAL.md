# Conulayer — Setup Manual

The core experience of this project is:

> **Tell Claude Code what you want → it writes WASM → deploys to the unikernel instantly, no reboot.**

This manual explains how to set up that connection.

---

## How It Works

```
You (natural language)
    ↓
Claude Code
    ↓ MCP protocol
MCP Server (running on Alpine Linux)
    ↓ HTTP POST /update
unikernel (KVM guest inside Alpine)
    ↓ hot-swap WASM module
Your app is live
```

The MCP server is the bridge between Claude Code and the unikernel. Once set up, you never touch the unikernel directly.

---

## System Requirements

| Resource | Minimum | Recommended |
|---|---|---|
| CPU | 1 core (KVM-capable x86_64) | 2 cores |
| RAM | 512MB | 1GB |
| Disk | 4GB | 8GB |

**KVM is required.** Check with:
```sh
grep -c vmx /proc/cpuinfo   # Intel
grep -c svm /proc/cpuinfo   # AMD
# Any number > 0 means KVM is available
```

---

## Option A: Quick Install (Recommended)

If you already have Alpine Linux running (on Proxmox or bare metal), run:

```sh
curl -fsSL https://raw.githubusercontent.com/koichirouemura-cmd/conulayer/main/install.sh | sh
```

This automatically:
1. Installs required packages (QEMU, Python, wabt, socat)
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
| `deploy_wasm` fails with wat2wasm error | Check WAT syntax; ensure `wabt` is installed on Alpine |
| Unikernel not booting | `tail -f /var/log/unikernel.log` on Alpine |
| KVM not available | Check CPU virtualization is enabled in BIOS/Proxmox VM settings |
