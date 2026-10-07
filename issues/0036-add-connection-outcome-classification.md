# 接続単位の outcome と失敗理由を記録する

- Created: 2026-10-07
- Completed: 2026-10-07
- Branch: feature/add-connection-outcome-classification
- Polished: {YYYY-MM-DD}

## 目的

負荷試験の結果を機械判定できるようにする。その第一段として、接続 1 本ごとに成功 / 失敗の別と
失敗理由を記録する。

現状は確立時刻とメディアの送受信を記録しているだけで、それを解釈する層が無い。記録した材料から
「どの接続がなぜ失敗したか」を数えられないため、試験結果を人が読むしかない。

## 現状

- `zakuro/src/connection_lifecycle.rs` の `ConnectionLifecycle` は、確立時刻、映像 / 音声の
  初回送受信、SFU からのレポート到着、統計サンプル数、最後に観測した各状態、終了理由
  (`LifecycleEnd`) を保持する
- `zakuro/src/duckdb_schema.sql` の `connection_lifecycle` テーブルに、成功 / 失敗の別を表す列は無い
- 成功の定義は「接続が確立し、メディアが流れていることを観測できること」であり、ロールによって
  必要な観測が異なる
  - `sendonly`: 送信と SFU からのレポート到着
  - `recvonly`: 受信
  - `sendrecv`: 送信、SFU からのレポート到着、受信
- `zakuro/src/virtual_client.rs` の `audio_value` / `video_value` が映像 / 音声の有効 / 無効を
  判定している。無効にした種別の観測は求められない
- 統計サンプルが 0 の接続 (確立後すぐ終了して統計を 1 度も取れなかった場合) は、送受信の有無を
  判定できない
- 確立直後はメディアが流れ始めるまでの猶予が要る。猶予の間に終了した接続を「送信が無い」と
  判定すると、起動が遅いだけの接続を失敗に数えてしまう

## 設計方針

- `ConnectionLifecycle` から outcome を求める判定を追加する。入力はロール、映像 / 音声の
  有効 / 無効、観測結果とし、判定は時刻に依存させない (単体テストで組み合わせを検証できるようにする)
- 成功 / 失敗の別と失敗理由を別の型にして、失敗理由は優先順に 1 つだけ付ける
  - `build-failed`: 接続を開始できなかった
  - `connect-failed`: 確立できなかった
  - `no-media-sent`: 確立したが、送るはずの種別のメディアが流れなかった
  - `no-delivery-report`: 送信はあったが SFU からのレポートが届かなかった
  - `no-media-received`: 確立したが、受けるはずの種別のメディアが届かなかった
  - `unexpected-disconnect`: メディアの観測は満たしたが、想定外の切断が起きた
- 確立から 10 秒 (定数) 未満で終了した接続と、統計サンプルが 0 の接続は `unjudged` として
  成功にも失敗にも数えない
- 動いていたメディアが止まった状態は `stalled` として、成功 / 失敗とは別の軸で記録する。
  途中で止まった接続を失敗に混ぜると、成功接続率の意味が読めなくなるため
- 判定結果は `connection_lifecycle` テーブルの列として書く。集計としきい値判定は後続で扱う

## 完了条件

- `connection_lifecycle` テーブルに outcome、失敗理由、`stalled`、連続して増加が観測されなかった
  サンプル数が記録されること
- ロールと映像 / 音声の有効 / 無効、確立の有無、観測結果の組み合わせごとに、成功 / 失敗 /
  判定不能が期待どおりになること (単体テスト)
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること

## 解決方法

- `zakuro/src/connection_lifecycle.rs` に `ConnectionOutcome` (成功 / 失敗 / 判定不能) と
  `ConnectionFailure` (失敗理由)、判定の入力になる `OutcomeSettings` を追加し、
  `ConnectionLifecycle::judge` で判定する。判定は記録済みの時刻だけで行うため、
  時刻に依存せず単体テストで組み合わせを検証できる
- 成功述語はロールと映像 / 音声の有効 / 無効で決める。`expects_send` なら有効な種別すべての
  送信と SFU からのレポート到着、`expects_receive` なら有効な種別すべての受信を求める
- 判定不能は、統計サンプルが 0 のとき、映像も音声も無効なとき、確立から猶予
  (`OUTCOME_GRACE`、10 秒) 未満で終了したときとした
- `ConnectionLifecycle` に `last_media_activity_at` と `max_idle_samples` を追加し、
  `zakuro/src/media_observer.rs` がサンプルごとにメディアの増加を記録する。
  `is_stalled` は成功 / 失敗とは別の軸として `STALL_SAMPLES` (3) 回連続で増加が無い場合に true を返す
- 映像 / 音声の有効判定は `is_video_enabled` / `is_audio_enabled` にまとめ、SDP の組み立てと
  判定が食い違わないようにした
- `connection_lifecycle` テーブルに `outcome` / `failure_reason` / `stalled` /
  `max_idle_samples` / `last_media_activity_at` を追加し、接続終了時に 1 行として記録する
- `docs/DUCKDB.md` に列の意味と集計クエリを追記した
- 単体テストとして、構築失敗 / 確立失敗 / 成功 / 送信欠け / レポート欠け / 受信欠け /
  想定外の切断 / サンプル 0 / 猶予内の終了 / 映像無効の場合 / メディア無効の場合 /
  停止の検出と非検出、統計サンプルからの停止検出を追加した
