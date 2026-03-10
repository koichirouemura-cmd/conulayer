# Conulayer — 設計ドキュメント

> 完全な設計思想・未実装の構想を含む原本は `original_plan.md` を参照。
> このドキュメントは「現在実装済みの内容」と「将来のRoadmap」を明確に分けて記述する。

---

## 0. なぜこれを作るのか

### 背景

現在のAIとのアプリ開発は、人間のために設計されたレイヤーの上で動いている。

```
意図・目的
    ↓
AIエージェント（Claude Code等）
    ↓
Python / Linux / シェル / ファイルシステム  ← 人間用に設計されたレイヤー
    ↓
実際にやりたいこと
```

PythonもLinuxも、**人間が読み書きし・操作するために作られたもの**だ。
AIが主体になったとき、これらは中間コストになる。

### 問い

> 「AIが主体であるなら、人間用に設計された中間レイヤーはそもそも必要か？」

### 答え（このプロジェクトの仮説）

```
意図・目的
    ↓
AIエージェント（意図を直接コードへ）
    ↓
unikernel（やりたいことだけが動いている）
```

人間用の中間レイヤーを排除することで、計算リソースを根本から削減できる。

### システムアーキテクチャ：2層構造

unikernel単体には根本的な制約がある。

```
unikernelの制約:
  コードを変更する = 全体を再コンパイル + 再起動
  → AIが動的にコンポーネントを追加・置き換えるには
    「再コンパイルなしに機能を差し込める層」が別途必要
```

そのため、アーキテクチャは2層に分ける。

```
┌──────────────────────────────────────────────────┐
│  Layer B: 動的WASMレイヤー（変化する）             │
│  ・Linux + Python ができることはすべてできる       │
│  ・AIが人間のニーズに応じて順次開発・追加          │
│  ・WASMモジュールとしてネットワーク経由で交換可能  │
│  ・再起動なしにコンポーネントを差し替えられる      │
├──────────────────────────────────────────────────┤
│  Layer A: ベアメタル固定層（変化しない）           │
│  ・ブート / メモリ管理 / ネットワーク基礎          │
│  ・設計を固め、以後は触らない                      │
│  ・これが「unikernelの本体」にあたる               │
└──────────────────────────────────────────────────┘
```

#### Layer B になぜ WASM を使うのか

理由はユーザー体験（再起動時間）ではない。**エネルギー効率の問題だ。**

```
Rustバイナリ: x86_64用・ARM64用・それぞれ別にビルドが必要
              → アーキテクチャごとにコンパイルコストが発生

WASM:         一度コンパイルすればどのアーキテクチャでも動く
              → 世界中のどのノードにも同じバイナリを配れる
              → コンポーネントの再利用コストが最小化
```

### エネルギーへの影響

典型的なWebアプリが動くとき、実際に必要な処理は数MBの仕事だが、
人間用レイヤーがそれを何十倍にも膨らませている：

```
Linux カーネル常駐メモリ:    〜50MB
Python ランタイム:           〜30MB
フレームワーク:              〜50MB
アプリ本体:                  〜 5MB  ← 本当にやりたいこと
─────────────────────────────────────
合計:                       〜135MB

unikernelなら:               〜2-5MB
```

これが世界中のデータセンターで何百万インスタンスも走り続けている。
**AIが動かすシステムの無駄を根本から削ることで、エネルギー消費の増加を抑えられる。**

### なぜエンジニアにはこの発想が難しいか

エンジニアはLinux・Pythonの知識が職業的資産になっている。
その資産を否定する方向への変化には、無意識に抵抗が生まれる。

**利害関係のない視点からこそ、この問いが立てられた。**

---

## 1. 現在の実装構成

### システム全体の構造

```
┌─────────────────────────────────────────┐
│  【人間のレイヤー】                      │
│  Alpine Linux（最小Linux VM）            │
│  ・APIキー・シークレット管理             │
│  ・個人データ管理                        │
│  ・ファイルシステムあり（人間が読み書き）│
│  ・アクセス制御・監査ログ               │
│  ・vsockサーバー常駐                     │
└──────────────────┬──────────────────────┘
                   │ vsock（許可されたデータ・鍵のみ渡す）
                   ↓
┌─────────────────────────────────────────┐
│  unikernel（KVMゲスト）                  │
│  Layer A: Rust no_std（ブート・NW・メモリ）│
│  Layer B: WASMランタイム（wasmi）         │
│    ├── app.wasm（地震モニター）           │
│    └── bbs.wasm（BBS）                   │
│  鍵・データを持たない・ファイルシステムなし│
└─────────────────────────────────────────┘
```

### 実装済みルーティング

```
GET /              → eq_ui.html（静的）Leaflet地図
GET /api/quake     → app.wasm（557バイト）JMA JSON取得
GET /bbs           → bbs_ui.html（静的）
GET /bbs/api/messages → bbs.wasm（1224バイト）
POST /bbs/post     → bbs.wasm
```

### シークレット管理の設計

平文の保管問題は業界未解決。このプロジェクトの解決策：
**人間レイヤーとAIレイヤーを分離する。**

- 鍵管理はAlpine Linux（人間が管理できるLinux VM）に集約
- unikernelは鍵を持たない。Alpine に要求するだけ
- 通信路はvsock（ホストOSのみアクセス可能な仮想ソケット）
- 「全部unikernelにする必要はない」が重要な設計判断

```
シークレット取得フロー（実装済み）:
  vsock経由でAlpineに要求（優先）
      ↓ Alpine未起動の場合
  fw_cfg（QEMU直接注入）にフォールバック
```

---

## 2. 技術スタックの選定

### 主言語: Rust `no_std` + `alloc`

**理由**: LLVM IRの直接記述は保守性が極めて低く、AIエージェントが反復的に修正するユースケースに適さない。RustのノウハウはAIモデルの訓練データに大量に含まれており、型システムによるバグ早期発見も重要な利点。

### ハイパーバイザー非依存設計

**このプロジェクトはProxmoxに縛られない。** Proxmoxは現在の出発点に過ぎない。VirtIO-netを持つ任意のハイパーバイザーで動く設計になっている。

| レイヤー | 依存性 |
|---|---|
| アプリ・ネットワーク・鍵管理 | 完全共通（アーキテクチャ非依存） |
| VirtIOプロトコル | 完全共通（業界標準） |
| ブート・ページング | ここだけアーキテクチャ固有 |

### WASMランタイム選定

| ランタイム | no_std対応 | 選定理由 |
|---|---|---|
| **wasmi** | ✅ | **採用。** no_std + alloc 対応・実績あり |
| tinywasm | ✅ | 候補だったが wasmi を選択 |
| wasmtime | ❌ | std必要・不可 |

### エコシステム成熟度

| コンポーネント | クレート / ツール | 成熟度 |
|---|---|---|
| CPU抽象化 | `x86_64` crate | 高 |
| VirtIO-net | `virtio-drivers` (rcore-os) | 中〜高 |
| TCP/IP | `smoltcp` | 高 |
| メモリ管理 | `linked_list_allocator` | 高 |
| シリアル出力 | `uart_16550` | 高 |
| WASMランタイム | `wasmi` | 高 |

---

## 3. 実装フェーズ記録

### Phase 0: 開発環境構築 ✅ 完了

**成果物**: シリアルポートに文字列を出力するだけの最小unikernel

**ブートについての設計変更**:
> 当初はQEMUの`-kernel`オプションによるMultiboot2ロードを予定していた。
> しかしQEMU 10.x が 64-bit ELF + Multiboot2 を `-kernel` で拒否することが判明したため、
> **GRUB ISO経由（`-cdrom`）に変更**した。

```
現在のブートシーケンス:
BIOS → GRUB（ISO内） → Multiboot2ヘッダ検出
→ ELFセグメントをメモリへロード
→ カーネルエントリポイントへJMP（32bitプロテクトモード）
→ GDT設定 → ページング → 64bitロングモードへ移行
→ Rust kernel_main() へ
```

---

### Phase 1: メモリ管理 ✅ 完了

**成果物**: `alloc::vec![]` 等が使える動的メモリ管理

- `linked_list_allocator` + 独自 BumpAllocator
- ページングは恒等写像（仮想アドレス == 物理アドレス）のみ採用

---

### Phase 2: シークレット取得 ✅ 完了

**成果物**: fw_cfg（0x510/0x511ポート）経由でQEMUから文字列を取得

**設計変更**:
> Phase 2では fw_cfg で実装。Alpine + vsock の構成に移行後、
> vsock優先・fw_cfgフォールバックの実装に変更。
> SecretProviderトレイトの差し替えで上位層は変更不要。

---

### Phase 3: PCI列挙 + VirtIO-net初期化 ✅ 完了（2026-03-05）

**成果物**: VirtIO-netデバイスの検出・初期化・送受信ができる状態

**主要バグと修正**:
- VirtIO QUEUE_SIZE をコード内でハードコードしていたが、デバイスは異なる値を報告 → `inw(REG_QUEUE_SIZE)` でデバイス報告値を動的に使用
- QUEUE_NOTIFY を DRIVER_OK 前に発行 → VirtIO 仕様違反。DRIVER_OK 後に移動

---

### Phase 4: TCP/IPスタック統合 ✅ 完了（2026-03-05）

**成果物**: ポート80でHTTPリクエストを受信してレスポンスを返すTCPサーバ

**主要バグと修正**:
- smoltcp 0.11 に `proto-arp` feature は存在しない（`proto-ipv4` に統合）→ 削除
- TX バッファ競合: smoltcp が1回の poll で複数パケット送る際に単一バッファを上書き → QUEUE_BUFS(16)枚の独立バッファに変更
- **最重要**: VirtIO QUEUE_SIZE のハードコード問題（Phase 3と同根）

---

### Phase 5: ビルドパイプライン・自動化 ✅ 完了（2026-03-05）

**成果物**: `run_test.sh` 一発で「ビルド → ISO作成 → QEMU起動 → HTTP疎通確認 → PASS/FAIL」が自動化

```bash
# run_test.sh の処理:
# 1. cargo build --target x86_64-unknown-none
# 2. i686-elf-grub-mkrescue -o boot.iso isodir/
# 3. qemu-system-x86_64 -cdrom boot.iso ... &
# 4. [HTTP READY] が出るまで待機してcurlテスト
# 5. レスポンスに期待文字列があれば [PASS]
```

---

### Phase 6: ハイパーバイザーデプロイ ✅ 完了

**成果物**: Proxmox VE上で実際に動作する状態

**デプロイ方法**:
```bash
# GRUB ISO → qcow2 変換
qemu-img convert -f raw -O qcow2 boot.iso app.qcow2

# Proxmox に転送してVM作成
scp app.qcow2 root@proxmox:/var/lib/vz/images/200/
qm create 200 --name conulayer --memory 128 --cores 1 \
  --net0 virtio,bridge=vmbr0
```

> **設計変更メモ**: 当初は ELF を `-kernel` で直接指定する予定だったが、
> QEMU 10.x の制約により GRUB ISO 経由に変更。

---

### Layer B完成 + SPA化 ✅ 完了（2026-03-08）

**成果物**: WASMランタイム（wasmi）実装 + 地震モニター・BBSのSPA化

**実装知見**:
- WASMマウントが "/" の場合、渡されるパスは先頭スラッシュなし（`api/quake` not `/api/quake`）
- JMAデータ173KB → smoltcp TX バッファ 256KB 必要
- HTTPヘッダー長は実測して正確に計算（実測102バイト）

---

## 4. 各コンポーネント技術詳細

### 4.1 メモリ管理

ページングは**恒等写像（Identity Mapping）**のみ採用（仮想アドレス == 物理アドレス）。

```
物理メモリレイアウト（-m 128M 想定）:
0x0000_0000 - 0x0009_FFFF: BIOS・VGAリザーブ
0x0010_0000 - 0x07FF_FFFF: カーネル + ヒープ（〜127MB）
```

- グローバルアロケータ: `linked_list_allocator`
- ヒープ領域: 8MB（`0x0200_0000` から）

### 4.2 fw_cfg読み出し

```
I/Oポート 0x510（セレクタ）/ 0x511（データ）
fw_cfg ディレクトリ: selector=0x0019 でエントリ一覧取得
任意ファイル: 名前で検索してselectorを取得し読み出し
```

### 4.3 smoltcp構成

```toml
smoltcp = { features = ["proto-ipv4", "socket-tcp", "medium-ethernet"] }
```

- 静的IP: 10.0.2.15/24（QEMU user network）
- デフォルトGW: 10.0.2.2
- TCP port 80 でHTTP/1.1サーバ

### 4.4 WASMランタイム（wasmi）

- 起動時にAlpine（10.0.2.2:8888）からWASMをHTTPでダウンロード
- ルートプレフィックスでディスパッチ（`/` → app.wasm、`/bbs` → bbs.wasm）
- WASMモジュールへのホスト関数: HTTPリクエスト読み取り・レスポンス書き込み

---

## 5. リスクと対策

| リスク | 対策 |
|---|---|
| smoltcp APIの破壊的変更 | バージョン固定（Cargo.lock） |
| WASMモジュールのメモリ境界違反 | wasmiのサンドボックスが自動検出 |
| VirtIO仕様違反による動作不定 | run_test.sh の自動テストで毎回検証 |
| fw_cfg非対応環境 | vsockフォールバック実装済み |

---

## 6. AIエージェント自律実装のための指示体系

このプロジェクトはAIが自律的に実装・デバッグすることを前提に設計されている。

### 6.1 絶対制約

- シークレットをコードにハードコードしない（必ずvsock or fw_cfg経由）
- シリアル出力は `[PHASE] 内容` フォーマットを守る（AIがログを読んで自己修正するため）
- `run_test.sh` が通ることが実装完了の条件

### 6.2 フェーズ遷移チェックリスト

各フェーズ完了の条件:
1. `run_test.sh` が `[PASS]` を返す
2. シリアルログに想定外の `[ERROR]` がない
3. 次フェーズの前提条件（メモリ確保、デバイス初期化等）が満たされている

### 6.3 エラーパターンと対処法

| エラー | 原因 | 対処 |
|---|---|---|
| QEMU が `-kernel` を拒否 | QEMU 10.x + 64bit ELF | GRUB ISO経由に変更 |
| TCP接続後にパケットが届かない | VirtIO QUEUE_SIZE 不一致 | `inw(REG_QUEUE_SIZE)` で動的取得 |
| smoltcp poll で TX 上書き | バッファ1枚 | QUEUE_BUFS 枚の独立バッファ |
| HTTP レスポンスが途切れる | smoltcp TX バッファ不足 | バッファを 256KB に拡大 |

---

## 7. 先行調査

2026年3月時点での調査結果。

| 要素 | 現状 |
|---|---|
| unikernel技術 | 解決済み（MirageOS 2013〜） |
| Rust no_std + VirtIO + smoltcp | 解決済み（hermit-os等） |
| AIがunikernelコードを生成 | 部分的（研究段階） |
| **システム全体をAI主体で設計** | **未解決・このプロジェクトの新しい問い** |

### 特筆すべき先行プロジェクト

| プロジェクト | 概要 | 差異 |
|---|---|---|
| hermit-os/kernel | Rust no_std unikernel | 人間が開発・運用する前提 |
| Unikraft | unikernel構築フレームワーク | AIによる自律生成を想定していない |
| AIOS | AI向けOS論文（2024） | Linux上のソフトウェア層として設計 |
| Solid Project | データ主権 | unikernelと無関係 |

**この組み合わせ（AI × unikernel × データ主権）を体系化したものは存在しない。**

---

## 8. Roadmap

> 以下は現時点で未実装。

### Phase 7: ARM64対応

- `src/arch/aarch64/` を追加するだけで上位層は変えない設計
- Apple Silicon Mac上でネイティブ動作
- x86_64との共通コードは変更なし

### MCPサーバー統合（拡張）

unikernelの操作（deploy/logs/status）はMCPで実装済み。
今後はAlpineのシークレット管理操作もMCPから行えるようにする。

### DHCP対応・配布可能ISO

現状は静的IP（10.0.2.15）固定。
DHCP対応により「ISOをダウンロードしてどこでも即起動」が実現できる。

---

## 9. 参考プロジェクト

- [hermit-os/kernel](https://github.com/hermit-os/kernel) — Rust no_std unikernel（最も近い参照実装）
- [syswonder/RuxOS](https://github.com/syswonder/ruxos) — Rust unikernel
- [Writing an OS in Rust](https://os.phil-opp.com/) — phil-opp（学習リソース）
- [smoltcp](https://github.com/smoltcp-rs/smoltcp) — no_std TCP/IPスタック
- [wasmi](https://github.com/wasmi-labs/wasmi) — no_std WASMランタイム
