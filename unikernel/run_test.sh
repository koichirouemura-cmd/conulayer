#!/usr/bin/env bash
# run_test.sh — ビルド → GRUB ISO 作成 → QEMU 起動 → HTTP 確認
#
# Phase 4: HTTP サーバーとして動作確認。ポート転送 8080→80 で curl テスト。
set -euo pipefail

KERNEL="target/x86_64-unknown-none/debug/unikernel"
ISODIR="isodir"
ISO="boot.iso"

echo "==> ビルド中..."
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --target x86_64-unknown-none 2>&1

if [ ! -f "$KERNEL" ]; then
    echo "[FAIL] カーネルバイナリが見つかりません: $KERNEL"
    exit 1
fi

echo "==> GRUB ISO を作成中..."
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

echo "==> QEMU で起動中..."
TMPFILE=$(mktemp)
SECRET_VALUE="${FW_CFG_SECRET:-hello-from-fw_cfg}"
DISK_IMG="disk.img"

# ディスクイメージが存在しない場合のみ作成（8MB）
if [ ! -f "$DISK_IMG" ]; then
    echo "==> disk.img を作成中 (8MB)..."
    dd if=/dev/zero of="$DISK_IMG" bs=1M count=8 2>/dev/null
fi

# ポート転送: ホスト 8080 → VM 80（HTTP テスト用）
qemu-system-x86_64 \
    -cdrom "$ISO" \
    -m 128M \
    -serial "file:${TMPFILE}" \
    -display none \
    -no-reboot \
    -fw_cfg "name=opt/secret,string=${SECRET_VALUE}" \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0,hostfwd=tcp::8080-:80 \
    -drive file="${DISK_IMG}",format=raw,id=blk0,if=none \
    -device virtio-blk-pci,drive=blk0,disable-modern=true,disable-legacy=false \
    2>/dev/null &
QEMU_PID=$!

# [HTTP READY] が出るまで最大 20 秒待つ
echo "==> HTTP READY を待機中..."
for i in $(seq 1 40); do
    sleep 0.5
    if grep -q "\[HTTP READY\]" "$TMPFILE" 2>/dev/null; then
        break
    fi
done

# HTTP テスト
HTTP_RESPONSE=""
if grep -q "\[HTTP READY\]" "$TMPFILE" 2>/dev/null; then
    echo "==> HTTP リクエスト送信中..."
    HTTP_RESPONSE=$(curl -s --max-time 5 http://127.0.0.1:8080/ 2>/dev/null || true)
fi

kill $QEMU_PID 2>/dev/null || true
wait $QEMU_PID 2>/dev/null || true

OUTPUT=$(cat "$TMPFILE" 2>/dev/null || true)
rm -f "$TMPFILE"

echo "$OUTPUT"

if echo "$HTTP_RESPONSE" | grep -qE "unikernel|WASM"; then
    echo ""
    echo "HTTP レスポンス: $HTTP_RESPONSE"
    echo "[PASS] [NET OK] + HTTP レスポンス確認済み"
    exit 0
elif echo "$OUTPUT" | grep -q "\[NET OK\]"; then
    echo ""
    echo "[FAIL] [NET OK] は出ましたが HTTP レスポンスが取得できませんでした"
    exit 1
elif echo "$OUTPUT" | grep -q "\[SECRET OK\]"; then
    echo ""
    echo "[FAIL] [SECRET OK] は出ましたが [NET OK] がありません"
    exit 1
else
    echo ""
    echo "[FAIL] 起動に失敗しました"
    exit 1
fi
