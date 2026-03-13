#!/usr/bin/env bash
# deploy.sh — ビルド → GRUB ISO 作成 → Alpine に転送 → サービス再起動
set -euo pipefail

ALPINE_HOST="${ALPINE_HOST:-root@YOUR_ALPINE_IP}"
REMOTE_ISO="/root/unikernel.iso"
KERNEL="target/x86_64-unknown-none/release/unikernel"
ISODIR="isodir"
ISO="unikernel.iso"

echo "==> ビルド中 (release)..."
cargo build --target x86_64-unknown-none --release 2>&1

echo "==> GRUB ISO 作成中..."
mkdir -p "$ISODIR/boot/grub"
cp "$KERNEL" "$ISODIR/boot/kernel"
cat > "$ISODIR/boot/grub/grub.cfg" << 'GRUBCFG'
set timeout=0
set default=0

menuentry "unikernel" {
    multiboot2 /boot/kernel
    boot
}
GRUBCFG
i686-elf-grub-mkrescue -o "$ISO" "$ISODIR" 2>/dev/null
echo "   ISO: $(du -h $ISO | cut -f1)"

echo "==> Alpine に転送中..."
scp "$ISO" "$ALPINE_HOST:$REMOTE_ISO"

echo "==> サービス再起動中..."
ssh "$ALPINE_HOST" '> /var/log/unikernel.log; rc-service unikernel restart'

echo "==> 起動確認中 (最大30秒)..."
for i in $(seq 1 30); do
    sleep 1
    if ssh "$ALPINE_HOST" 'grep -q "\[HTTP READY\]" /var/log/unikernel.log 2>/dev/null'; then
        echo "==> [OK] unikernel 起動完了"
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "==> [WARN] タイムアウト。ログを確認:"
        ssh "$ALPINE_HOST" 'tail -10 /var/log/unikernel.log'
        exit 1
    fi
done

echo "==> HTTP 疎通確認..."
RESP=$(curl -s --max-time 5 http://YOUR_ALPINE_HOST:8080/ 2>/dev/null || true)
echo "   レスポンス: $RESP"

if [ -n "$RESP" ]; then
    echo "[DEPLOY OK]"
else
    echo "[DEPLOY WARN] HTTP レスポンスなし"
fi
