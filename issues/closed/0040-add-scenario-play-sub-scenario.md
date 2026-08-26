# シナリオ操作 PlaySubScenario を追加する

- Created: 2026-08-03
- Completed: 2026-08-03
- Branch: feature/add-scenario-play-sub-scenario
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の ScenarioPlayer は `OpPlaySubScenario` を持ち、名前付きサブシナリオをメインシナリオから起動する (`zakuro/src/scenario_player.h` の `OP_PLAY_SUB_SCENARIO` 処理)。`zakuro/src/zakuro.cpp` では DataChannel 連続送信をラベルごとに「scenario-dcs-{label}」というサブシナリオとして、数字音声を「scenario-voice-number-client」というサブシナリオとしてメインシナリオに組み込んでいる。サブシナリオは非同期に起動され、メインシナリオは即座に次の操作へ進む (並行実行)。シナリオ定義の分割・再利用と、C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、シナリオは 1 つの ops ベクターを op_index で直列に実行するだけであり、サブシナリオの概念がない
- DataChannel 連続送信は `src/data_channel.rs` の `run_messaging()` タスクで実現済み (C++ 版は dcs サブシナリオで実現)。数字音声はシナリオ操作 PlayVoiceNumberClient で対応する想定だったが、後に非対応とした
- C++ 版の `OpPlaySubScenario` は `name` / `data` (サブシナリオの ScenarioData) / `loop_op_index` を持ち、サブシナリオは独立したループ開始位置を持つ

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `PlaySubScenario { name: String, data: Scenario, loop_op_index: usize }` を追加する
2. C++ 版と同じく「サブシナリオを並行実行し、メインシナリオは続行する」ことを基本とする。サブシナリオの実行状態はシナリオプレイヤー内に保持する方式と、接続単位のタスクとして spawn する方式のどちらかを、キャンセルと再接続ライフサイクルとの整合を考慮して実装時に決定する
3. サブシナリオの実行は接続ごとに開始され、切断時に停止し、再接続後に再開される
4. dcs サブシナリオ化 (C++ 版の scenario-dcs-{label} 相当) は本 issue のスコープ外とし、既存の `run_messaging()` を維持する。サブシナリオ機構そのものの追加がゴール

## 完了条件

- シナリオに PlaySubScenario 操作が定義できる
- サブシナリオの実行中もメインシナリオの実行が継続する
- サブシナリオはループ開始位置 (loop_op_index) を指定できる
- 切断・再接続時にサブシナリオの停止・再開が正しく行われる

## 解決方法

磨き上げ時の必要性判断で「不要」(確信度: 中) と判定され、反対尋問でも維持されたため、ユーザー承認のうえ closed にした。

判定根拠 (陳腐化):

- zakuro-rs ではサブシナリオ機構の実用例が存在しない。DataChannel 連続送信は `src/data_channel.rs` の `run_messaging()` が実現済み (本 issue 自身がスコープ外と宣言)、数字音声はシナリオ操作 PlayVoiceNumberClient で対応する想定だったが後に非対応とした
- CLI のシナリオは `--scenario reconnect` のみで、PlaySubScenario を含むシナリオ種別が選択できないため、機構を追加してもデッドコードになる
- C++ 版との機能互換性は、0016 (closed) の判断と同じく「観測可能な機能を別実装で実現」で満たされる (並行実行は tokio タスクで実現済み)

サブシナリオ機構の実用例 (dcs サブシナリオ化やシナリオ定義の外部化) が生まれた時点で再起票する。
