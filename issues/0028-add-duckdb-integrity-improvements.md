# DuckDB 統計書き込み層の整合性を改善する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/add-duckdb-integrity-improvements
- Polished: 2026-00-00

## 目的

DuckDB 統計書き込み層 (`src/duckdb_stats.rs`) に存在する複数のデータ整合性・安全性の問題をまとめて修正する。

## 優先度根拠

いずれも負荷試験中の統計データ欠落・破損・検知不能に繋がる問題であり、負荷試験ツールの信頼性に直結する。

## 現状

以下の 8 件の問題が存在する:

### 1. init 失敗時の cmd_rx 未 drop

`src/duckdb_stats.rs:133-139`: `Connection::open` が失敗した場合、`return` で早期リターンするが `cmd_rx` が drop されない。全 sender が永久に `Full` エラーになり続ける。

### 2. INSERT 失敗後の Connection 不整合

`src/duckdb_stats.rs:260-263`: `dispatch_command` が `Err` を返しても警告ログだけで継続する。Connection が不正状態になった場合、後続の全 INSERT が失敗し続けるが外部から検知不能。

### 3. reporter_loop が停止不能

`src/duckdb_stats.rs:169,307-324`: `reporter_loop` は `tokio::spawn` で起動されるが、JoinHandle 未保存、CancellationToken 未伝達、break 条件なし。タスクが停止不能。

### 4. zakuro テーブルに PRIMARY KEY / UNIQUE 制約がない

`src/duckdb_schema.sql:23-34`: 1 行レコード前提だが DDL に制約がない。複数行が誤挿入された場合、`UPDATE zakuro SET stop_timestamp = ?` が全行を更新する。

### 5. 複合インデックスに instance_id が欠落

`src/duckdb_schema.sql:320-328`: 全 9 個の複合インデックスが `(channel_id, connection_id, timestamp)` で `instance_id` を含まない。instance 単位集計で全テーブルフルスキャンになる。

### 6. send() が channel closed エラーを握りつぶす

`src/duckdb_stats.rs:245`: `let _ = tx.send(cmd).await` で送信失敗を無視。shutdown 時に writer が死んでいる場合、`stop_timestamp` が欠落する。

### 7. system_time_to_duck の expect パニック

`src/duckdb_stats.rs:654`: `SystemTime::duration_since(UNIX_EPOCH).expect(...)` がシステム時計 1970 年以前でパニック。

### 8. type キャストの安全性

`src/duckdb_stats.rs:655`: `as_micros() as i64` で `u128` → `i64` キャスト。`src/duckdb_stats.rs:696,717,742,etc`: `instance_id as i32` / `vc_id as i32` が 8 箇所。

## 設計方針

1. init 失敗時は `cmd_rx` を明示的に drop する
2. INSERT 失敗が一定数を超えたら writer を停止する
3. `reporter_loop` に CancellationToken を渡して停止可能にする
4. `zakuro` テーブルに PRIMARY KEY または CHECK 制約を追加する
5. 全複合インデックスに `instance_id` を追加する
6. `send()` の戻り値をチェックし、失敗時に警告する
7. `expect` を `unwrap_or(Duration::ZERO)` に変更する
8. `as` キャストを `try_into` に変更するか、DuckDB スキーマ側の型を `UINTEGER` に変更する

## 完了条件

- 上記 8 件すべてが修正されていること
- 既存の DuckDB テストが通過すること
- writer task の shutdown シーケンスが正常に動作すること

## 解決方法

各項目に記載の修正を `src/duckdb_stats.rs` および `src/duckdb_schema.sql` に適用する。
