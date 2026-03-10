#!/usr/bin/env python3
import os
os.environ["FASTMCP_HOST"] = "0.0.0.0"
os.environ["FASTMCP_PORT"] = "8090"

import subprocess
import tempfile
import urllib.request
import urllib.error

from mcp.server.fastmcp import FastMCP

mcp = FastMCP("unikernel-manager", host="0.0.0.0", port=8090)

UNIKERNEL_ADMIN_BASE = "http://127.0.0.1:8081/update"
LOG_FILE = "/var/log/unikernel.log"


def _post_wasm(wasm_bytes: bytes, route: str = "/") -> str:
    if route == "/":
        url = UNIKERNEL_ADMIN_BASE
    else:
        url = UNIKERNEL_ADMIN_BASE + route
    req = urllib.request.Request(
        url,
        data=wasm_bytes,
        headers={"Content-Type": "application/wasm", "Content-Length": str(len(wasm_bytes))},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            body = resp.read().decode()
            return f"OK ({len(wasm_bytes)} bytes deployed to route '{route}'): {body}"
    except urllib.error.URLError as e:
        return f"Cannot connect to unikernel: {e.reason}"


@mcp.tool()
def deploy_wasm(wat_source: str, route: str = "/") -> str:
    """Compile WAT (WebAssembly Text) source and hot-deploy to the unikernel.
    route: path prefix this WASM handles (e.g. "/", "/bbs", "/api").
    Applied instantly without reboot. Re-deploying to the same route overwrites it."""
    with tempfile.NamedTemporaryFile(suffix=".wat", mode="w", delete=False) as f:
        f.write(wat_source)
        wat_path = f.name
    wasm_path = wat_path.replace(".wat", ".wasm")
    try:
        result = subprocess.run(["wat2wasm", wat_path, "-o", wasm_path], capture_output=True, text=True, timeout=30)
        if result.returncode != 0:
            return f"wat2wasm error:\n{result.stderr}"
        with open(wasm_path, "rb") as f:
            wasm_bytes = f.read()
        return _post_wasm(wasm_bytes, route)
    except (subprocess.TimeoutExpired, FileNotFoundError) as e:
        return f"Compile error: {e}"
    finally:
        for p in [wat_path, wasm_path]:
            if os.path.exists(p): os.unlink(p)


@mcp.tool()
def get_logs(last_n_lines: int = 50) -> str:
    """Get unikernel serial log"""
    try:
        with open(LOG_FILE) as f:
            lines = f.readlines()
        return "".join(lines[-last_n_lines:]) or "(no logs)"
    except FileNotFoundError:
        return f"Log file not found: {LOG_FILE}"


@mcp.tool()
def get_status() -> str:
    """Check unikernel running status"""
    results = []
    try:
        with urllib.request.urlopen("http://127.0.0.1:8080/", timeout=3) as resp:
            body = resp.read().decode()
            results.append(f"HTTP port 8080: OK")
            results.append(f"Response: {body[:200]}")
    except Exception as e:
        results.append(f"HTTP port 8080: {e}")
    try:
        with open(LOG_FILE) as f:
            content = f.read()
        if "[HTTP READY]" in content:
            results.append("Unikernel: RUNNING")
        elif "[BOOT OK]" in content:
            results.append("Unikernel: BOOTING")
        else:
            results.append("Unikernel: UNKNOWN")
        swap_lines = [l for l in content.split("\n") if "[SWAP]" in l]
        if swap_lines:
            results.append(f"Last hot-swap: {swap_lines[-1]}")
    except FileNotFoundError:
        results.append("Log file not found")
    return "\n".join(results)


if __name__ == "__main__":
    mcp.run(transport="sse")
