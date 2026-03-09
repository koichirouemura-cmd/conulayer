#!/usr/bin/env bash
set -euo pipefail

# デフォルト値は環境に合わせて変更してください / Change this to your Alpine VM's IP address
ALPINE_HOST="${ALPINE_HOST:-192.168.1.100}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "==> Deploying vsock file server to Alpine at ${ALPINE_HOST}..."

# ファイルをコピー
scp "${SCRIPT_DIR}/vsock-file-server.py" \
    "root@${ALPINE_HOST}:/usr/local/bin/vsock-file-server.py"

scp "${SCRIPT_DIR}/vsock-file-server.openrc" \
    "root@${ALPINE_HOST}:/etc/init.d/vsock-file-server"

# Alpine 側でセットアップ
ssh "root@${ALPINE_HOST}" bash <<'REMOTE'
chmod +x /usr/local/bin/vsock-file-server.py
chmod +x /etc/init.d/vsock-file-server
mkdir -p /data
rc-update add vsock-file-server default
rc-service vsock-file-server restart
echo "vsock-file-server status:"
rc-service vsock-file-server status
REMOTE

echo "Done. File server deployed and running."
