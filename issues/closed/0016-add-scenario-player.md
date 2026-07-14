# シナリオプレイヤー機能を追加する

Created: 2026-03-27
Completed: 2026-07-14
Model: Opus 4.6

## 概要

シナリオベースの動作シミュレーション機能を追加する。時間経過に応じた切断・再接続・終了などの操作を自動実行できるようにする。

## 根拠

zakuro (C++) では ScenarioPlayer により OpSleep、OpDisconnect、OpReconnect、OpExit などのオペレーションをシナリオとして定義し、自動実行できる。長時間の負荷試験で再接続パターンのテストや、段階的な負荷変動の再現に必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. シナリオプレイヤーモジュール

- シナリオの定義と実行エンジンを実装する
- 以下のオペレーションを実装する:
  - Sleep: 指定時間の待機
  - Disconnect: クライアントの切断
  - Reconnect: クライアントの再接続
  - Exit: プロセスの終了

### 2. コマンドライン引数

- `--scenario {reconnect}` オプションを追加する

### 3. VirtualClient との連携

- シナリオプレイヤーから VirtualClient の接続・切断を制御する

## 解決方法

`src/scenario.rs` に `ScenarioPlayer` / `ScenarioOp::{Sleep, Disconnect}` / `ScenarioType::Reconnect` を実装した。`--scenario reconnect` は `src/args.rs` で受け付け、`build_reconnect_scenario()` で 9 回のランダム Sleep (1-5 秒) の後に Disconnect するシナリオを構築する。

`src/virtual_client.rs` では接続中に `run_until_disconnect()` を走らせ、Disconnect 到達後に切断して外側ループで再接続する。C++ 版の `OpReconnect` に相当する明示オペレーションは持たず、再接続は VirtualClient 側のループで実現している。C++ 版 reconnect シナリオに挟まる PlayVoiceNumberClient は音声未対応のため Sleep のみとした。

本 issue 時点で未対応のまま残したもの:

- `OpExit`（duration / repeat_interval=0 経路。zakuro-rs では duration / repeat を VirtualClient 側で別処理）
- `OpPlayVoiceNumberClient`（音声基盤依存）
- シナリオ操作としての `OpSendDataChannelMessage`（`--sora-data-channels` による別系統の連続送信は実装済み）
