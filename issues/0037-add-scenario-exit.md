# シナリオ操作 Exit を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-scenario-exit
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の ScenarioPlayer は `OpExit` を持ち、シナリオ実行中にクライアントを切断し、全クライアントの Exit 完了後にプロセスを終了する (`zakuro/src/scenario_player.h` の `OP_EXIT` 処理)。負荷試験の自動終了と、シナリオ定義による試験全体のライフサイクル制御のために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、`ScenarioPlayer::run_until_disconnect()` が Disconnect 到達時に呼び出し元へ制御を返す
- プロセス終了の経路は Ctrl+C (`tokio::signal::ctrl_c` → `CancellationToken`) と、`--duration` / `--repeat-interval` による VirtualClient 側の別処理のみ
- `issues/closed/0016-add-scenario-player.md` の解決方法で、OpExit は「duration / repeat_interval=0 経路。zakuro-rs では duration / repeat を VirtualClient 側で別処理」として未対応のまま残されている

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `Exit` を追加し、`ScenarioPlayer` の実行ループを Exit 到達時に完了として終了する (Disconnect と同じく呼び出し元に制御を返す形にする)
2. シナリオ実行ループの呼び出し元 (`src/virtual_client.rs`) は Exit 完了後に再接続せず、接続を閉じたままクライアントを終了させる
3. 全クライアントの終了は既存の CancellationToken によるグレースフルシャットダウンの仕組みを利用し、全 vc の終了を待ってプロセスを終了する (C++ 版の全クライアント exit 後に `io_context.stop()` に相当)
4. 既存 reconnect シナリオ (ループ前提) との整合は保ちつつ、Exit を含むシナリオ定義が書けるようにする

## 完了条件

- シナリオに Exit 操作が定義できる
- Exit に到達したクライアントは切断され、再接続しない
- 全クライアントが Exit した後、プロセスが正常終了コード 0 で終了する
