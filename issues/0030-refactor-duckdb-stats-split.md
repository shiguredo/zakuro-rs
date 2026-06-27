# duckdb_stats.rs を責務単位でファイル分割する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/refactor-duckdb-stats-split
- Polished: 2026-00-00

## 目的

2,191 行に肥大化した `src/duckdb_stats.rs` を責務単位で複数ファイルに分割し、保守性と可読性を向上させる。

## 優先度根拠

単一ファイルに 10 の責務（DDL、writer 管理、Row 構造体 9 個、INSERT 実装 9 関数、SQL 定数 6 個、RTCStats JSON パース 7 関数、config_json 構築、ファイル名生成、offer パース、テスト）が詰め込まれており、コードの見通しが著しく悪い。新規機能追加やバグ修正の度に巨大なファイル全体を読み直す必要がある。

## 現状

`src/duckdb_stats.rs` の構成:

- 行 1-60: モジュールドキュメント、定数、unknown_types 管理
- 行 61-70: DDL 定数
- 行 72-248: DuckDBWriterConfig / DuckDBStatsWriter / DuckDBClient
- 行 250-324: writer_run_loop、dispatch_command、reporter_loop
- 行 326-350: WriteCommand 列挙型
- 行 352-644: Row 構造体 (9 個)
- 行 646-1029: INSERT/UPDATE 実装 (9 関数)
- 行 1031-1096: INSERT SQL 文字列定数 (6 個)
- 行 1098-1112: ファイル名生成
- 行 1114-1148: parse_offer_ids
- 行 1150-1559: dispatch_stats、StatsCommon、JSON パース (7 関数)、ヘルパー (get_i64/get_f64/get_string/get_bool/get_i16)
- 行 1561-1749: config_json 構築
- 行 1751-2191: テスト

## 設計方針

以下のファイル構成に分割する:

- `src/duckdb_stats.rs` — モジュール再エクスポートのみ
- `src/duckdb_writer.rs` — DuckDBStatsWriter、DuckDBClient、writer_run_loop、reporter_loop
- `src/duckdb_schema.rs` — DDL 定数 (SCHEMA_SQL)、INSERT SQL 定数
- `src/duckdb_rows.rs` — Row 構造体、WriteCommand、INSERT/UPDATE 実装
- `src/duckdb_stats_json.rs` — dispatch_stats、StatsCommon、JSON パース、ヘルパー、config_json 構築、parse_offer_ids、generate_filename
- `src/duckdb_stats/tests.rs` または既存の `#[cfg(test)]` を分割後のファイルに分散

## 完了条件

- 責務単位でファイルが分割されていること
- `cargo build` と `cargo test` が通過すること
- 公開 API (`pub(crate)`) が変わっていないこと
- `git diff --stat` で削除行と追加行の合計が最小限であること（実質的なコード変更なし）

## 解決方法

各責務を新しいファイルに移動し、`pub(crate) use` で再エクスポートする。`main.rs` の `mod duckdb_stats;` 宣言は変更不要とする（lib.rs 相当の main.rs でサブモジュールとして読み込む）。
