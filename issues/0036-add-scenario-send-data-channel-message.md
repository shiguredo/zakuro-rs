# シナリオ操作 SendDataChannelMessage を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-scenario-send-data-channel-message
- Polished: 2026-08-03

## 目的

zakuro (C++) の ScenarioPlayer は `OpSendDataChannelMessage { label, min_size, max_size }` を持ち、シナリオ実行中にラベル指定・サイズ指定で DataChannel メッセージを 1 回送信できる (`zakuro/src/scenario_player.h` の `OP_SEND_DATA_CHANNEL_MESSAGE` 処理)。C++ 版は DataChannel 連続送信を「Sleep(interval) → SendDataChannelMessage をループする」ラベル別サブシナリオ (`scenario-dcs-{label}`) としてメインシナリオと並行実行している (`zakuro/src/zakuro.cpp` の dcs シナリオ構築)。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、`ScenarioPlayer::run_until_disconnect()` が順次実行する
- `src/data_channel.rs` には ZAKURO ヘッダ付きメッセージを構築する `build_message()` と、接続中に定間隔で連続送信する `run_messaging()` が実装済み。`build_message()` はカウンタ値・connection_id・ペイロードサイズ・xorshift 状態を引数に取り、`run_messaging()` は `HashMap<String, u64>` でラベル別カウンタを保持している
- C++ 版は BinaryPool から `min_size - 48` 〜 `max_size - 48` バイトのランダムバイナリを取得し、ZAKURO ヘッダ (シグネチャ 6 + 時刻 8 + ラベル別カウンタ 8 + connection_id 26 バイト) を付けて `SendMessage(label, data)` で送信する
- `src/virtual_client.rs` は接続中に `run_messaging()` を走らせる構成で、シナリオから DataChannel ハンドルを触る経路がない

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `SendDataChannelMessage { label: String, min_size: usize, max_size: usize }` を追加する
2. シナリオプレイヤーは接続ループの外で 1 回生成され再接続をまたいで生存する一方、`SoraConnectionHandle` と connection_id は接続ごとに変わる。ハンドルと ids (`Arc<Mutex<Option<ConnectionIds>>>`) は `run_until_disconnect()` の実行時引数として毎接続渡し、ラベル別カウンタは `ScenarioPlayer` に保持して再接続をまたいで永続させる (C++ 版の `dc_counter_` 相当)
3. connection_id は `src/duckdb_stats/rows.rs` の `ConnectionIds` から取得する (offer 受信時に `parse_offer_ids()` で確定し、`src/virtual_client.rs` の ids が保持する)。`run_until_disconnect()` 開始時点では offer 未受信で ids が None のため、実行時に ids を読み取ってから送信し、未確定の場合やロック失敗 (poison) の場合は空文字列として送信する (poison 時は既存コードと同じく warning ログを出す)。既存 `run_messaging()` が connection_id に空文字列を渡している点は本 issue のスコープ外とし、既知の差分として残す (将来の対応余地は残す)
4. `min_size` / `max_size` の意味は C++ 版に合わせ「ZAKURO ヘッダを含む合計サイズ」とし、ペイロードは `min_size - 48` 〜 `max_size - 48` バイトのランダムバイナリとする。既存 `run_messaging()` と同じく 48 〜 256000 バイトの範囲検証と、`max_size < min_size` の場合は `max_size = min_size` にクランプする
5. ランダムペイロードの xorshift seed は既存の `compute_seed(instance_id, vc_id)` で初期化し、状態は `ScenarioPlayer` に保持して送信ごとに更新する (既存 `run_messaging()` と同じ方式)。instance_id / vc_id は `ScenarioPlayer` の生成時に受け取る
6. カウンタは vc ごとに独立させる (C++ 版は全 vc 共有だが、zakuro-rs は vc ごとに `ScenarioPlayer` が生成される)。同一ラベルで `run_messaging()` と併用した場合はカウンタが別系統になり、connection_id (空文字列と実値) の混在や xorshift 初期状態の同一性によるペイロード乱数列の相関が生じることを既知の制約として許容する
7. 送信失敗時 (DataChannel 未 open 等) は warning ログを出して操作は完了として扱い、ラベル別カウンタは失敗時も増加させる (C++ 版互換)
8. 既存 reconnect シナリオへの組み込みや新シナリオ種別の追加は本 issue では行わず、`ScenarioOp` への操作追加のみをスコープとする。C++ 版の dcs サブシナリオ相当の組み込みは PlaySubScenario 対応と合わせて別途検討する。動作確認は実サーバー (Sora SFU) への接続による手動確認で行う (モック・スタブ利用不可のため、送信ロジックの単体テストは `build_message()` 相当の構築部のみ)

## 完了条件

- シナリオ定義に SendDataChannelMessage 操作を含められる
- 操作到達時に指定ラベル・指定サイズ範囲の ZAKURO ヘッダ付きメッセージが送信される (実サーバー接続での手動確認)
- ラベルごとのカウンタが送信のたびに増加し、再接続後も継続する
