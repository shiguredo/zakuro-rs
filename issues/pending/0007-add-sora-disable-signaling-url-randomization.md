# シグナリング URL ランダム化無効 (`--sora-disable-signaling-url-randomization`) を追加する

- Created: 2026-08-24
- Completed: {YYYY-MM-DD}
- Branch: feature/add-sora-disable-signaling-url-randomization
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の `--sora-disable-signaling-url-randomization` はシグナリング URL のランダム化を無効化する。ランダム化は複数 URL を指定した場合に接続先を分散させるため、接続先がランダム化されるのを避けて単一 URL に固定したい負荷試験シナリオがある。C++ 版との機能互換性を維持するために対応する。

## 現状

- zakuro-rs には対応する CLI 引数がなく (`src/args.rs` の `CommonArgs` / `InstanceArgs`)、`src/virtual_client.rs` の SoraConnectionBuilder 構築でも同オプションを指定していない
- C++ 版では Sora C++ SDK の `SoraClientContext` 設定の `disable_signaling_url_randomization` を `--sora-disable-signaling-url-randomization` で制御している

## 設計方針

1. `src/args.rs` の `InstanceArgs` に `--sora-disable-signaling-url-randomization` フラグを追加する
2. `src/virtual_client.rs` の SoraConnectionBuilder 構築でフラグを渡す
3. シグナリング URL のシャッフルは sora_sdk 内部 (`SoraConnection::connect` 相当で URL シャッフル後に接続) で行われるため、zakuro-rs 側で実装するには sora-rust-sdk 側の対応が必要

## 完了条件

- `--sora-disable-signaling-url-randomization` 指定時に、複数の `--sora-signaling-url` が指定されていても常に先頭の URL へ接続される (実サーバー接続での手動確認)

## pending にした理由

sora-rust-sdk (`sora_sdk` クレート) にランダム化無効 API が実装されておらず、外部依存の対応待ちのため保留とする。

- sora_sdk 2026.1.0-canary.13 (現行使用) と 2026.1.0-canary.21 (調査時点の最新) の両方で、URL シャッフルが無条件で行われ無効化オプションが存在しないことをソースコードで確認済み
- ZAKURO.md の「sora-rust-sdk / webrtc-rs 側の制約により未実装の機能」に従い、sora-rust-sdk 側で API が追加された時点で対応する
