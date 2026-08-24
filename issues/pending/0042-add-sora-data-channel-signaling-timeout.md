# DataChannel シグナリングタイムアウト (`--sora-data-channel-signaling-timeout`) を追加する

- Created: 2026-08-24
- Completed: {YYYY-MM-DD}
- Branch: feature/add-sora-data-channel-signaling-timeout
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の `--sora-data-channel-signaling-timeout` は DataChannel でシグナリングを中継する場合に、DataChannel の確立を待つタイムアウトを指定する。負荷試験でまだ DataChannel が確立していない間に接続を試みる事態を避けるために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- zakuro-rs には対応する CLI 引数がなく (`src/args.rs` の `CommonArgs` / `InstanceArgs`)、`src/virtual_client.rs` の SoraConnectionBuilder 構築でも同オプションを指定していない
- DataChannel シグナリング自体 (`--sora-data-channel-signaling`) は `src/virtual_client.rs` で `data_channel_signaling` として実装済みだが、タイムアウト指定の API は存在しない
- C++ 版では Sora C++ SDK の `data_channel_signaling_timeout` 設定を `--sora-data-channel-signaling-timeout` で制御している

## 設計方針

1. `src/args.rs` の `InstanceArgs` に `--sora-data-channel-signaling-timeout <SEC>` を追加する (デフォルトは C++ 版に合わせる)
2. `src/virtual_client.rs` の SoraConnectionBuilder 構築で `Duration` を渡す
3. sora-rust-sdk に DataChannel シグナリングタイムアウトの API が必要

## 完了条件

- `--sora-data-channel-signaling-timeout` 指定時に、DataChannel 確立待機が指定時間でタイムアウトする (実サーバー接続での手動確認)

## pending にした理由

sora-rust-sdk (`sora_sdk` クレート) に DataChannel シグナリングタイムアウト API が実装されておらず、外部依存の対応待ちのため保留とする。

- sora_sdk 2026.1.0-canary.13 (現行使用) と 2026.1.0-canary.21 (調査時点の最新) の両方で、`data_channel_signaling` のタイムアウト指定 API が存在しないことをソースコードで確認済み
- ZAKURO.md の「sora-rust-sdk / webrtc-rs 側の制約により未実装の機能」に従い、sora-rust-sdk 側で API が追加された時点で対応する
