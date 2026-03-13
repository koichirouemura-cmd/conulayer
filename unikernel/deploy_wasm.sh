#!/usr/bin/env bash
# deploy_wasm.sh — WASMのみをレジストリに更新（カーネル再ビルド不要）
set -euo pipefail

ALPINE_HOST="${ALPINE_HOST:-root@YOUR_ALPINE_IP}"

echo "==> WAT → WASM コンパイル..."
wat2wasm "wasm/app.wat"    -o "wasm/app.wasm"
echo "   app.wasm サイズ: $(wc -c < wasm/app.wasm) bytes"
wat2wasm "wasm/bbs.wat"    -o "wasm/bbs.wasm"
echo "   bbs.wasm サイズ: $(wc -c < wasm/bbs.wasm) bytes"
wat2wasm "wasm/editor.wat" -o "wasm/editor.wasm"
echo "   editor.wasm サイズ: $(wc -c < wasm/editor.wasm) bytes"

echo "==> レジストリに転送..."
scp "wasm/app.wasm"        "$ALPINE_HOST:/var/registry/app.wasm"
scp "wasm/bbs.wasm"        "$ALPINE_HOST:/var/registry/bbs.wasm"
scp "wasm/editor.wasm"     "$ALPINE_HOST:/var/registry/editor.wasm"
scp "wasm/bbs_ui.html"     "$ALPINE_HOST:/var/registry/bbs.html"
scp "wasm/eq_ui.html"      "$ALPINE_HOST:/var/registry/eq.html"
scp "wasm/editor_ui.html"  "$ALPINE_HOST:/var/registry/editor.html"

echo "==> unikernel 再起動..."
ssh "$ALPINE_HOST" '> /var/log/unikernel.log; rc-service unikernel restart'

echo "==> 起動確認中 (最大30秒)..."
for i in $(seq 1 30); do
    sleep 1
    if ssh "$ALPINE_HOST" 'grep -q "\[HTTP READY\]" /var/log/unikernel.log 2>/dev/null'; then
        echo "==> [OK] 起動完了"
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "==> [WARN] タイムアウト"
        exit 1
    fi
done

echo "==> HTTP 疎通確認..."
RESP=$(curl -s --max-time 5 http://YOUR_ALPINE_HOST:8080/ 2>/dev/null || true)
echo "   レスポンス: $RESP"
