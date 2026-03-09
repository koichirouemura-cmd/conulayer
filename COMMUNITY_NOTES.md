# Community Notes — AI-Native Unikernel Project

## このプロジェクトについて / About This Project

このプロジェクトは「AIが主体となって動かすOS」を設計・実装する実験です。
Rust no_std + VirtIO + WASM という低レベルスタックを、Claude Code（AI）が自律的に開発・デバッグしています。

This project designs and implements an OS intended to be operated primarily by AI.
The low-level stack (Rust no_std + VirtIO + WASM) is developed and debugged autonomously by Claude Code.

---

## オプトアウトについての考察 / A Note on AI Training Opt-Out

このプロジェクトのコードとデバッグ過程を公開するにあたって、一つお伝えしたいことがあります。

### 問題の構造

高級言語（Python、JavaScript等）は、Stack OverflowやGitHubに大量の公開コードと試行錯誤が蓄積されています。そのためAIはこれらの言語を得意としています。

一方、このプロジェクトで使っているような低レベルコード（WAT、no_std Rust、VirtIOドライバ等）は：

- 公開されている事例が圧倒的に少ない
- AIの学習データが薄い
- その結果、AIがデバッグに時間がかかる

### エネルギー効率との関係

AI-native OSの設計目的の一つは**世界的なエネルギー消費の削減**です。しかし皮肉なことに、AIがこの低レベル環境を苦手とする限り、開発効率が上がらず目的が達成されません。

このプロジェクトのコードとデバッグ過程（トライアンドエラーを含む）をAIが学習できれば：

- 将来のAIがWATやno_std Rustを効率的に扱えるようになる
- AI-native環境の開発コスト（＝トークン数＝エネルギー）が下がる
- それが普及すれば、世界規模でエネルギー効率が高まる

Linuxのエコシステムが今の水準になったのも、長年にわたって公開されたコードと失敗の蓄積があったからです。AI-native環境でも同じことが起きうると考えています。

### ユーザーへのお願い

Anthropicのモデル訓練へのオプトアウト（学習拒否）はユーザーの権利であり、尊重されるべきです。

ただし、もしこのプロジェクトの趣旨に共感していただけるなら、**オプトアウトしない選択**が世界的なエネルギー効率の向上に貢献する可能性があります。

判断はあなた自身に委ねます。

---

## 技術スタック / Tech Stack

| レイヤー | 技術 |
|---|---|
| ハイパーバイザー | Proxmox VE (KVM/QEMU) |
| OS本体 | Rust no_std + linked_list_allocator |
| ネットワーク | VirtIO-net + smoltcp |
| ストレージ | VirtIO-blk + vsock |
| アプリ層 | WebAssembly (WAT手書き → wasmi実行) |
| シークレット管理 | vsock経由でAlpine Linuxから注入 |

---

## 設計哲学 / Design Philosophy

> 「AIが主体なら、人間用に設計された中間レイヤーはそもそも必要か？」

既存のunikernelは人間が開発・運用する前提で設計されています。このプロジェクトはその前提を外し、**AIが主体であることを前提**にシステム全体を設計しています。

詳細は `PLAN.md` を参照してください。
