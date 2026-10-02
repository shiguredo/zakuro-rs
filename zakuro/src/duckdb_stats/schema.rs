// ============================================================================
// DDL
// ============================================================================

/// シーケンス 8 個 + テーブル 10 個 + インデックス 9 個を 1 発で投入する DDL
///
/// `BEGIN; ... COMMIT;` で囲むことで `Connection::execute_batch` 1 回で投入する。
/// `instance_id INTEGER` 列を各 stats テーブルの `pk` 列の直後に挿入する
/// (C++ 版 zakuro の DDL に Rust 版固有の差分を加えた正本)。
pub(crate) const SCHEMA_SQL: &str = include_str!("../duckdb_schema.sql");

// ============================================================================
// INSERT SQL 文字列定数 (列数が多いので定数に切り出し)
// ============================================================================

pub(crate) const INSERT_INBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_inbound_rtp (instance_id, \
  timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, ssrc, kind, \
  transport_id, codec_id, packets_received, packets_lost, bytes_received, jitter, \
  packets_received_with_ect1, packets_received_with_ce, packets_reported_as_lost, \
  packets_reported_as_lost_but_recovered, last_packet_received_timestamp, \
  header_bytes_received, packets_discarded, fec_bytes_received, fec_packets_received, \
  fec_packets_discarded, nack_count, pli_count, fir_count, track_identifier, mid, \
  remote_id, frames_decoded, key_frames_decoded, frames_rendered, frames_dropped, \
  frame_width, frame_height, frames_per_second, qp_sum, total_decode_time, \
  total_inter_frame_delay, total_squared_inter_frame_delay, pause_count, \
  total_pauses_duration, freeze_count, total_freezes_duration, total_processing_delay, \
  estimated_playout_timestamp, jitter_buffer_delay, jitter_buffer_target_delay, \
  jitter_buffer_emitted_count, jitter_buffer_minimum_delay, total_samples_received, \
  concealed_samples, silent_concealed_samples, concealment_events, \
  inserted_samples_for_deceleration, removed_samples_for_acceleration, audio_level, \
  total_audio_energy, total_samples_duration, frames_received, decoder_implementation, \
  playout_id, power_efficient_decoder, frames_assembled_from_multiple_packets, \
  total_assembly_time, retransmitted_packets_received, retransmitted_bytes_received, \
  rtx_ssrc, fec_ssrc, total_corruption_probability, total_squared_corruption_probability, \
  corruption_measurements) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_OUTBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_outbound_rtp (instance_id, \
  timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, ssrc, kind, \
  transport_id, codec_id, packets_sent, bytes_sent, packets_sent_with_ect1, mid, \
  media_source_id, remote_id, rid, encoding_index, header_bytes_sent, \
  retransmitted_packets_sent, retransmitted_bytes_sent, rtx_ssrc, target_bitrate, \
  total_encoded_bytes_target, frame_width, frame_height, frames_per_second, frames_sent, \
  huge_frames_sent, frames_encoded, key_frames_encoded, qp_sum, total_encode_time, \
  total_packet_send_delay, quality_limitation_reason, quality_limitation_duration_none, \
  quality_limitation_duration_cpu, quality_limitation_duration_bandwidth, \
  quality_limitation_duration_other, quality_limitation_resolution_changes, nack_count, \
  pli_count, fir_count, encoder_implementation, power_efficient_encoder, active, \
  scalability_mode) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_MEDIA_SOURCE_SQL: &str = "INSERT INTO rtc_stats_media_source (instance_id, \
  timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, \
  track_identifier, kind, audio_level, total_audio_energy, total_samples_duration, \
  echo_return_loss, echo_return_loss_enhancement, width, height, frames, \
  frames_per_second) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_REMOTE_INBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_remote_inbound_rtp \
  (instance_id, timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, \
  ssrc, kind, transport_id, codec_id, packets_received, packets_received_with_ect1, \
  packets_received_with_ce, packets_reported_as_lost, \
  packets_reported_as_lost_but_recovered, packets_lost, jitter, local_id, round_trip_time, \
  total_round_trip_time, fraction_lost, round_trip_time_measurements, \
  packets_with_bleached_ect1_marking) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_REMOTE_OUTBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_remote_outbound_rtp \
  (instance_id, timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, \
  ssrc, kind, transport_id, codec_id, packets_sent, bytes_sent, local_id, remote_timestamp, \
  reports_sent, round_trip_time, total_round_trip_time, round_trip_time_measurements) \
  VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_DATA_CHANNEL_SQL: &str = "INSERT INTO rtc_stats_data_channel (instance_id, \
  timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, label, \
  protocol, data_channel_identifier, state, messages_sent, bytes_sent, messages_received, \
  bytes_received) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

#[cfg(test)]
mod tests {
    use duckdb::Connection;

    /// 一時ディレクトリに `.db` ファイルを作りスキーマを投入するヘルパー
    fn setup_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("test.db");
        let conn = Connection::open(&path).expect("DuckDB open に失敗");
        conn.execute_batch(super::SCHEMA_SQL)
            .expect("スキーマ投入に失敗");
        (dir, conn)
    }

    #[test]
    fn schema_creates_ten_tables() {
        let (_dir, conn) = setup_db();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM duckdb_tables() WHERE schema_name='main'",
                [],
                |row| row.get(0),
            )
            .expect("テーブル数の取得に失敗");
        assert_eq!(count, 10, "テーブル数は 10 であるべき");
    }

    #[test]
    fn schema_creates_eight_sequences() {
        let (_dir, conn) = setup_db();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM duckdb_sequences() WHERE schema_name='main'",
                [],
                |row| row.get(0),
            )
            .expect("シーケンス数の取得に失敗");
        assert_eq!(count, 8, "シーケンス数は 8 であるべき");
    }

    #[test]
    fn schema_creates_nine_indexes() {
        let (_dir, conn) = setup_db();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM duckdb_indexes() WHERE schema_name='main'",
                [],
                |row| row.get(0),
            )
            .expect("インデックス数の取得に失敗");
        assert_eq!(count, 9, "インデックス数は 9 であるべき");
    }

    #[test]
    fn connection_table_has_instance_id_as_second_column() {
        let (_dir, conn) = setup_db();
        let mut stmt = conn
            .prepare("PRAGMA table_info('connection')")
            .expect("table_info の準備に失敗");
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .expect("table_info のクエリに失敗");
        let mut cols: Vec<(i64, String)> = rows.map(|r| r.expect("row get 失敗")).collect();
        cols.sort_by_key(|(i, _)| *i);
        // cid=0 は pk、cid=1 は instance_id
        assert_eq!(cols[0].1, "pk", "1 列目は pk であるべき");
        assert_eq!(cols[1].1, "instance_id", "2 列目は instance_id であるべき");
    }
}
