//! DuckDB ファイルへの統計情報書き込み
//!
//! 1 プロセスにつき 1 つの DuckDB ファイル (`zakuro_YYYYMMDD_HHMMSS_mmm.db`) を生成し、
//! Zakuro の起動情報・シナリオ設定・接続情報・WebRTC RTCStats を定期的に保存する。
//!
//! 書き込みは VirtualClient (= 1 Sora connection) ごとに
//! `sora_sdk::SoraConnectionHandle::get_stats()` を `--duckdb-interval` 秒間隔で呼び、
//! 戻り JSON の `type` で振り分けて対応テーブルに INSERT する。
//!
//! `Connection` は `Send` だが `!Sync` のため複数 task から共有できない。
//! そのため 1 つの `spawn_blocking` OS スレッド内で `Handle::current().block_on`
//! して mpsc 受信ループを回し、VirtualClient 側からは `DuckDBClient::try_send` で
//! `Send` 可能な `WriteCommand` を投げる構成とする。

pub(crate) mod module;
pub(crate) mod rows;
pub(crate) mod schema;
pub(crate) mod stats_json;
pub(crate) mod writer;

#[cfg(test)]
pub(crate) use module::clear_unknown_types_for_test;
pub(crate) use rows::{
    ConnectionIds, InsertConnectionRow, InsertZakuroRow, InsertZakuroScenarioRow, WriteCommand,
};
pub(crate) use stats_json::{
    build_config_json, dispatch_stats, generate_filename, parse_offer_ids,
};
pub(crate) use writer::{DuckDBClient, DuckDBStatsWriter, DuckDBWriterConfig};
