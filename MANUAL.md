# AI-Native Unikernel 実践マニュアル

## このマニュアルの使い方

- **あなたがやること**: ターミナルでコマンドを実行する
- **Claudeがやること**: コードを書く・問題を診断する・次の指示を出す
- **各ステップの終わりに「確認」がある** → それが出たら次へ進む
- **うまくいかなかったら**: 出力をそのままClaudeに貼る

---

## 環境


| 項目    | 内容                           |
| ----- | ---------------------------- |
| 開発機   | Mac Studio M4 Max（ARM64）     |
| デプロイ先 | Proxmox（x86_64）              |
| 最初の目標 | Proxmox上でunikernelが起動してログが出る |


---

## Phase 0：最初のブート（シリアルに `[BOOT OK]` を出す）

### Step 1: 開発ツールのインストール

ターミナルを開いて、以下を1行ずつ実行してください。

```bash
# Homebrewが入っていない場合（入っている場合はスキップ）
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

```bash
# QEMUのインストール（ローカルテスト用）
brew install qemu
```

```bash
# Rustのインストール
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Rustのインストール中に選択を求められたら `1`（デフォルト）を押してEnter。

インストール完了後、ターミナルを一度閉じて開き直す。

```bash
# Rustのツールチェーンを追加
rustup target add x86_64-unknown-none
rustup component add rust-src llvm-tools-preview
```

**確認**: 以下を実行して、バージョンが表示されればOK

```bash
rustc --version
qemu-system-x86_64 --version
```

---

### Step 2: プロジェクトフォルダを作る

```bash
# このプロジェクトのフォルダに移動
cd ~/Claudecode/Program/OS

# unikernelフォルダを作成
mkdir -p unikernel/src
mkdir -p unikernel/.cargo
```

ここからはClaudeがファイルを作ります。準備ができたら「Step 2完了」と伝えてください。

---

### Step 3: ビルドする

Claudeがファイルを作り終えたら：

```bash
cd ~/Claudecode/Program/OS/unikernel
cargo build --target x86_64-unikernel.json
```

**確認**: `Compiling unikernel` と表示されてエラーなく終わればOK

---

### Step 4: Macローカルでテストする

```bash
./run_test.sh
```

**確認**: ターミナルに `[BOOT OK]` が表示されればOK

---

### Step 5: Proxmoxにデプロイする

**Proxmoxの準備（ブラウザでProxmoxにログイン）**

1. ProxmoxのUIを開く
2. 新しいVMを作成（詳細はClaudeが案内します）

**ファイルをProxmoxに転送**（ProxmoxのIPアドレスを入れてください）



```bash
scp target/x86_64-unikernel/debug/unikernel root@【ProxmoxのIP】:/var/lib/vz/kernels/
```

**VM起動（Proxmoxのシェルで実行）**

```bash
qm set 200 --args "-kernel /var/lib/vz/kernels/unikernel -serial file:/var/log/unikernel.log"
qm start 200
```

**ログ確認**

```bash
cat /var/log/unikernel.log
```

**確認**: `[BOOT OK]` が表示されればPhase 0完了

---

## トラブルシューティング


| 症状             | やること                                       |
| -------------- | ------------------------------------------ |
| コマンドがエラーになる    | エラーメッセージをそのままClaudeに貼る                     |
| ビルドが通らない       | `cargo build` の出力全部をClaudeに貼る              |
| QEMUで何も表示されない  | `run_test.sh` の出力をClaudeに貼る                |
| Proxmoxでログが出ない | `cat /var/log/unikernel.log` の結果をClaudeに貼る |


---

## 現在地

- Step 1: 開発ツールのインストール
- Step 2: プロジェクトフォルダを作る
- Step 3: ビルドする
- Step 4: Macローカルでテストする
- Step 5: Proxmoxにデプロイする

