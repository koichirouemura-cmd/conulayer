#!/usr/bin/env sh
# Conulayer Install Script
# Installs and configures Conulayer on a fresh Alpine Linux system.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/koichirouemura-cmd/conulayer/main/install.sh | sh
#
# Requirements:
#   - Alpine Linux (physical hardware or VM)
#   - KVM-capable CPU (check: grep -c vmx /proc/cpuinfo or grep -c svm /proc/cpuinfo)
#   - 512MB RAM minimum, 2GB recommended
#   - 2GB disk space minimum

set -e

REPO="https://github.com/koichirouemura-cmd/conulayer"
RAW="https://raw.githubusercontent.com/koichirouemura-cmd/conulayer/main"
RELEASE_URL="${REPO}/releases/latest/download/unikernel.iso"
INSTALL_DIR="/opt/conulayer"
REGISTRY_DIR="/var/registry"
SECRETS_DIR="/secrets"

echo "==> Conulayer Installer"
echo ""

# ── 1. パッケージインストール ──────────────────────────────────────────
echo "==> Installing packages..."
# Enable community repo (needed for qemu, py3-pip)
sed -i '/^#.*\/community$/s/^#//' /etc/apk/repositories
apk update -q
apk add -q \
    python3 py3-pip \
    qemu-system-x86_64 qemu-img \
    socat \
    curl

# ── 2. KVM確認 ────────────────────────────────────────────────────────
echo "==> Checking KVM support..."
if ! grep -qE 'vmx|svm' /proc/cpuinfo; then
    echo "[WARN] KVM not detected. unikernel will run without hardware acceleration."
    ENABLE_KVM=""
else
    modprobe kvm_intel 2>/dev/null || modprobe kvm_amd 2>/dev/null || true
    modprobe vhost_vsock 2>/dev/null || true
    echo "[OK] KVM available"
    ENABLE_KVM="-enable-kvm -cpu host"
fi

# ── 3. ディレクトリ作成 ───────────────────────────────────────────────
echo "==> Setting up directories..."
mkdir -p "${INSTALL_DIR}"
mkdir -p "${REGISTRY_DIR}"
mkdir -p "${SECRETS_DIR}"

# ── 4. unikernel ISO ダウンロード ─────────────────────────────────────
echo "==> Downloading unikernel ISO..."
if [ -f "${INSTALL_DIR}/unikernel.iso" ]; then
    echo "    Already exists, skipping."
else
    curl -fsSL "${RELEASE_URL}" -o "${INSTALL_DIR}/unikernel.iso"
    echo "    Downloaded: $(du -h ${INSTALL_DIR}/unikernel.iso | cut -f1)"
fi

# ── 4b. WASM モジュール & UI ダウンロード ──────────────────────────────
echo "==> Downloading WASM modules..."
RELEASE_BASE="${REPO}/releases/latest/download"
for f in app.wasm bbs.wasm editor.wasm eq.html bbs.html editor.html; do
    curl -fsSL "${RELEASE_BASE}/${f}" -o "${REGISTRY_DIR}/${f}"
done
echo "    Done."

# ── 5. vsock secret handler ───────────────────────────────────────────
echo "==> Installing vsock secret handler..."
cat > /usr/local/bin/vsock-secret-handler << 'HANDLER'
#!/bin/sh
read KEY
KEY=$(echo "$KEY" | tr -d "\r\n")
SECRET_FILE="/secrets/$KEY"
if [ -f "$SECRET_FILE" ]; then
    cat "$SECRET_FILE"
else
    echo "ERROR: not found"
fi
HANDLER
chmod +x /usr/local/bin/vsock-secret-handler

# ── 6. vsock secret サービス ──────────────────────────────────────────
echo "==> Installing vsock-secret-server service..."
cat > /etc/init.d/vsock-secret-server << 'SVC'
#!/sbin/openrc-run
description="vsock secret server for unikernel guests"
pidfile="/run/vsock-secret-server.pid"
start() {
    ebegin "Starting vsock secret server"
    modprobe vhost_vsock 2>/dev/null || true
    start-stop-daemon --start --background \
        --make-pidfile --pidfile "$pidfile" \
        --exec /usr/bin/socat \
        -- VSOCK-LISTEN:1234,fork,reuseaddr EXEC:/usr/local/bin/vsock-secret-handler
    eend $?
}
stop() {
    ebegin "Stopping vsock secret server"
    start-stop-daemon --stop --pidfile "$pidfile"
    eend $?
}
SVC
chmod +x /etc/init.d/vsock-secret-server

# ── 7. vsock file サービス ────────────────────────────────────────────
echo "==> Installing vsock-file-server service..."
curl -fsSL "${RAW}/alpine/vsock-file-server.py" \
    -o /usr/local/bin/vsock-file-server.py
curl -fsSL "${RAW}/alpine/vsock-file-server.openrc" \
    -o /etc/init.d/vsock-file-server
chmod +x /usr/local/bin/vsock-file-server.py
chmod +x /etc/init.d/vsock-file-server

# ── 8. WASM レジストリサービス ────────────────────────────────────────
echo "==> Installing wasm-registry service..."
cat > /etc/init.d/wasm-registry << 'SVC'
#!/sbin/openrc-run
description="WASM Registry HTTP server"
command="/usr/bin/python3"
command_args="-m http.server 8888 --directory /var/registry"
command_background=true
pidfile="/run/wasm-registry.pid"
depend() { need net; }
SVC
chmod +x /etc/init.d/wasm-registry

# ── 9. unikernel サービス ─────────────────────────────────────────────
echo "==> Installing unikernel service..."
VSOCK_DEVICE=""
if modprobe vhost_vsock 2>/dev/null && [ -e /dev/vhost-vsock ]; then
    VSOCK_DEVICE="-device vhost-vsock-pci,guest-cid=3"
fi
cat > /etc/init.d/unikernel << UKSVC
#!/sbin/openrc-run
description="Conulayer unikernel KVM guest"
pidfile="/run/unikernel.pid"
logfile="/var/log/unikernel.log"
depend() {
    after localmount
}
start() {
    ebegin "Starting unikernel"
    SECRET=\$(cat /secrets/api_key 2>/dev/null || echo "no-secret")
    start-stop-daemon --start --background \
        --make-pidfile --pidfile "\$pidfile" \
        --stdout "\$logfile" --stderr "\$logfile" \
        -- qemu-system-x86_64 \
            ${ENABLE_KVM} \
            -m 256M \
            -cdrom ${INSTALL_DIR}/unikernel.iso \
            -boot d \
            -serial mon:stdio \
            -display none \
            -no-reboot \
            -netdev user,id=net0,net=10.0.2.0/24,host=10.0.2.2,hostfwd=tcp::8080-:80,hostfwd=tcp::8081-:8081 \
            -device virtio-net-pci,netdev=net0 \
            ${VSOCK_DEVICE} \
            -fw_cfg "name=opt/secret,string=\${SECRET}"
    eend \$?
}
stop() {
    ebegin "Stopping unikernel"
    start-stop-daemon --stop --pidfile "\$pidfile" --signal TERM
    eend \$?
}
UKSVC
chmod +x /etc/init.d/unikernel

# ── 10. MCP サーバー ──────────────────────────────────────────────────
echo "==> Installing MCP server..."
mkdir -p /opt/mcp-server
curl -fsSL "${RAW}/alpine/mcp-server/server.py" \
    -o /opt/mcp-server/server.py
curl -fsSL "${RAW}/alpine/mcp-server/requirements.txt" \
    -o /opt/mcp-server/requirements.txt
python3 -m venv /opt/mcp-server/.venv
/opt/mcp-server/.venv/bin/pip install -q -r /opt/mcp-server/requirements.txt

curl -fsSL "${RAW}/alpine/mcp-server/mcp-server.openrc" \
    -o /etc/init.d/mcp-server
chmod +x /etc/init.d/mcp-server

# ── 11. サービス有効化 ────────────────────────────────────────────────
echo "==> Enabling services..."
rc-update add vsock-secret-server default
rc-update add vsock-file-server default
rc-update add wasm-registry default
rc-update add unikernel default
rc-update add mcp-server default

# ── 12. サービス起動 ──────────────────────────────────────────────────
echo "==> Starting services..."
rc-service vsock-secret-server start
rc-service vsock-file-server start
rc-service wasm-registry start
rc-service unikernel start
rc-service mcp-server start

# ── 13. 起動確認 ──────────────────────────────────────────────────────
echo "==> Waiting for unikernel to boot (up to 30s)..."
i=0
while [ $i -lt 30 ]; do
    if grep -q '\[HTTP READY\]' /var/log/unikernel.log 2>/dev/null; then
        echo "[OK] unikernel is running"
        break
    fi
    sleep 1
    i=$((i + 1))
done
if [ $i -eq 30 ]; then
    echo "[WARN] Timeout waiting for unikernel. Check: tail -f /var/log/unikernel.log"
fi

# ── 完了 ──────────────────────────────────────────────────────────────
IP=$(ip route get 1.1.1.1 2>/dev/null | grep -o 'src [0-9.]*' | awk '{print $2}')
echo ""
echo "============================================================"
echo "  Conulayer installation complete!"
echo ""
echo "  Earthquake Monitor: http://${IP}:8080/"
echo "  BBS:                http://${IP}:8080/bbs"
echo ""
echo "  Claude Code MCP config (~/.claude.json):"
echo "  {"
echo "    \"mcpServers\": {"
echo "      \"unikernel\": {"
echo "        \"type\": \"sse\","
echo "        \"url\": \"http://${IP}:8090/sse\""
echo "      }"
echo "    }"
echo "  }"
echo "============================================================"
