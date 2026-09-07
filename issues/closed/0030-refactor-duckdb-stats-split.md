# duckdb_stats.rs を責務単位でファイル分割する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-06-28
- Branch: feature/refactor-duckdb-stats-split
- Polished: 2026-06-28

## 目的

2,191 行に肥大化した `src/duckdb_stats.rs` を責務単位で複数ファイルに分割し、保守性と可読性を向上させる。

## 優先度根拠

単一ファイルに DDL、writer 管理、Row 構造体 (10 個)、INSERT/UPDATE 実装 (11 関数)、SQL 定数 (6 個)、RTCStats JSON パース (7 関数)、config_json 構築、ファイル名生成、offer パース、テストの責務が詰め込まれており、コードの見通しが著しく悪い。新規機能追加やバグ修正の度に巨大なファイル全体を読み直す必要がある。

## 現状

`src/duckdb_stats.rs` の構成:

- 行 1-59: モジュールドキュメント、`CHANNEL_CAPACITY` 定数、`UNKNOWN_TYPES` 管理
- 行 61-70: DDL 定数 (`SCHEMA_SQL`)
- 行 72-248: `DuckDBWriterConfig` / `DuckDBStatsWriter` / `DuckDBClient` (定義と impl)
- 行 250-304: `writer_run_loop`、`dispatch_command`
- 行 306-324: `reporter_loop`
- 行 326-350: `WriteCommand` 列挙型、`ConnectionIds` 構造体
- 行 352-644: Row 構造体 (10 個: `InsertZakuroRow`、`InsertZakuroScenarioRow`、`InsertConnectionRow`、`RtcStatsCodecRow`、`RtcStatsInboundRtpRow`、`RtcStatsOutboundRtpRow`、`RtcStatsMediaSourceRow`、`RtcStatsRemoteInboundRtpRow`、`RtcStatsRemoteOutboundRtpRow`、`RtcStatsDataChannelRow`)
- 行 646-1029: INSERT/UPDATE 実装 (`insert_zakuro`、`update_zakuro_stop`、`insert_zakuro_scenario`、`insert_connection`、`insert_rtc_stats_codec`、`insert_rtc_stats_inbound_rtp`、`insert_rtc_stats_outbound_rtp`、`insert_rtc_stats_media_source`、`insert_rtc_stats_remote_inbound_rtp`、`insert_rtc_stats_remote_outbound_rtp`、`insert_rtc_stats_data_channel`)、`system_time_to_duck` ヘルパー
- 行 1031-1096: INSERT SQL 文字列定数 (6 個)
- 行 1098-1112: `generate_filename`
- 行 1114-1148: `parse_offer_ids`
- 行 1150-1559: `dispatch_stats`、`StatsCommon`、JSON パース関数 (7 個: `parse_codec`、`parse_inbound_rtp`、`parse_outbound_rtp`、`parse_media_source`、`parse_remote_inbound_rtp`、`parse_remote_outbound_rtp`、`parse_data_channel`)、ヘルパー (`get_i64`、`get_f64`、`get_string`、`get_bool`、`get_i16`)
- 行 1561-1749: `build_config_json`、`common_json`、`instance_json`、`MaskedJson`
- 行 1751-2191: テスト (スキーマ生成、RTCStats JSON 振り分け、zakuro shutdown、config_json マスク、parse_offer_ids、ファイル名生成)

## 設計方針

以下のディレクトリ構成に分割する。`src/duckdb_stats.rs` をディレクトリ `src/duckdb_stats/` に置き換え、元の `duckdb_stats.rs` は `mod.rs` として再エクスポート専用にする。`main.rs` の `mod duckdb_stats;` 宣言は変更不要。

```
src/
  duckdb_stats/
    mod.rs             — mod 宣言 + pub(crate) use 再エクスポートのみ
    writer.rs          — DuckDBStatsWriter、DuckDBClient、writer_run_loop、dispatch_command、reporter_loop
    schema.rs          — SCHEMA_SQL、INSERT SQL 文字列定数 (6 個)
    rows.rs            — Row 構造体 (10 個)、WriteCommand、ConnectionIds、INSERT/UPDATE 実装 (11 関数)、system_time_to_duck
    stats_json.rs      — dispatch_stats、StatsCommon、JSON パース (7 関数)、ヘルパー (get_i64/get_f64/get_string/get_bool/get_i16)、config_json 構築 (build_config_json/common_json/instance_json/MaskedJson)、parse_offer_ids、generate_filename
    module.rs          — CHANNEL_CAPACITY 定数、UNKNOWN_TYPES 管理 (全局 static + アクセッサ + test ヘルパー)
```

ファイル分割の詳細:

- `mod.rs`: 全サブモジュールを `mod` で宣言し、外部から必要な全アイテムを `pub(crate) use` で再エクスポートする。モジュールドキュメントもここに移動する。
- `writer.rs`:
  - `DuckDBWriterConfig` 構造体 (行 76-85)
  - `DuckDBStatsWriter` 構造体と impl (行 87-200)
  - `DuckDBClient` 構造体と impl (行 202-248)
  - `writer_run_loop` (行 250-264)
  - `dispatch_command` (行 266-304)
  - `reporter_loop` (行 306-324)
- `schema.rs`:
  - `SCHEMA_SQL` (行 70) — `include_str!` のパスは `concat!(env!("CARGO_MANIFEST_DIR"), "/src/duckdb_schema.sql")` に変更する
  - INSERT SQL 文字列定数 (行 1031-1096)
- `rows.rs`:
  - `ConnectionIds` 構造体 (行 330-335)
  - `WriteCommand` 列挙型 (行 337-350)
  - Row 構造体 10 個 (行 352-644)
  - `system_time_to_duck` (行 650-657)
  - INSERT/UPDATE 実装 11 関数 (行 659-1029)
- `stats_json.rs`:
  - `dispatch_stats` (行 1154-1251)
  - `StatsCommon` (行 1253-1263)
  - ヘルパー 5 関数 (行 1265-1301)
  - JSON パース 7 関数 (行 1303-1559)
  - `MaskedJson` (行 1570-1576)
  - `build_config_json` / `common_json` / `instance_json` (行 1578-1749)
  - `parse_offer_ids` (行 1118-1148)
  - `generate_filename` (行 1101-1112)
- `module.rs`:
  - `CHANNEL_CAPACITY` 定数 (行 33)
  - `UNKNOWN_TYPES` global static (行 37-41)
  - テスト用ヘルパー関数 (行 43-59)

テストは各サブモジュールの末尾 (`#[cfg(test)] mod tests`) に分散配置する。`setup_db()` など複数ファイルで共有が必要なテストヘルパーは、各ファイルに重複定義するか、`#[cfg(test)]` 内で `super::schema::SCHEMA_SQL` を参照する形式に変更する。

### 0028 との依存関係

本 issue は `issues/0028-add-duckdb-integrity-improvements`（DuckDB 統計書き込み層の整合性改善）が完了した後に適用すること。0028 が先に `src/duckdb_stats.rs` に修正を加え、0030 がその修正後のファイルを分割する順序とする。逆順で進めるとマージ競合が発生する。

## 完了条件

- 責務単位でファイルが分割されていること
- `cargo build`、`cargo test`、`cargo clippy --workspace` が通過すること
- `main.rs` の既存の `use crate::duckdb_stats::*;` が新しい再エクスポート経由で変更なく動作すること
- 内部でクロスモジュール呼び出しが必要な関数 (`system_time_to_duck`、`dispatch_command` が呼び出す INSERT 関数群、ヘルパー関数群) が適切に `pub(crate)` 化されていること
- `git diff --stat` で実質的なコード変更が最小限であること（可視性修飾子の追加は許容、関数本体の変更は禁止）

## 解決方法

1. `src/duckdb_stats.rs` を `git mv src/duckdb_stats.rs src/duckdb_stats/mod.rs` で移動する
2. `src/duckdb_stats/` ディレクトリを作成する
3. `mod.rs` から各サブモジュールにコードを切り出す (Edit で移動)
4. `mod.rs` には以下のみを残す:
   - `mod writer;` / `mod schema;` / `mod rows;` / `mod stats_json;` / `mod module;`
   - モジュールドキュメント
   - 必要な全アイテムの `pub(crate) use` 再エクスポート
5. クロスモジュール参照が必要な関数に `pub(crate)` を追加する
6. `include_str!` を絶対パスに変更する
7. テストを各サブモジュール末尾に移動する
