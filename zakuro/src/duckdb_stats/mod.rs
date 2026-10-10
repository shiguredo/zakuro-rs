//! DuckDB ファイルへの統計情報書き込み
//!
//! 1 プロセスにつき 1 つの DuckDB ファイル (`zakuro_YYYYMMDD_HHMMSS_mmm.db`) を生成し、
//! Zakuro の起動情報・シナリオ設定・接続情報・WebRTC RTCStats を定期的に保存する。
//!
//! 書き込みは VirtualClient (= 1 Sora connection) ごとに
//! `sora_sdk::SoraConnectionHandle::get_stats()` を `--duckdb-interval` 秒間隔で呼び、
//! 戻り JSON の `type` で振り分けて対応テーブルに入れる。
//!
//! 制御コマンド (起動情報、接続行、ライフサイクル、codec) は unbounded チャネルで欠落させない。
//! 統計サンプルは接続 1 本 × 1 tick を 1 メッセージにし、writer がテーブルごとの
//! Appender でバルク INSERT する。同じ接続の未書き込みサンプルは最新だけを残す。
//!
//! `Connection` は `Send` だが `!Sync` のため複数 task から共有できない。
//! そのため 1 つの `spawn_blocking` OS スレッド内で `Handle::current().block_on`
//! して mpsc 受信ループを回す。

pub(crate) mod module;
pub(crate) mod rows;
pub(crate) mod schema;
pub(crate) mod stats_json;
pub(crate) mod writer;

#[cfg(test)]
pub(crate) use module::clear_unknown_types_for_test;
pub(crate) use rows::{
    ConnectionIds, InsertConnectionLifecycleRow, InsertConnectionRow, InsertZakuroRow,
    InsertZakuroScenarioRow, WriteCommand,
};
pub(crate) use stats_json::{
    CodecIdentity, build_config_json, generate_filename, parse_offer_ids, parse_rtc_stats,
};
pub(crate) use writer::{DuckDBClient, DuckDBStatsWriter, DuckDBWriterConfig};
