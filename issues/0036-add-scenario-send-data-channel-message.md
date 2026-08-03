# シナリオ操作 SendDataChannelMessage を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-scenario-send-data-channel-message
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の ScenarioPlayer は `OpSendDataChannelMessage { label, min_size, max_size }` を持ち、シナリオ実行中にラベル指定・サイズ指定で DataChannel メッセージを 1 回送信できる (`zakuro/src/scenario_player.h` の `OP_SEND_DATA_CHANNEL_MESSAGE` 処理)。C++ 版では reconnect シナリオの Sleep 間に挟まれて送信タイミングを制御している。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、`ScenarioPlayer::run_until_disconnect()` が順次実行する
- `src/data_channel.rs` には ZAKURO ヘッダ付きメッセージを構築する `build_message()` と、接続中に定間隔で連続送信する `run_messaging()` が実装済み。`build_message()` はラベル別カウンタと connection_id を引数に取り、`run_messaging()` は `HashMap<String, u64>` でラベル別カウンタを保持している
- C++ 版は BinaryPool から `min_size - 48` 〜 `max_size - 48` バイトのランダムバイナリを取得し、ZAKURO ヘッダ (シグネチャ 6 + 時刻 8 + ラベル別カウンタ 8 + connection_id 26 バイト) を付けて `SendMessage(label, data)` で送信する
- `src/virtual_client.rs` は接続中に `run_messaging()` を走らせる構成で、シナリオから DataChannel ハンドルを触る経路がない

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `SendDataChannelMessage { label: String, min_size: usize, max_size: usize }` を追加する
2. `ScenarioPlayer` が DataChannel ハンドルとラベル別カウンタを保持できるよう config を拡張し、`build_message()` を再利用する
3. connection_id は `src/stats.rs` の `StatsSnapshot` から取得可能なため、C++ 版と同じくヘッダに含める (既存 `run_messaging()` が connection_id に空文字列を渡している点は本 issue のスコープ外とし、既知の差分として残す)
4. `min_size` / `max_size` の意味は C++ 版に合わせ「ZAKURO ヘッダを含む合計サイズ」とし、ペイロードは `min_size - 48` 〜 `max_size - 48` バイトのランダムバイナリとする
5. 既存 reconnect シナリオに組み込むかは別途判断し、まずはシナリオ定義の拡張 (新シナリオ種別の追加等) で実現する

## 完了条件

- シナリオに SendDataChannelMessage 操作が定義できる
- 操作到達時に指定ラベル・指定サイズの ZAKURO ヘッダ付きメッセージが送信される
- ラベルごとのカウンタが送信のたびに増加する
