# ホスト関数拡張 実装計画

## 設計思想

WASMは「ロジック」だけ持つ。インフラ能力はカーネルがホスト関数として提供する。
Linuxのシステムコールを、AI-native向けに最小再設計したもの。

```
WASM (Layer B)
  └─ host.https_get(url) → データ取得
  └─ host.now()          → 現在時刻
  └─ host.random()       → 乱数

Kernel (Layer A)
  └─ rustls + TCP client → HTTPS実装
  └─ CMOS RTC            → 時刻取得
  └─ RDRAND命令          → ハードウェア乱数
```

---

## Phase 1: host.now() — 時刻取得

**難易度**: 低
**理由**: RTC（Real Time Clock）はx86の標準ハードウェア。ポートI/Oで読める。

### カーネル側 (src/rtc.rs 新規)
```rust
// CMOSポート経由でRTCを読む
pub fn read_unix_timestamp() -> i64
```

### WASM ホスト関数
```
host.now() -> i64   // Unix timestamp (秒)
```

### 使用例 (WAT)
```wat
(import "host" "now" (func $now (result i64)))
;; 投稿時刻の記録、1分ごとのポーリング判定など
```

---

## Phase 2: host.random() — 乱数生成

**難易度**: 低
**理由**: x86の RDRAND 命令1発。

### カーネル側
```rust
// src/wasm_rt.rs のLinkerに追加
linker.func_wrap("host", "random", |_: Caller<()>| -> i64 {
    let mut val: u64;
    unsafe { core::arch::asm!("rdrand {}", out(reg) val) };
    val as i64
});
```

### WASM ホスト関数
```
host.random() -> i64
```

---

## Phase 3: host.https_get() — HTTPS通信

**難易度**: 高
**必要なもの**:
- `rustls` クレート（no_std + alloc対応）
- `webpki-roots` クレート（Mozillaルート証明書バンドル、~300KB）
- カーネル内アウトバウンドTCPクライアント（registry.rsのパターン流用）

### Cargo.toml 追加
```toml
rustls = { version = "0.23", default-features = false, features = ["alloc"] }
webpki-roots = "0.26"
```

### 実装構造
```
src/tls_client.rs  (新規)
  fetch_https(url: &str) -> Option<Vec<u8>>
    → DNS解決（IPをハードコードまたはhost.resolve追加）
    → smoltcp TCP接続
    → rustls TLSハンドシェイク
    → HTTP/1.0 GET送信
    → レスポンス受信・ヘッダスキップ
    → ボディ返却
```

### WASM ホスト関数
```
host.https_get(url_ptr: i32, url_len: i32, out_ptr: i32) -> i32
  // WASMメモリのout_ptrにレスポンスボディを書き込む
  // 戻り値: 書き込んだバイト数（-1でエラー）
```

### DNS問題
TLSには「どのIPに接続するか」だけでなく「ドメイン名による証明書検証」が必要。
IPアドレスはDNS解決が必要。

選択肢:
- A. カーネルにDNSクライアント追加（smoltcp UDPで実装）
- B. よく使うドメインのIPを設定ファイルで渡す（fw_cfg経由）
- C. まず固定IP（JMA等）でプロトタイプ、後でDNS追加

→ **Cから始める**が現実的。

---

## Phase 4: 地震監視アプリ (WASM)

ホスト関数が揃ったら作るもの。

### WASM の役割
```
handle_request() が呼ばれるたびに:
  1. host.now() で現在時刻を取得
  2. 前回フェッチから60秒以上経過していたら:
     host.https_get("http://10.0.2.2:8889/eqvol.xml") で取得
     (AlpineがJMA HTTPSをHTTPでリレー → Phase3完成前の暫定)
  3. XMLをパースして地震リストを抽出
  4. SVGで日本地図を生成、震源地をプロット
  5. HTML+SVGを返す
```

### Alpine リレー (暫定、Phase3完成まで)
```nginx
location /eqvol.xml {
    proxy_pass https://www.data.jma.go.jp/developer/xml/feed/eqvol.xml;
}
```

---

## 実装順序

```
1. host.now()    → src/rtc.rs + wasm_rt.rs修正   (1日)
2. host.random() → wasm_rt.rs修正のみ             (数時間)
3. Alpine nginx  → JMA XMLリレー設定              (数時間)
4. 地震WASMプロト → host.now() + Alpine経由HTTP   (1-2日)
5. host.https_get() → rustls導入・DNS暫定対応     (3-5日)
6. 地震WASM完成  → Alpine不要化                   (1日)
```

---

## 将来のホスト関数候補

| 関数 | 用途 |
|---|---|
| `host.dns_resolve(name)` | ドメイン→IP |
| `host.tcp_connect(ip, port)` | 生TCP（WebSocket等） |
| `host.kv_get/set(key, val)` | virtio-blkベースのKV（永続化） |
| `host.sleep_until(ts)` | タイマー待機 |

`host.kv_get/set` が入るとBBSの投稿が再起動後も残る。
