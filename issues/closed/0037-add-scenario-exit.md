# シナリオ操作 Exit を追加する

- Created: 2026-08-03
- Completed: 2026-08-03
- Branch: feature/add-scenario-exit
- Polished: 2026-08-03

## 目的

zakuro (C++) の ScenarioPlayer は `OpExit` を持ち、シナリオ実行中にクライアントを切断し、全クライアントの Exit 完了後にプロセスを終了する (`zakuro/src/scenario_player.h` の `OP_EXIT` 処理)。シナリオ定義による試験全体のライフサイクル制御 (任意の位置での切断終了) のために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、`ScenarioPlayer::run_until_disconnect()` が Disconnect 到達時に呼び出し元へ制御を返す
- プロセス終了の経路は Ctrl+C (`tokio::signal::ctrl_c` → `CancellationToken`。2 回目の Ctrl+C は強制終了) と、`--duration` + `--repeat-interval` なし (または 0) の VirtualClient 側処理 (duration 経過で切断 → ループ break → 全 vc タスク終了 → プロセス終了)、予期しない切断での `max_retry` 超過による当該 vc タスクの終了である (プロセス終了は全 vc タスクの終了後)。なおシナリオモードでは duration タイマーは動かない
- `issues/closed/0016-add-scenario-player.md` の解決方法で、OpExit は「duration / repeat_interval=0 経路。zakuro-rs では duration / repeat を VirtualClient 側で別処理」として未対応のまま残されている

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `Exit` を追加し、`ScenarioPlayer` の実行ループを Exit 到達時に完了として終了する
2. `run_until_disconnect()` の戻り値を、制御を返した理由 (Disconnect / Exit) を区別できる値に変更する。呼び出し元は Disconnect で返ったら再接続し、Exit で返ったら再接続しない (命名と戻り値の形は Reconnect 操作対応と整合させて実装時に決定する)
3. 呼び出し元 (`src/virtual_client.rs`) は Exit 到達時に接続を切断した後に vc タスクを終了させる (ループ break)。切断処理は既存のシナリオ Disconnect 分岐と同じく `handle.disconnect()` 待機と `connection_token.cancel()` と `StatsEvent::Disconnected` 送信を行う
4. プロセス終了は追加機構を設けず、既存の「vc タスク終了 → `clients.join_next()` → `instances.join_next()` → main の `Ok(())`」という自然完了チェーンで実現する (C++ 版の全クライアント exit 後に `io_context.stop()` に相当)。Exit 経路ではプロセス全体の CancellationToken を cancel しない (Ctrl+C 専用の仕組みであり、呼ぶと他 vc が Exit 未到達のまま Shutdown で終了し、シナリオの完了条件である「全クライアントが Exit した後」を満たせなくなるため)
5. シナリオ内の Exit の配置は C++ 版と同じく「Disconnect の後に置く」ことができ、その場合 zakuro-rs では再接続後に実行される (op_index が接続をまたいで継続するため)。C++ 版は Exit 後も Next() でシナリオが継続するが、zakuro-rs では Exit 到達時に実行ループを完了とし、以降の操作は実行しない
6. 既存 reconnect シナリオへの組み込みや新シナリオ種別の追加は本 issue では行わず、`ScenarioOp` への操作追加のみをスコープとする。C++ 版の `add_reconnect_scenario` 相当 (duration > 0 かつ repeat_interval = 0 経路の Exit) のシナリオ化は本 issue のスコープ外とし、既存の `run()` 側処理を維持する。動作確認は実サーバー (Sora SFU) への接続による手動確認で行う (モック・スタブ利用不可のため)。確認時のみ既存シナリオへの一時的な組み込みを許容する

## 完了条件

- シナリオ定義に Exit 操作を含められる
- Exit に到達したクライアントは切断され、再接続しない
- Exit を含むシナリオの全クライアントが Exit した後、プロセスが正常終了コード 0 で終了する (実サーバー接続での手動確認。プロセス終了は既存の JoinSet 完了チェーンによる)

## 解決方法

`src/scenario.rs` の `ScenarioOp` に `Exit` を追加し、`ScenarioPlayer::run_until_disconnect()` の戻り値を `ScenarioEnd::{Reconnect, Exit}` に変更した。`ScenarioEnd` は後続の Reconnect 操作対応と整合する形 (Reconnect 操作も `ScenarioEnd::Reconnect` を返す予定) で設計した。

- `src/scenario.rs`: `Exit` 操作は実行ループを完了として `ScenarioEnd::Exit` を返し、op_index は進めない (vc タスク終了でプレイヤーが破棄されるため)。キャンセル時も vc タスクの終了を意図する `ScenarioEnd::Exit` を返す (呼び出し元の biased select が token.cancelled() を優先するため通常は Shutdown 経路が選択される。競合で Exit 経路が選ばれても安全な動作になる fail-safe 設計)
- `src/virtual_client.rs`: `DisconnectReason` に `ScenarioExit` を追加し、`ScenarioEnd::Exit` を `ScenarioExit` にマップ。`ScenarioExit` 分岐は ScenarioDisconnect と同じく切断処理 (handle.disconnect() 待機 → connection_token.cancel() → StatsEvent::Disconnected 送信) を行い、再接続せずループ break で vc タスクを終了する。プロセス全体の token は cancel しない (他 vc が Exit 未到達のまま Shutdown で終了し、「全クライアントが Exit した後」の完了条件を満たせなくなるため)。プロセス終了は既存の JoinSet 完了チェーンに任せる
- 既存 reconnect シナリオへの組み込みは設計方針どおりスコープ外とし、`Exit` バリアントには `#[cfg_attr(not(test), expect(dead_code))]` を付けた (組み込み時に expect を外す旨は enum doc に集約)

追加したテスト (`src/scenario.rs`。実サーバー接続での手動確認は Exit 後のプロセス終了を含む完了条件の確認に使用):

- `test_run_until_disconnect_exit_op`: Exit 操作で `ScenarioEnd::Exit` が返り、op_index が進まないこと (SoraConnectionHandle は実サーバー接続なしで build() できるため、モックなしで実行分岐を検証可能)
- `test_run_until_disconnect_disconnect_op`: Disconnect 操作で `ScenarioEnd::Reconnect` が返り、op_index が進むこと (再接続時に続きの操作から再開される仕組みの検証)
- `test_run_until_disconnect_cancelled_returns_exit`: キャンセル済みトークンで `ScenarioEnd::Exit` が返ること (fail-safe 設計の検証)
- `test_advance_wraps_to_loop_index`: op_index の進行と loop_index への折返し (loop_index=0/1 の両ケース)
