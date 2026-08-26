# シナリオ操作 Reconnect を追加する

- Created: 2026-08-03
- Completed: 2026-08-26
- Branch: feature/add-scenario-reconnect
- Polished: 2026-08-26

## 目的

zakuro (C++) の ScenarioPlayer は `OpReconnect` を持ち、シナリオ実行中にクライアントを再接続する (`zakuro/src/scenario_player.h` の `OP_RECONNECT` 処理)。C++ 版の reconnect シナリオは「Reconnect → [Sleep(1-5s) + PlayVoiceNumberClient] × 8 → Sleep(1-5s) → ループ先頭 (Reconnect) に戻る」という構造で再接続を繰り返す (`zakuro/src/zakuro.cpp` の reconnect シナリオ構築)。既存のシナリオプレイヤー実装は「明示的な Reconnect 操作を持たず、再接続は VirtualClient 側の外側ループで実現する」判断で作られており、本 issue はシナリオ操作としての Reconnect を追加する (外側ループによる再接続は維持する)。C++ 版とのシナリオ定義互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` / `Disconnect` / `Exit` / `SendDataChannelMessage` を持つ (Exit と SendDataChannelMessage は実装済み)。`ScenarioPlayer::run_until_disconnect()` は Disconnect 到達時に `ScenarioEnd::Reconnect`、Exit 到達時に `ScenarioEnd::Exit` を返して呼び出し元へ制御を返す。再接続は `src/virtual_client.rs` の `run()` が外側ループで自動的に行う
- 既存 reconnect シナリオ (`build_reconnect_scenario()`) は「[Sleep(1-5s)] × 9 → Disconnect → 外側ループで再接続」という構造で、C++ 版の構造とは異なる (再接続をシナリオ内操作として定義できない)
- `PlayVoiceNumberClient` 操作は未実装 (open) であり、本 issue のシナリオ書き換え (設計方針 3) はその実装が前提となる
- C++ 版の `add_reconnect_scenario` (`zakuro/src/zakuro.cpp`) は duration / repeat_interval 経路をシナリオとして定義しているが、zakuro-rs では `duration_timer()` と repeat_interval の分岐で `run()` 側に実装している

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `Reconnect` を追加する
2. `ScenarioPlayer::run_until_disconnect()` は Reconnect 到達時に呼び出し元へ制御を返す。呼び出し元は Disconnect 到達時と同じく「切断 → 即再接続」を行うため区別は不要であり、既存の `ScenarioEnd::Reconnect` (Disconnect 操作と同じ値) を返す (`ScenarioEnd` は Exit 操作対応時に制御を返す理由を区別する形で設計済みで、Reconnect 操作も同じ値を返す予定として 0037 で確定済み)。シナリオプレイヤーは Disconnect と同じく op_index を進めてから返り、再接続後は続きの操作から再開される (接続ループの外で 1 回生成して op_index を保持する現状の構成を保つ)。再接続は切断完了を待ってから行う直列処理であり、C++ 版の Reconnect (切断と並行して次の操作を開始する) とは実行タイミングが異なる (意図した差分)
3. 既存の `build_reconnect_scenario()` を C++ 版と同じ「Reconnect → [Sleep + PlayVoiceNumberClient] × 8 → Sleep → ループ」の構造へ書き換える。`PlayVoiceNumberClient` 操作は未実装 (open) のため、本項目はその実装後に実施する。書き換えに伴い、接続確立直後に先頭の Reconnect が切断 → 再接続を 1 回行う点は C++ 版との挙動差として許容する (C++ 版は Reconnect 操作が初回接続を兼ねるが、zakuro-rs は接続してからシナリオを実行するため、この差分は避けられない)
4. duration / repeat_interval 経路のシナリオ化 (C++ 版 `add_reconnect_scenario` 相当) は本 issue のスコープ外とし、既存の `run()` 側処理を維持する
5. 動作確認は実サーバー (Sora SFU) への接続による手動確認で行う (モック・スタブ利用不可のため。ログの connected / disconnecting per scenario 出力と DuckDB の接続行数で確認する)。続きの操作からの再開は、各接続の持続時間が Sleep 合計 (9-45 秒) に相当するパターンになることで確認する

## 完了条件

- シナリオに Reconnect 操作が定義できる
- Reconnect 到達後にクライアントが再接続され、シナリオが続きの操作から再開される (実サーバー接続での手動確認)
- reconnect シナリオを C++ 版と同じ「Reconnect → [Sleep + PlayVoiceNumberClient] × 8 → Sleep → ループ」の構造で定義できる (PlayVoiceNumberClient 操作の実装後。実サーバー接続での手動確認)

## 解決方法

設計方針どおり、`ScenarioOp` に `Reconnect` を追加し、`build_reconnect_scenario()` を C++ 版と同じ構造へ書き換えた。呼び出し元 (外側ループ) は既存の `ScenarioEnd::Reconnect` 経路で切断 → 即再接続するため、新しい分岐は不要だった。

- `src/scenario.rs`: `ScenarioOp` に `Reconnect` (引数なし) を追加。`run_until_disconnect()` は Reconnect 到達時に op_index を進めてから `ScenarioEnd::Reconnect` を返す (Disconnect 操作と同じ動作。再接続後は続きの操作から再開される)。`build_reconnect_scenario()` を「Reconnect → [Sleep(1-5s) + PlayVoiceNumberClient] × 8 → Sleep(1-5s) → ループ先頭 (Reconnect)」の 18 ops / loop_index 0 に書き換え。PlayVoiceNumberClient は reconnect シナリオに組み込んだため `#[cfg_attr(not(test), expect(dead_code))]` を外し、逆に reconnect シナリオから使われなくなった Disconnect に `#[cfg_attr(not(test), expect(dead_code))]` を付けた (既存 Exit と同じパターン)
- `src/virtual_client.rs`: シナリオモードのコメントを Reconnect を含む形に更新 (コード変更なし)
- 接続確立直後に先頭の Reconnect が切断 → 再接続を 1 回行う点は設計方針どおりの意図した差分 (初回接続は短命になり、接続 2 以降の各接続が Sleep 合計 9-45 秒で持続する)

追加したテスト (`src/scenario.rs`。実サーバー接続での手動確認は完了条件の確認に使用):

- `test_run_until_disconnect_reconnect_op`: Reconnect 操作で `ScenarioEnd::Reconnect` が返り、op_index が進むこと
- `test_run_until_disconnect_reconnect_after_loop_wrap`: ループ折返し後に先頭の Reconnect へ戻り、再接続後も op_index 1 (先頭 Reconnect の次) から再開されること
- `test_build_reconnect_scenario_matches_cpp_structure`: reconnect シナリオが C++ 版と同じ構造 (18 ops / loop_index 0 / [Sleep + PlayVoiceNumberClient] × 8 の交互 / Sleep 1-5 秒) であること

実サーバーでの手動確認は、シナリオ操作 PlayVoiceNumberClient (0038) の数字音声再生確認とあわせて実施する。確認時は「初回接続が短命であること」「以降の各接続の持続時間が 9-45 秒のパターンになること (続きの操作からの再開)」を確認する (初回接続の DuckDB 接続行はタイミング依存で現れない可能性がある)。
