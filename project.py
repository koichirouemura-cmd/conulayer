# AI-Native Unikernel 開発方針書

## 1. プロジェクト概要
本プロジェクトの目的は、汎用OS（Linux等）やコンテナ技術（Docker等）を一切排除し、AIエージェント（Claude Code）がハードウェア（ハイパーバイザ）の直上で動作する「極限まで薄い専用OS（Unikernel）」を自律的に生成・運用する環境を構築することである。
「1つのアプリケーションにつき、1つの専用OS（VM）」という1対1の原則を貫き、セキュリティ、起動速度、リソース効率の最大化を図る。

## 2. アーキテクチャ基本制約
* **インフラストラクチャ:** Proxmox VE (KVM/QEMU) を本番環境とする。
* **OSレス / 依存関係の排除:** POSIX API、シェル、ファイルシステム、標準ライブラリ（libc等）は存在しない。
* **出力形式:** アプリケーションのコアロジックは、AIが **LLVM IR**（または `no_std` のRust/C）で直接記述し、コンパイラバックエンドを通じて単一のブータブルイメージ（`.qcow2`）を生成する。
* **ネットワーク:** QEMU/Proxmoxの VirtIO-net デバイスと直接対話する最小限のドライバ・TCPスタックを実装（またはリンク）する。

## 3. シークレット管理（最も重要なセキュリティ原則）
**APIキー、パスワード等の機密情報をソースコード（LLVM IR含む）内にハードコードすることは固く禁ずる。**

* シークレットは実行時にQEMU/Proxmoxの **`fw_cfg` (Firmware Configuration Device)** 経由でホストからゲストOSのメモリへ直接注入される。
* OSの初期化フェーズにて、指定されたI/Oポート（x86の場合は `0x510`, `0x511`等）を叩き、`fw_cfg` から必要なキー（例: `opt/myapp/api_key`）を動的に読み出すロジックを実装すること。

## 4. 開発・デバッグワークフロー（ローカルMac環境）
本番環境（Proxmox）へデプロイする前に、開発マシンのターミナル上でQEMUを用いた「全自動デバッグループ」を回す。

### 4.1. 必要なツールチェーン
* `qemu-system-x86_64` (テスト起動用エミュレータ)
* `llvm` / `clang` / `lld` (コンパイルおよびリンク用)

### 4.2. テスト自動化スクリプト (`run_test.sh`)
AIエージェントはコード修正後、以下のステップを自動実行するスクリプトを用いて動作検証を行う。
1. **ビルド:** LLVM IRをコンパイルし、ブートローダ等と静的リンクして `.qcow2` を生成。
2. **QEMU起動:** バックグラウンドでQEMUを起動し、シリアルポート出力を標準出力へリダイレクト。
   ```bash
   # 起動オプション例
   qemu-system-x86_64 \
     -drive file=app.qcow2,format=qcow2,if=virtio \
     -netdev user,id=n1,hostfwd=tcp::8080-:80 -device virtio-net,netdev=n1 \
     -fw_cfg name=opt/myapp/api_key,string=DUMMY_KEY \
     -serial stdio -display none &