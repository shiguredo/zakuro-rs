// ============================================================================
// Row 構造体
// ============================================================================

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use duckdb::types::{TimeUnit, Value as DuckValue};
use duckdb::{Connection, ToSql};

use super::schema::{
    INSERT_DATA_CHANNEL_SQL, INSERT_INBOUND_RTP_SQL, INSERT_MEDIA_SOURCE_SQL,
    INSERT_OUTBOUND_RTP_SQL, INSERT_REMOTE_INBOUND_RTP_SQL, INSERT_REMOTE_OUTBOUND_RTP_SQL,
};

/// `type:offer` メッセージから抽出した接続識別子
#[derive(Debug, Clone)]
pub(crate) struct ConnectionIds {
    pub(crate) connection_id: String,
    pub(crate) session_id: String,
}

/// writer task へ送るコマンド
pub(crate) enum WriteCommand {
    InsertZakuro(Box<InsertZakuroRow>),
    UpdateZakuroStop { stop_timestamp: SystemTime },
    InsertZakuroScenario(Box<InsertZakuroScenarioRow>),
    InsertConnection(Box<InsertConnectionRow>),
    InsertRtcStatsCodec(Box<RtcStatsCodecRow>),
    InsertRtcStatsInboundRtp(Box<RtcStatsInboundRtpRow>),
    InsertRtcStatsOutboundRtp(Box<RtcStatsOutboundRtpRow>),
    InsertRtcStatsMediaSource(Box<RtcStatsMediaSourceRow>),
    InsertRtcStatsRemoteInboundRtp(Box<RtcStatsRemoteInboundRtpRow>),
    InsertRtcStatsRemoteOutboundRtp(Box<RtcStatsRemoteOutboundRtpRow>),
    InsertRtcStatsDataChannel(Box<RtcStatsDataChannelRow>),
}

/// `zakuro` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct InsertZakuroRow {
    pub(crate) version: String,
    pub(crate) sora_sdk_version: Option<String>,
    pub(crate) webrtc_version: Option<String>,
    pub(crate) openh264_version: Option<String>,
    pub(crate) duckdb_version: Option<String>,
    pub(crate) environment: String,
    pub(crate) config_mode: String,
    pub(crate) config_json: String,
    pub(crate) start_timestamp: SystemTime,
}

/// `zakuro_scenario` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct InsertZakuroScenarioRow {
    pub(crate) instance_id: u32,
    pub(crate) vcs: u32,
    pub(crate) duration: Option<f64>,
    pub(crate) repeat_interval: Option<f64>,
    pub(crate) max_retry: u32,
    pub(crate) retry_interval: f64,
    pub(crate) sora_signaling_urls: Vec<String>,
    pub(crate) sora_channel_id: String,
    pub(crate) sora_role: String,
}

/// `connection` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct InsertConnectionRow {
    pub(crate) instance_id: u32,
    pub(crate) vc_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) connection_id: String,
    pub(crate) session_id: String,
    pub(crate) role: String,
    pub(crate) audio: bool,
    pub(crate) video: bool,
}

/// `rtc_stats_codec` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsCodecRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) payload_type: Option<i64>,
    pub(crate) clock_rate: Option<i64>,
    pub(crate) channels: Option<i64>,
    pub(crate) sdp_fmtp_line: Option<String>,
}

/// `rtc_stats_inbound_rtp` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsInboundRtpRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) ssrc: Option<i64>,
    pub(crate) kind: Option<String>,
    pub(crate) transport_id: Option<String>,
    pub(crate) codec_id: Option<String>,
    pub(crate) packets_received: Option<i64>,
    pub(crate) packets_lost: Option<i64>,
    pub(crate) bytes_received: Option<i64>,
    pub(crate) jitter: Option<f64>,
    pub(crate) packets_received_with_ect1: Option<i64>,
    pub(crate) packets_received_with_ce: Option<i64>,
    pub(crate) packets_reported_as_lost: Option<i64>,
    pub(crate) packets_reported_as_lost_but_recovered: Option<i64>,
    pub(crate) last_packet_received_timestamp: Option<f64>,
    pub(crate) header_bytes_received: Option<i64>,
    pub(crate) packets_discarded: Option<i64>,
    pub(crate) fec_bytes_received: Option<i64>,
    pub(crate) fec_packets_received: Option<i64>,
    pub(crate) fec_packets_discarded: Option<i64>,
    pub(crate) nack_count: Option<i64>,
    pub(crate) pli_count: Option<i64>,
    pub(crate) fir_count: Option<i64>,
    pub(crate) track_identifier: Option<String>,
    pub(crate) mid: Option<String>,
    pub(crate) remote_id: Option<String>,
    pub(crate) frames_decoded: Option<i64>,
    pub(crate) key_frames_decoded: Option<i64>,
    pub(crate) frames_rendered: Option<i64>,
    pub(crate) frames_dropped: Option<i64>,
    pub(crate) frame_width: Option<i64>,
    pub(crate) frame_height: Option<i64>,
    pub(crate) frames_per_second: Option<f64>,
    pub(crate) qp_sum: Option<i64>,
    pub(crate) total_decode_time: Option<f64>,
    pub(crate) total_inter_frame_delay: Option<f64>,
    pub(crate) total_squared_inter_frame_delay: Option<f64>,
    pub(crate) pause_count: Option<i64>,
    pub(crate) total_pauses_duration: Option<f64>,
    pub(crate) freeze_count: Option<i64>,
    pub(crate) total_freezes_duration: Option<f64>,
    pub(crate) total_processing_delay: Option<f64>,
    pub(crate) estimated_playout_timestamp: Option<f64>,
    pub(crate) jitter_buffer_delay: Option<f64>,
    pub(crate) jitter_buffer_target_delay: Option<f64>,
    pub(crate) jitter_buffer_emitted_count: Option<i64>,
    pub(crate) jitter_buffer_minimum_delay: Option<f64>,
    pub(crate) total_samples_received: Option<i64>,
    pub(crate) concealed_samples: Option<i64>,
    pub(crate) silent_concealed_samples: Option<i64>,
    pub(crate) concealment_events: Option<i64>,
    pub(crate) inserted_samples_for_deceleration: Option<i64>,
    pub(crate) removed_samples_for_acceleration: Option<i64>,
    pub(crate) audio_level: Option<f64>,
    pub(crate) total_audio_energy: Option<f64>,
    pub(crate) total_samples_duration: Option<f64>,
    pub(crate) frames_received: Option<i64>,
    pub(crate) decoder_implementation: Option<String>,
    pub(crate) playout_id: Option<String>,
    pub(crate) power_efficient_decoder: Option<bool>,
    pub(crate) frames_assembled_from_multiple_packets: Option<i64>,
    pub(crate) total_assembly_time: Option<f64>,
    pub(crate) retransmitted_packets_received: Option<i64>,
    pub(crate) retransmitted_bytes_received: Option<i64>,
    pub(crate) rtx_ssrc: Option<i64>,
    pub(crate) fec_ssrc: Option<i64>,
    pub(crate) total_corruption_probability: Option<f64>,
    pub(crate) total_squared_corruption_probability: Option<f64>,
    pub(crate) corruption_measurements: Option<i64>,
}

/// `rtc_stats_outbound_rtp` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsOutboundRtpRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) ssrc: Option<i64>,
    pub(crate) kind: Option<String>,
    pub(crate) transport_id: Option<String>,
    pub(crate) codec_id: Option<String>,
    pub(crate) packets_sent: Option<i64>,
    pub(crate) bytes_sent: Option<i64>,
    pub(crate) packets_sent_with_ect1: Option<i64>,
    pub(crate) mid: Option<String>,
    pub(crate) media_source_id: Option<String>,
    pub(crate) remote_id: Option<String>,
    pub(crate) rid: Option<String>,
    pub(crate) encoding_index: Option<i64>,
    pub(crate) header_bytes_sent: Option<i64>,
    pub(crate) retransmitted_packets_sent: Option<i64>,
    pub(crate) retransmitted_bytes_sent: Option<i64>,
    pub(crate) rtx_ssrc: Option<i64>,
    pub(crate) target_bitrate: Option<f64>,
    pub(crate) total_encoded_bytes_target: Option<i64>,
    pub(crate) frame_width: Option<i64>,
    pub(crate) frame_height: Option<i64>,
    pub(crate) frames_per_second: Option<f64>,
    pub(crate) frames_sent: Option<i64>,
    pub(crate) huge_frames_sent: Option<i64>,
    pub(crate) frames_encoded: Option<i64>,
    pub(crate) key_frames_encoded: Option<i64>,
    pub(crate) qp_sum: Option<i64>,
    pub(crate) total_encode_time: Option<f64>,
    pub(crate) total_packet_send_delay: Option<f64>,
    pub(crate) quality_limitation_reason: Option<String>,
    pub(crate) quality_limitation_duration_none: Option<f64>,
    pub(crate) quality_limitation_duration_cpu: Option<f64>,
    pub(crate) quality_limitation_duration_bandwidth: Option<f64>,
    pub(crate) quality_limitation_duration_other: Option<f64>,
    pub(crate) quality_limitation_resolution_changes: Option<i64>,
    pub(crate) nack_count: Option<i64>,
    pub(crate) pli_count: Option<i64>,
    pub(crate) fir_count: Option<i64>,
    pub(crate) encoder_implementation: Option<String>,
    pub(crate) power_efficient_encoder: Option<bool>,
    pub(crate) active: Option<bool>,
    pub(crate) scalability_mode: Option<String>,
}

/// `rtc_stats_media_source` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsMediaSourceRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) track_identifier: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) audio_level: Option<f64>,
    pub(crate) total_audio_energy: Option<f64>,
    pub(crate) total_samples_duration: Option<f64>,
    pub(crate) echo_return_loss: Option<f64>,
    pub(crate) echo_return_loss_enhancement: Option<f64>,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
    pub(crate) frames: Option<i64>,
    pub(crate) frames_per_second: Option<f64>,
}

/// `rtc_stats_remote_inbound_rtp` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsRemoteInboundRtpRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) ssrc: Option<i64>,
    pub(crate) kind: Option<String>,
    pub(crate) transport_id: Option<String>,
    pub(crate) codec_id: Option<String>,
    pub(crate) packets_received: Option<i64>,
    pub(crate) packets_received_with_ect1: Option<i64>,
    pub(crate) packets_received_with_ce: Option<i64>,
    pub(crate) packets_reported_as_lost: Option<i64>,
    pub(crate) packets_reported_as_lost_but_recovered: Option<i64>,
    pub(crate) packets_lost: Option<i64>,
    pub(crate) jitter: Option<f64>,
    pub(crate) local_id: Option<String>,
    pub(crate) round_trip_time: Option<f64>,
    pub(crate) total_round_trip_time: Option<f64>,
    pub(crate) fraction_lost: Option<f64>,
    pub(crate) round_trip_time_measurements: Option<i64>,
    pub(crate) packets_with_bleached_ect1_marking: Option<i64>,
}

/// `rtc_stats_remote_outbound_rtp` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsRemoteOutboundRtpRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) ssrc: Option<i64>,
    pub(crate) kind: Option<String>,
    pub(crate) transport_id: Option<String>,
    pub(crate) codec_id: Option<String>,
    pub(crate) packets_sent: Option<i64>,
    pub(crate) bytes_sent: Option<i64>,
    pub(crate) local_id: Option<String>,
    pub(crate) remote_timestamp: Option<f64>,
    pub(crate) reports_sent: Option<i64>,
    pub(crate) round_trip_time: Option<f64>,
    pub(crate) total_round_trip_time: Option<f64>,
    pub(crate) round_trip_time_measurements: Option<i64>,
}

/// `rtc_stats_data_channel` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsDataChannelRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) label: Option<String>,
    pub(crate) protocol: Option<String>,
    pub(crate) data_channel_identifier: Option<i16>,
    pub(crate) state: Option<String>,
    pub(crate) messages_sent: Option<i64>,
    pub(crate) bytes_sent: Option<i64>,
    pub(crate) messages_received: Option<i64>,
    pub(crate) bytes_received: Option<i64>,
}

// ============================================================================
// INSERT / UPDATE 実装
// ============================================================================

/// `SystemTime` を DuckDB `TIMESTAMP` (microsecond) に bind する形式のヘルパー
pub(crate) fn system_time_to_duck(ts: SystemTime) -> DuckValue {
    let micros = ts
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros() as i64;
    DuckValue::Timestamp(TimeUnit::Microsecond, micros)
}

pub(crate) fn insert_zakuro(conn: &Connection, row: InsertZakuroRow) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &row.version,
        &row.sora_sdk_version,
        &row.webrtc_version,
        &row.openh264_version,
        &row.duckdb_version,
        &row.environment,
        &row.config_mode,
        &row.config_json,
        &system_time_to_duck(row.start_timestamp),
    ];
    conn.execute(
        "INSERT INTO zakuro (version, sora_sdk_version, webrtc_version, openh264_version, \
         duckdb_version, environment, config_mode, config_json, start_timestamp) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params,
    )?;
    Ok(())
}

pub(crate) fn update_zakuro_stop(
    conn: &Connection,
    stop_timestamp: SystemTime,
) -> duckdb::Result<()> {
    conn.execute(
        "UPDATE zakuro SET stop_timestamp = ?",
        [&system_time_to_duck(stop_timestamp) as &dyn ToSql],
    )?;
    Ok(())
}

pub(crate) fn insert_zakuro_scenario(
    conn: &Connection,
    row: InsertZakuroScenarioRow,
) -> duckdb::Result<()> {
    let urls: Vec<DuckValue> = row
        .sora_signaling_urls
        .into_iter()
        .map(DuckValue::Text)
        .collect();
    let urls_value = DuckValue::List(urls);
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &(row.vcs as i64),
        &row.duration,
        &row.repeat_interval,
        &(row.max_retry as i64),
        &row.retry_interval,
        &urls_value,
        &row.sora_channel_id,
        &row.sora_role,
    ];
    conn.execute(
        "INSERT INTO zakuro_scenario (instance_id, vcs, duration, repeat_interval, \
         max_retry, retry_interval, sora_signaling_urls, sora_channel_id, sora_role) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params,
    )?;
    Ok(())
}

pub(crate) fn insert_connection(conn: &Connection, row: InsertConnectionRow) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &(i32::try_from(row.vc_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.connection_id,
        &row.session_id,
        &row.role,
        &row.audio,
        &row.video,
        // offer は WebSocket 経由で届くため必ず true
        &true,
        // offer 時点では DataChannel SCTP handshake 未完了のため false
        &false,
    ];
    conn.execute(
        "INSERT INTO connection (instance_id, vc_id, timestamp, channel_id, \
         connection_id, session_id, role, audio, video, websocket_connected, \
         datachannel_connected) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params,
    )?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_codec(
    conn: &Connection,
    row: RtcStatsCodecRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.mime_type,
        &row.payload_type,
        &row.clock_rate,
        &row.channels,
        &row.sdp_fmtp_line,
    ];
    conn.execute(
        "INSERT INTO rtc_stats_codec (instance_id, timestamp, channel_id, session_id, \
         connection_id, rtc_timestamp, type, id, mime_type, payload_type, clock_rate, \
         channels, sdp_fmtp_line) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (connection_id, id, mime_type, payload_type, clock_rate, channels, \
         sdp_fmtp_line) DO NOTHING",
        params,
    )?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_inbound_rtp(
    conn: &Connection,
    row: RtcStatsInboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.ssrc,
        &row.kind,
        &row.transport_id,
        &row.codec_id,
        &row.packets_received,
        &row.packets_lost,
        &row.bytes_received,
        &row.jitter,
        &row.packets_received_with_ect1,
        &row.packets_received_with_ce,
        &row.packets_reported_as_lost,
        &row.packets_reported_as_lost_but_recovered,
        &row.last_packet_received_timestamp,
        &row.header_bytes_received,
        &row.packets_discarded,
        &row.fec_bytes_received,
        &row.fec_packets_received,
        &row.fec_packets_discarded,
        &row.nack_count,
        &row.pli_count,
        &row.fir_count,
        &row.track_identifier,
        &row.mid,
        &row.remote_id,
        &row.frames_decoded,
        &row.key_frames_decoded,
        &row.frames_rendered,
        &row.frames_dropped,
        &row.frame_width,
        &row.frame_height,
        &row.frames_per_second,
        &row.qp_sum,
        &row.total_decode_time,
        &row.total_inter_frame_delay,
        &row.total_squared_inter_frame_delay,
        &row.pause_count,
        &row.total_pauses_duration,
        &row.freeze_count,
        &row.total_freezes_duration,
        &row.total_processing_delay,
        &row.estimated_playout_timestamp,
        &row.jitter_buffer_delay,
        &row.jitter_buffer_target_delay,
        &row.jitter_buffer_emitted_count,
        &row.jitter_buffer_minimum_delay,
        &row.total_samples_received,
        &row.concealed_samples,
        &row.silent_concealed_samples,
        &row.concealment_events,
        &row.inserted_samples_for_deceleration,
        &row.removed_samples_for_acceleration,
        &row.audio_level,
        &row.total_audio_energy,
        &row.total_samples_duration,
        &row.frames_received,
        &row.decoder_implementation,
        &row.playout_id,
        &row.power_efficient_decoder,
        &row.frames_assembled_from_multiple_packets,
        &row.total_assembly_time,
        &row.retransmitted_packets_received,
        &row.retransmitted_bytes_received,
        &row.rtx_ssrc,
        &row.fec_ssrc,
        &row.total_corruption_probability,
        &row.total_squared_corruption_probability,
        &row.corruption_measurements,
    ];
    conn.execute(INSERT_INBOUND_RTP_SQL, params)?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_outbound_rtp(
    conn: &Connection,
    row: RtcStatsOutboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.ssrc,
        &row.kind,
        &row.transport_id,
        &row.codec_id,
        &row.packets_sent,
        &row.bytes_sent,
        &row.packets_sent_with_ect1,
        &row.mid,
        &row.media_source_id,
        &row.remote_id,
        &row.rid,
        &row.encoding_index,
        &row.header_bytes_sent,
        &row.retransmitted_packets_sent,
        &row.retransmitted_bytes_sent,
        &row.rtx_ssrc,
        &row.target_bitrate,
        &row.total_encoded_bytes_target,
        &row.frame_width,
        &row.frame_height,
        &row.frames_per_second,
        &row.frames_sent,
        &row.huge_frames_sent,
        &row.frames_encoded,
        &row.key_frames_encoded,
        &row.qp_sum,
        &row.total_encode_time,
        &row.total_packet_send_delay,
        &row.quality_limitation_reason,
        &row.quality_limitation_duration_none,
        &row.quality_limitation_duration_cpu,
        &row.quality_limitation_duration_bandwidth,
        &row.quality_limitation_duration_other,
        &row.quality_limitation_resolution_changes,
        &row.nack_count,
        &row.pli_count,
        &row.fir_count,
        &row.encoder_implementation,
        &row.power_efficient_encoder,
        &row.active,
        &row.scalability_mode,
    ];
    conn.execute(INSERT_OUTBOUND_RTP_SQL, params)?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_media_source(
    conn: &Connection,
    row: RtcStatsMediaSourceRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.track_identifier,
        &row.kind,
        &row.audio_level,
        &row.total_audio_energy,
        &row.total_samples_duration,
        &row.echo_return_loss,
        &row.echo_return_loss_enhancement,
        &row.width,
        &row.height,
        &row.frames,
        &row.frames_per_second,
    ];
    conn.execute(INSERT_MEDIA_SOURCE_SQL, params)?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_remote_inbound_rtp(
    conn: &Connection,
    row: RtcStatsRemoteInboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.ssrc,
        &row.kind,
        &row.transport_id,
        &row.codec_id,
        &row.packets_received,
        &row.packets_received_with_ect1,
        &row.packets_received_with_ce,
        &row.packets_reported_as_lost,
        &row.packets_reported_as_lost_but_recovered,
        &row.packets_lost,
        &row.jitter,
        &row.local_id,
        &row.round_trip_time,
        &row.total_round_trip_time,
        &row.fraction_lost,
        &row.round_trip_time_measurements,
        &row.packets_with_bleached_ect1_marking,
    ];
    conn.execute(INSERT_REMOTE_INBOUND_RTP_SQL, params)?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_remote_outbound_rtp(
    conn: &Connection,
    row: RtcStatsRemoteOutboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.ssrc,
        &row.kind,
        &row.transport_id,
        &row.codec_id,
        &row.packets_sent,
        &row.bytes_sent,
        &row.local_id,
        &row.remote_timestamp,
        &row.reports_sent,
        &row.round_trip_time,
        &row.total_round_trip_time,
        &row.round_trip_time_measurements,
    ];
    conn.execute(INSERT_REMOTE_OUTBOUND_RTP_SQL, params)?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_data_channel(
    conn: &Connection,
    row: RtcStatsDataChannelRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &system_time_to_duck(row.timestamp),
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.label,
        &row.protocol,
        &row.data_channel_identifier,
        &row.state,
        &row.messages_sent,
        &row.bytes_sent,
        &row.messages_received,
        &row.bytes_received,
    ];
    conn.execute(INSERT_DATA_CHANNEL_SQL, params)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;
    use duckdb::types::{TimeUnit, Value as DuckValue};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// 一時ディレクトリに `.db` ファイルを作りスキーマを投入するヘルパー
    fn setup_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("test.db");
        let conn = Connection::open(&path).expect("DuckDB open に失敗");
        conn.execute_batch(crate::duckdb_stats::schema::SCHEMA_SQL)
            .expect("スキーマ投入に失敗");
        (dir, conn)
    }

    #[test]
    fn rtc_stats_codec_unique_constraint_dedupes() {
        // 同一値で 2 回 INSERT しても 1 行だけ残る
        // (UNIQUE 制約の全列に NULL でない値を入れることで重複排除を検証)
        let (_dir, conn) = setup_db();
        let row = RtcStatsCodecRow {
            instance_id: 0,
            timestamp: SystemTime::now(),
            channel_id: "ch".into(),
            session_id: "s1".into(),
            connection_id: "c1".into(),
            rtc_timestamp: Some(1.0),
            stats_type: "codec".into(),
            id: "C1".into(),
            mime_type: Some("video/VP8".into()),
            payload_type: Some(96),
            clock_rate: Some(90000),
            channels: Some(2),
            sdp_fmtp_line: Some("profile-id=0".into()),
        };
        insert_rtc_stats_codec(&conn, row.clone()).expect("1 回目の INSERT 失敗");
        insert_rtc_stats_codec(&conn, row).expect("2 回目の INSERT 失敗");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM rtc_stats_codec", [], |row| row.get(0))
            .expect("カウント取得に失敗");
        assert_eq!(count, 1, "UNIQUE 制約で 1 行だけ残るべき");
    }

    #[test]
    fn zakuro_insert_and_update_stop_flow() {
        let (_dir, conn) = setup_db();
        let start = SystemTime::now();
        insert_zakuro(
            &conn,
            InsertZakuroRow {
                version: "1".into(),
                sora_sdk_version: None,
                webrtc_version: None,
                openh264_version: None,
                duckdb_version: Some("v1".into()),
                environment: "macos/arm64".into(),
                config_mode: "ARGS".into(),
                config_json: "{}".into(),
                start_timestamp: start,
            },
        )
        .expect("InsertZakuro 失敗");
        let stop = SystemTime::now();
        update_zakuro_stop(&conn, stop).expect("UpdateZakuroStop 失敗");
        let (s, e): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT start_timestamp, stop_timestamp FROM zakuro",
                [],
                |row| {
                    let s: duckdb::Result<i64> = row.get(0);
                    let e: duckdb::Result<i64> = row.get(1);
                    Ok((s.ok(), e.ok()))
                },
            )
            .expect("SELECT 失敗");
        assert!(s.is_some(), "start_timestamp は NOT NULL のべき");
        assert!(e.is_some(), "stop_timestamp は NOT NULL のべき");
    }

    #[test]
    fn system_time_to_duck_handles_pre_epoch() {
        let ts = UNIX_EPOCH
            .checked_sub(Duration::from_secs(3600))
            .expect("UNIX_EPOCH より前の時刻を作成できること");
        let val = system_time_to_duck(ts);
        match val {
            DuckValue::Timestamp(TimeUnit::Microsecond, micros) => {
                assert_eq!(
                    micros, 0,
                    "UNIX_EPOCH より前の時刻は micros=0 にマップされるべき"
                );
            }
            _ => panic!("Timestamp が返されるべき"),
        }
    }

    #[test]
    fn instance_id_as_i32_handles_overflow() {
        let overflow_value = u32::MAX;
        let result = i32::try_from(overflow_value);
        assert!(
            result.is_err(),
            "u32::MAX は i32 に収まらず try_from が Err を返すこと"
        );
        let safe: i32 = result.unwrap_or(0);
        assert_eq!(safe, 0, "オーバーフローハンドリングで 0 が使われること");
    }
}
