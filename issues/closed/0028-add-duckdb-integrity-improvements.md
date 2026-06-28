# DuckDB 統計書き込み層の整合性を改善する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-06-28
- Model: DeepSeek V4 Pro
- Branch: feature/add-duckdb-integrity-improvements
- Polished: 2026-06-28

## 目的

DuckDB 統計書き込み層 (`src/duckdb_stats.rs`) に存在する複数のデータ整合性・安全性の問題をまとめて修正する。本 issue は 7 件の独立した修正を含む複合 issue であり、同一ファイルに対する変更のため 1 ブランチで対応する。

## 優先度根拠

いずれも負荷試験中の統計データ欠落・破損・検知不能に繋がる問題であり、負荷試験ツールの信頼性に直結する。

## 現状

### 1. INSERT 失敗後の Connection 不整合

`src/duckdb_stats.rs:255-264`: `dispatch_command` が `Err` を返しても警告ログだけで継続する。I/O エラーやディスクフル発生時に後続の全 INSERT が失敗し続けるが、外部から検知不能。

### 2. reporter_loop が停止不能

`src/duckdb_stats.rs:169,307-324`: `reporter_loop` は `tokio::spawn` で起動されるが、JoinHandle 未保存、CancellationToken 未伝達、break 条件なし。shutdown 時にタスクを停止できない。

### 3. zakuro テーブルに PRIMARY KEY 制約がない

`src/duckdb_schema.sql:23-34`: 1 行レコード前提だが DDL に制約がない。複数行が誤挿入された場合、`UPDATE zakuro SET stop_timestamp = ?` (:682) が全行を更新する。

### 4. 複合インデックスに instance_id が欠落

`src/duckdb_schema.sql:320-328`: 全 9 個の複合インデックスが `(channel_id, connection_id, timestamp)` で `instance_id` を含まない。instance 単位集計 (`WHERE instance_id = ?`) で全テーブルフルスキャンになる。

### 5. send() が channel closed エラーを握りつぶす

`src/duckdb_stats.rs:245`: `let _ = tx.send(cmd).await` で送信失敗を無視。shutdown 時に `UpdateZakuroStop` の送信失敗で `stop_timestamp` が欠落する。

### 6. system_time_to_duck の expect パニック

`src/duckdb_stats.rs:654`: `SystemTime::duration_since(UNIX_EPOCH).expect(...)` がシステム時計 1970 年以前でパニック。防御的プログラミングとして `unwrap_or(Duration::ZERO)` に変更すべき。

### 7. type キャストの安全性

- `src/duckdb_stats.rs:655`: `as_micros() as i64` で `u128` → `i64` の silent truncation
- `instance_id as i32` が 9 箇所、`vc_id as i32` が 1 箇所（計 10 箇所）

## 設計方針

| # | 問題 | 修正方針 |
|---|------|---------|
| 1 | INSERT 失敗時に検知不能 | 連続エラー回数が閾値 (例: 10 回) を超えたら writer を停止し、エラーログを出力する |
| 2 | reporter_loop 停止不能 | `CancellationToken` を渡し停止可能にする。`DuckDBStatsWriter` の shutdown シーケンスで reporter の完了も待つ |
| 3 | zakuro テーブルに制約なし | `id INTEGER PRIMARY KEY DEFAULT 0 CHECK (id = 0)` を追加し 1 行制約を表明する |
| 4 | 複合インデックスに instance_id の欠落 | 全インデックスの先頭に `instance_id` を追加し `(instance_id, channel_id, connection_id, timestamp)` とする |
| 5 | send() エラーを握りつぶす | 送信失敗時に `rtc_log_warning!` を出力する |
| 6 | expect パニック | `unwrap_or(Duration::ZERO)` に変更する |
| 7 | as キャストの truncation | `as` キャストを `i64::try_from(...).unwrap_or(0)` に変更する。DuckDB スキーマ側の型変更は行わない |

## 完了条件

- 上記 7 件すべてが修正されていること
- 既存の DuckDB テストが通過すること
- 新規に以下のテストを追加すること:
  - INSERT 連続エラーによる writer 停止のテスト
  - `CancellationToken` による `reporter_loop` 停止のテスト
  - `send()` 失敗時のログ出力確認テスト
  - 境界値 (`UNIX_EPOCH` 以前の時刻、`i64::MAX` 超の値) のテスト
- writer task の shutdown シーケンスが正常に動作すること

## 解決方法

7 件の修正を実施した。

### 1. INSERT 連続エラーによる writer 停止

`writer_run_loop` に連続エラーカウンタを追加し、`MAX_CONSECUTIVE_ERRORS = 10` を超えたら writer を停止する。

### 2. reporter_loop 停止可能化

`reporter_loop` に `CancellationToken` を渡し、shutdown 時に停止できるようにした。`DuckDBStatsWriter` に `reporter_handle` と `reporter_token` を追加し、`join()` で reporter の完了を待つ。

### 3. zakuro テーブル PRIMARY KEY 制約

`id INTEGER PRIMARY KEY DEFAULT 0 CHECK (id = 0)` を追加し、1 行制約を表明した。

### 4. 複合インデックスに instance_id 追加

全 7 個の複合インデックスの先頭に `instance_id` を追加した。

### 5. send() エラー検知

`send()` の `let _ = tx.send(cmd).await` を `if let Err(e) = ... { rtc_log_warning!(...) }` に変更した。

### 6. system_time_to_duck の expect 除去

`.expect(...)` を `.unwrap_or(Duration::ZERO)` に変更した。

### 7. as i32 キャストの安全化

10 箇所の `as i32` キャストを `i32::try_from(...).unwrap_or(0)` に変更した。

### テスト追加

- `reporter_loop_stops_on_cancellation`: CancellationToken による停止
- `send_logs_error_on_closed_channel`: channel closed 時のエラーログ
- `writer_run_loop_stops_after_consecutive_errors`: 連続エラーによる writer 停止
- `system_time_to_duck_handles_pre_epoch`: UNIX_EPOCH 以前の時刻で micros=0
- `instance_id_as_i32_handles_overflow`: u32::MAX で try_from が Err を返す

### 変更ファイル

- `src/duckdb_stats.rs`
- `src/duckdb_schema.sql`

### 0030 との依存関係

本 issue は `issues/0030-refactor-duckdb-stats-split`（`duckdb_stats.rs` のファイル分割）よりも先に適用すること。ファイル分割の前に整合性修正を完了させることで、分割時のマージ競合を回避する。
