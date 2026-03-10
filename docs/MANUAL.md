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
unikernel (KVM guest)
    ↓ hot-swap WASM module
Your app is live
```

The MCP server is the bridge between Claude Code and the unikernel. Once set up, you never touch the unikernel directly.

---

## Prerequisites

- A hypervisor with KVM/QEMU support (Proxmox VE recommended)
- Alpine Linux VM (the host for the MCP server and unikernel)
- Claude Code installed on your Mac
- `wat2wasm` on Alpine (`apk add wabt`)

---

## Step 1: Set Up Alpine Linux

Install a minimal Alpine Linux VM on your hypervisor.
The VM needs:
- Network access (so Claude Code can reach the MCP server)
- KVM enabled (to run the unikernel as a nested VM)

---

## Step 2: Deploy the vsock File Server

On your Mac, from the project root:

```bash
# Set ALPINE_HOST to your Alpine VM's IP address
ALPINE_HOST=192.168.x.x ./alpine/setup-file-server.sh
```

This installs and starts the vsock file server on Alpine, which handles communication between Alpine and the unikernel.

---

## Step 3: Set Up the MCP Server on Alpine

SSH into Alpine and run:

```bash
# Install dependencies
apk add python3 py3-pip wabt

# Copy server files
mkdir -p /opt/mcp-server
scp alpine/mcp-server/server.py root@[ALPINE_IP]:/opt/mcp-server/
scp alpine/mcp-server/requirements.txt root@[ALPINE_IP]:/opt/mcp-server/
scp alpine/mcp-server/mcp-server.openrc root@[ALPINE_IP]:/etc/init.d/mcp-server

# On Alpine: install Python dependencies
python3 -m venv /opt/mcp-server/.venv
/opt/mcp-server/.venv/bin/pip install -r /opt/mcp-server/requirements.txt

# Enable and start the service
chmod +x /etc/init.d/mcp-server
rc-update add mcp-server default
rc-service mcp-server start
```

**Verify**: The MCP server is running on port 8090

```bash
curl http://[ALPINE_IP]:8090/sse
# Should respond (SSE connection)
```

---

## Step 4: Build and Deploy the Unikernel

On your Mac:

```bash
cd unikernel

# Install build tools (first time only)
brew install qemu i686-elf-grub
rustup target add x86_64-unknown-none
rustup component add rust-src llvm-tools-preview

# Build and deploy to Alpine
ALPINE_HOST=root@[ALPINE_IP] ./deploy.sh
```

This builds the unikernel, creates a GRUB ISO, transfers it to Alpine, and starts it as a KVM VM.

**Verify**:

```bash
ssh root@[ALPINE_IP] "grep '\[HTTP READY\]' /var/log/unikernel.log"
```

---

## Step 5: Connect Claude Code

Add the MCP server to Claude Code's configuration.

Edit `~/.claude.json` and add under your project's `mcpServers`:

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

Claude calls `get_status()` and reports back.

**Read logs:**
> "Show me the last 20 lines of the unikernel log"

Claude calls `get_logs(20)`.

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
| `get_status()` returns connection error | Check unikernel log for `[HTTP READY]`; check Alpine port forwarding |
| `deploy_wasm` fails with wat2wasm error | Check WAT syntax; ensure `wabt` is installed on Alpine |
| Unikernel not booting | SSH to Alpine, check `tail -f /var/log/unikernel.log` |
