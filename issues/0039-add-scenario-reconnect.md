# シナリオ操作 Reconnect を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-scenario-reconnect
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の ScenarioPlayer は `OpReconnect` を持ち、シナリオ実行中にクライアントを再接続する (`zakuro/src/scenario_player.h` の `OP_RECONNECT` 処理)。`VirtualClient::Connect()` は接続中なら現在のシグナリングを切断して再接続するため、reconnect シナリオは「Reconnect → [Sleep(1-5s) + PlayVoiceNumberClient] × 9 → ループ先頭 (Reconnect) に戻る」という構造で再接続を繰り返す。C++ 版とのシナリオ定義互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、`ScenarioPlayer::run_until_disconnect()` が Disconnect 到達時に呼び出し元へ制御を返す。再接続は `src/virtual_client.rs` の `run()` が外側ループで自動的に行う
- 既存 reconnect シナリオ (`build_reconnect_scenario()`) は「[Sleep(1-5s)] × 9 → Disconnect → 外側ループで再接続」という構造で、C++ 版の「Reconnect → [Sleep + PlayVoiceNumberClient] × 9 → ループ」とはシナリオ定義の構造が異なる
- C++ 版の `add_reconnect_scenario` (`zakuro/src/zakuro.cpp`) は duration / repeat_interval 経路を「Sleep(duration) → Disconnect → Sleep(repeat_interval) → Reconnect (repeat_interval > 0 の場合)」というシナリオとして定義しているが、zakuro-rs では `duration_timer()` と repeat_interval の分岐で `run()` 側に実装している

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `Reconnect` を追加する
2. `ScenarioPlayer::run_until_disconnect()` は Reconnect 到達時に呼び出し元へ制御を返し、`run()` が接続を閉じて再接続する方式にする。シナリオプレイヤーは op_index を進めた状態を維持するため、現状の「接続ループの外で 1 回生成して op_index を保持する」構成を保つ (再接続後は続きの操作から再開される)
3. シナリオ定義として C++ 版と同じ「Reconnect → ... → ループ先頭に戻る」の構造が書けるようにする。既存の「Disconnect → 外側ループで再接続」経路は維持し、どちらの構造でも定義できるようにする
4. duration / repeat_interval 経路のシナリオ化 (C++ 版 `add_reconnect_scenario` 相当) は本 issue のスコープ外とし、既存の `run()` 側処理を維持する

## 完了条件

- シナリオに Reconnect 操作が定義できる
- Reconnect 到達後にクライアントが再接続され、シナリオが続きの操作から再開される
- reconnect シナリオを C++ 版と同じ「Reconnect → [Sleep + PlayVoiceNumberClient] × 9 → ループ」の構造で定義できる
