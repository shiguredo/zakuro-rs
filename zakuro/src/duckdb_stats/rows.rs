// ============================================================================
// Row 構造体
// ============================================================================

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use duckdb::types::{TimeUnit, Value as DuckValue};
use duckdb::{Appender, Connection, ToSql};

/// `type:offer` メッセージから抽出した接続識別子
#[derive(Debug, Clone)]
pub(crate) struct ConnectionIds {
    pub(crate) connection_id: String,
    pub(crate) session_id: String,
}

/// writer の制御チャネルへ送るコマンド
///
/// 件数は接続数に比例し、試験時間には比例しない。統計サンプルとは別チャネルで、
/// 満杯による欠落を起こさない。
pub(crate) enum WriteCommand {
    InsertZakuro(Box<InsertZakuroRow>),
    UpdateZakuroStop { stop_timestamp: SystemTime },
    InsertZakuroScenario(Box<InsertZakuroScenarioRow>),
    InsertConnection(Box<InsertConnectionRow>),
    InsertConnectionLifecycle(Box<InsertConnectionLifecycleRow>),
    InsertRtcStatsCodec(Box<RtcStatsCodecRow>),
    InsertRtcStatsLocalCandidate(Box<RtcStatsLocalCandidateRow>),
    InsertRtcStatsRemoteCandidate(Box<RtcStatsRemoteCandidateRow>),
    InsertRtcStatsCertificate(Box<RtcStatsCertificateRow>),
}

/// 接続 1 本の 1 回の `get_stats` から切り出した RTC 統計行
///
/// codec と ICE 候補そのものは含まない。それらは内容が変わらないため、
/// 接続ごとに 1 回だけ制御チャネルへ送る。
pub(crate) struct StatsSample {
    pub(crate) instance_id: u32,
    pub(crate) vc_id: u32,
    pub(crate) inbound: Vec<RtcStatsInboundRtpRow>,
    pub(crate) outbound: Vec<RtcStatsOutboundRtpRow>,
    pub(crate) media_source: Vec<RtcStatsMediaSourceRow>,
    pub(crate) remote_inbound: Vec<RtcStatsRemoteInboundRtpRow>,
    pub(crate) remote_outbound: Vec<RtcStatsRemoteOutboundRtpRow>,
    pub(crate) data_channel: Vec<RtcStatsDataChannelRow>,
    pub(crate) transport: Vec<RtcStatsTransportRow>,
    pub(crate) candidate_pair: Vec<RtcStatsCandidatePairRow>,
    pub(crate) peer_connection: Vec<RtcStatsPeerConnectionRow>,
    pub(crate) media_playout: Vec<RtcStatsMediaPlayoutRow>,
}

impl StatsSample {
    /// 行を持たないサンプルを作る
    pub(crate) fn empty(instance_id: u32, vc_id: u32) -> Self {
        Self {
            instance_id,
            vc_id,
            inbound: Vec::new(),
            outbound: Vec::new(),
            media_source: Vec::new(),
            remote_inbound: Vec::new(),
            remote_outbound: Vec::new(),
            data_channel: Vec::new(),
            transport: Vec::new(),
            candidate_pair: Vec::new(),
            peer_connection: Vec::new(),
            media_playout: Vec::new(),
        }
    }

    /// テーブルへ書く行が 1 つでもあるか
    pub(crate) fn has_rows(&self) -> bool {
        self.row_count() > 0
    }

    /// このサンプルに含まれる統計行数
    pub(crate) fn row_count(&self) -> usize {
        self.inbound.len()
            + self.outbound.len()
            + self.media_source.len()
            + self.remote_inbound.len()
            + self.remote_outbound.len()
            + self.data_channel.len()
            + self.transport.len()
            + self.candidate_pair.len()
            + self.peer_connection.len()
            + self.media_playout.len()
    }
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

/// `connection_lifecycle` テーブルへの 1 行
///
/// 接続 1 本につき 1 行を、接続が終了した時点で書く。接続の構築に失敗した場合は
/// connection_id / session_id が無いまま 1 行を書く (試行そのものは記録に残す)。
#[derive(Clone)]
pub(crate) struct InsertConnectionLifecycleRow {
    pub(crate) instance_id: u32,
    pub(crate) vc_id: u32,
    pub(crate) channel_id: String,
    pub(crate) role: String,
    pub(crate) connection_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) attempt_started_at: SystemTime,
    pub(crate) offer_received_at: Option<SystemTime>,
    pub(crate) webrtc_connected_at: Option<SystemTime>,
    pub(crate) ice_connected_at: Option<SystemTime>,
    pub(crate) ice_gathering_complete_at: Option<SystemTime>,
    pub(crate) first_video_sent_at: Option<SystemTime>,
    pub(crate) first_video_received_at: Option<SystemTime>,
    pub(crate) first_audio_sent_at: Option<SystemTime>,
    pub(crate) first_audio_received_at: Option<SystemTime>,
    pub(crate) first_delivery_report_at: Option<SystemTime>,
    pub(crate) samples: u32,
    pub(crate) last_media_activity_at: Option<SystemTime>,
    pub(crate) max_idle_samples: u32,
    pub(crate) disconnected_at: SystemTime,
    pub(crate) peer_connection_state: Option<&'static str>,
    pub(crate) ice_connection_state: Option<&'static str>,
    pub(crate) ice_gathering_state: Option<&'static str>,
    pub(crate) signaling_state: Option<&'static str>,
    pub(crate) end_reason: &'static str,
    /// 接続の判定結果 (`success` / `failure` / `unjudged`)
    pub(crate) outcome: &'static str,
    /// 失敗理由 (成功と判定不能の場合は NULL)
    pub(crate) failure_reason: Option<&'static str>,
    /// メディアが止まった状態か
    pub(crate) stalled: bool,
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
    pub(crate) transport_id: Option<String>,
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

/// `rtc_stats_transport` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsTransportRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) packets_sent: Option<i64>,
    pub(crate) packets_received: Option<i64>,
    pub(crate) bytes_sent: Option<i64>,
    pub(crate) bytes_received: Option<i64>,
    pub(crate) ice_role: Option<String>,
    pub(crate) ice_state: Option<String>,
    pub(crate) dtls_state: Option<String>,
    pub(crate) dtls_role: Option<String>,
    pub(crate) selected_candidate_pair_id: Option<String>,
    pub(crate) selected_candidate_pair_changes: Option<i64>,
    pub(crate) local_certificate_id: Option<String>,
    pub(crate) remote_certificate_id: Option<String>,
    pub(crate) tls_version: Option<String>,
    pub(crate) dtls_cipher: Option<String>,
    pub(crate) srtp_cipher: Option<String>,
}

/// `rtc_stats_candidate_pair` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsCandidatePairRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) transport_id: Option<String>,
    pub(crate) local_candidate_id: Option<String>,
    pub(crate) remote_candidate_id: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) nominated: Option<bool>,
    pub(crate) packets_sent: Option<i64>,
    pub(crate) packets_received: Option<i64>,
    pub(crate) bytes_sent: Option<i64>,
    pub(crate) bytes_received: Option<i64>,
    pub(crate) current_round_trip_time: Option<f64>,
    pub(crate) total_round_trip_time: Option<f64>,
    pub(crate) available_outgoing_bitrate: Option<f64>,
    pub(crate) available_incoming_bitrate: Option<f64>,
    pub(crate) requests_sent: Option<i64>,
    pub(crate) requests_received: Option<i64>,
    pub(crate) responses_sent: Option<i64>,
    pub(crate) responses_received: Option<i64>,
    pub(crate) consent_requests_sent: Option<i64>,
    pub(crate) packets_discarded_on_send: Option<i64>,
    pub(crate) bytes_discarded_on_send: Option<i64>,
}

/// `rtc_stats_local_candidate` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsLocalCandidateRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) transport_id: Option<String>,
    pub(crate) address: Option<String>,
    pub(crate) port: Option<i64>,
    pub(crate) protocol: Option<String>,
    pub(crate) candidate_type: Option<String>,
    pub(crate) relay_protocol: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) network_type: Option<String>,
    pub(crate) priority: Option<i64>,
    pub(crate) foundation: Option<String>,
    pub(crate) tcp_type: Option<String>,
}

/// `rtc_stats_remote_candidate` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsRemoteCandidateRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) transport_id: Option<String>,
    pub(crate) address: Option<String>,
    pub(crate) port: Option<i64>,
    pub(crate) protocol: Option<String>,
    pub(crate) candidate_type: Option<String>,
    pub(crate) priority: Option<i64>,
    pub(crate) foundation: Option<String>,
    pub(crate) tcp_type: Option<String>,
}

/// `rtc_stats_peer_connection` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsPeerConnectionRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) data_channels_opened: Option<i64>,
    pub(crate) data_channels_closed: Option<i64>,
}

/// `rtc_stats_media_playout` テーブルへの 1 行
#[derive(Clone)]
pub(crate) struct RtcStatsMediaPlayoutRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) kind: Option<String>,
    pub(crate) synthesized_samples_duration: Option<f64>,
    pub(crate) synthesized_samples_events: Option<i64>,
    pub(crate) total_samples_duration: Option<f64>,
    pub(crate) total_playout_delay: Option<f64>,
    pub(crate) total_samples_count: Option<i64>,
}

/// `rtc_stats_certificate` テーブルへの 1 行
///
/// 証明書本体は残さない。fingerprint で DTLS 証明書を識別する。
#[derive(Clone)]
pub(crate) struct RtcStatsCertificateRow {
    pub(crate) instance_id: u32,
    pub(crate) timestamp: SystemTime,
    pub(crate) channel_id: String,
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) rtc_timestamp: Option<f64>,
    pub(crate) stats_type: String,
    pub(crate) id: String,
    pub(crate) fingerprint: Option<String>,
    pub(crate) fingerprint_algorithm: Option<String>,
    pub(crate) issuer_certificate_id: Option<String>,
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
    // duckdb-rs は List パラメータのバインドに対応していない
    // (`binding List parameters is not yet supported` で失敗する)。
    // JSON 配列の文字列として渡し、SQL 側で VARCHAR[] へ CAST する。
    // エスケープを自前で書かないよう JSON の生成には nojson を使う。
    let urls_json = nojson::json(|f| {
        f.array(|f| {
            for url in &row.sora_signaling_urls {
                f.element(url.as_str())?;
            }
            Ok(())
        })
    })
    .to_string();
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &(row.vcs as i64),
        &row.duration,
        &row.repeat_interval,
        &(row.max_retry as i64),
        &row.retry_interval,
        &urls_json,
        &row.sora_channel_id,
        &row.sora_role,
    ];
    conn.execute(
        "INSERT INTO zakuro_scenario (instance_id, vcs, duration, repeat_interval, \
         max_retry, retry_interval, sora_signaling_urls, sora_channel_id, sora_role) \
         VALUES (?, ?, ?, ?, ?, ?, CAST(? AS VARCHAR[]), ?, ?)",
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

pub(crate) fn insert_connection_lifecycle(
    conn: &Connection,
    row: InsertConnectionLifecycleRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(i32::try_from(row.instance_id).unwrap_or(0)),
        &(i32::try_from(row.vc_id).unwrap_or(0)),
        &row.channel_id,
        &row.role,
        &row.connection_id,
        &row.session_id,
        &system_time_to_duck(row.attempt_started_at),
        &row.offer_received_at.map(system_time_to_duck),
        &row.webrtc_connected_at.map(system_time_to_duck),
        &row.ice_connected_at.map(system_time_to_duck),
        &row.ice_gathering_complete_at.map(system_time_to_duck),
        &row.first_video_sent_at.map(system_time_to_duck),
        &row.first_video_received_at.map(system_time_to_duck),
        &row.first_audio_sent_at.map(system_time_to_duck),
        &row.first_audio_received_at.map(system_time_to_duck),
        &row.first_delivery_report_at.map(system_time_to_duck),
        &(i32::try_from(row.samples).unwrap_or(i32::MAX)),
        &row.last_media_activity_at.map(system_time_to_duck),
        &(i32::try_from(row.max_idle_samples).unwrap_or(i32::MAX)),
        &system_time_to_duck(row.disconnected_at),
        &row.peer_connection_state,
        &row.ice_connection_state,
        &row.ice_gathering_state,
        &row.signaling_state,
        &row.end_reason,
        &row.outcome,
        &row.failure_reason,
        &row.stalled,
    ];
    conn.execute(
        "INSERT INTO connection_lifecycle (instance_id, vc_id, channel_id, role, \
         connection_id, session_id, attempt_started_at, offer_received_at, \
         webrtc_connected_at, ice_connected_at, ice_gathering_complete_at, \
         first_video_sent_at, first_video_received_at, first_audio_sent_at, \
         first_audio_received_at, first_delivery_report_at, samples, \
         last_media_activity_at, max_idle_samples, disconnected_at, \
         peer_connection_state, ice_connection_state, ice_gathering_state, \
         signaling_state, end_reason, outcome, failure_reason, stalled) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        &row.transport_id,
    ];
    conn.execute(
        "INSERT INTO rtc_stats_codec (instance_id, timestamp, channel_id, session_id, \
         connection_id, rtc_timestamp, type, id, mime_type, payload_type, clock_rate, \
         channels, sdp_fmtp_line, transport_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (connection_id, id, mime_type, payload_type, clock_rate, channels, \
         sdp_fmtp_line) DO NOTHING",
        params,
    )?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_local_candidate(
    conn: &Connection,
    row: RtcStatsLocalCandidateRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.transport_id,
        &row.address,
        &row.port,
        &row.protocol,
        &row.candidate_type,
        &row.relay_protocol,
        &row.url,
        &row.network_type,
        &row.priority,
        &row.foundation,
        &row.tcp_type,
    ];
    conn.execute(
        "INSERT INTO rtc_stats_local_candidate (instance_id, timestamp, channel_id, \
         session_id, connection_id, rtc_timestamp, type, id, transport_id, address, port, \
         protocol, candidate_type, relay_protocol, url, network_type, priority, \
         foundation, tcp_type) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (connection_id, id) DO NOTHING",
        params,
    )?;
    Ok(())
}

pub(crate) fn insert_rtc_stats_remote_candidate(
    conn: &Connection,
    row: RtcStatsRemoteCandidateRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.transport_id,
        &row.address,
        &row.port,
        &row.protocol,
        &row.candidate_type,
        &row.priority,
        &row.foundation,
        &row.tcp_type,
    ];
    conn.execute(
        "INSERT INTO rtc_stats_remote_candidate (instance_id, timestamp, channel_id, \
         session_id, connection_id, rtc_timestamp, type, id, transport_id, address, port, \
         protocol, candidate_type, priority, foundation, tcp_type) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (connection_id, id) DO NOTHING",
        params,
    )?;
    Ok(())
}

pub(crate) fn append_rtc_stats_inbound_rtp(
    appender: &mut Appender<'_>,
    row: &RtcStatsInboundRtpRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
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
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_outbound_rtp(
    appender: &mut Appender<'_>,
    row: &RtcStatsOutboundRtpRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
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
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_media_source(
    appender: &mut Appender<'_>,
    row: &RtcStatsMediaSourceRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
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
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_remote_inbound_rtp(
    appender: &mut Appender<'_>,
    row: &RtcStatsRemoteInboundRtpRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
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
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_remote_outbound_rtp(
    appender: &mut Appender<'_>,
    row: &RtcStatsRemoteOutboundRtpRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
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
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_data_channel(
    appender: &mut Appender<'_>,
    row: &RtcStatsDataChannelRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
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
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_transport(
    appender: &mut Appender<'_>,
    row: &RtcStatsTransportRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.packets_sent,
        &row.packets_received,
        &row.bytes_sent,
        &row.bytes_received,
        &row.ice_role,
        &row.ice_state,
        &row.dtls_state,
        &row.dtls_role,
        &row.selected_candidate_pair_id,
        &row.selected_candidate_pair_changes,
        &row.local_certificate_id,
        &row.remote_certificate_id,
        &row.tls_version,
        &row.dtls_cipher,
        &row.srtp_cipher,
    ];
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_peer_connection(
    appender: &mut Appender<'_>,
    row: &RtcStatsPeerConnectionRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.data_channels_opened,
        &row.data_channels_closed,
    ];
    appender.append_row(params)
}

pub(crate) fn append_rtc_stats_media_playout(
    appender: &mut Appender<'_>,
    row: &RtcStatsMediaPlayoutRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.kind,
        &row.synthesized_samples_duration,
        &row.synthesized_samples_events,
        &row.total_samples_duration,
        &row.total_playout_delay,
        &row.total_samples_count,
    ];
    appender.append_row(params)
}

pub(crate) fn insert_rtc_stats_certificate(
    conn: &Connection,
    row: RtcStatsCertificateRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.fingerprint,
        &row.fingerprint_algorithm,
        &row.issuer_certificate_id,
    ];
    conn.execute(
        "INSERT INTO rtc_stats_certificate (instance_id, timestamp, channel_id, \
         session_id, connection_id, rtc_timestamp, type, id, fingerprint, \
         fingerprint_algorithm, issuer_certificate_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (connection_id, id) DO NOTHING",
        params,
    )?;
    Ok(())
}

pub(crate) fn append_rtc_stats_candidate_pair(
    appender: &mut Appender<'_>,
    row: &RtcStatsCandidatePairRow,
) -> duckdb::Result<()> {
    let instance_id = i32::try_from(row.instance_id).unwrap_or(0);
    let timestamp = system_time_to_duck(row.timestamp);
    let params: &[&dyn ToSql] = &[
        &instance_id,
        &timestamp,
        &row.channel_id,
        &row.session_id,
        &row.connection_id,
        &row.rtc_timestamp,
        &row.stats_type,
        &row.id,
        &row.transport_id,
        &row.local_candidate_id,
        &row.remote_candidate_id,
        &row.state,
        &row.nominated,
        &row.packets_sent,
        &row.packets_received,
        &row.bytes_sent,
        &row.bytes_received,
        &row.current_round_trip_time,
        &row.total_round_trip_time,
        &row.available_outgoing_bitrate,
        &row.available_incoming_bitrate,
        &row.requests_sent,
        &row.requests_received,
        &row.responses_sent,
        &row.responses_received,
        &row.consent_requests_sent,
        &row.packets_discarded_on_send,
        &row.bytes_discarded_on_send,
    ];
    appender.append_row(params)
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
            transport_id: None,
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
    fn insert_zakuro_scenario_stores_signaling_urls() {
        // 複数 URL と空配列の両方で zakuro_scenario へ 1 行入ること
        // (List パラメータのバインド非対応で INSERT が失敗していた回帰の検証)
        let (_dir, conn) = setup_db();
        insert_zakuro_scenario(
            &conn,
            InsertZakuroScenarioRow {
                instance_id: 0,
                vcs: 3,
                duration: Some(10.0),
                repeat_interval: None,
                max_retry: 1,
                retry_interval: 5.0,
                sora_signaling_urls: vec![
                    "wss://a.example.com/".into(),
                    "wss://b.example.com/".into(),
                ],
                sora_channel_id: "ch".into(),
                sora_role: "sendonly".into(),
            },
        )
        .expect("InsertZakuroScenario 失敗");
        let urls: String = conn
            .query_row(
                "SELECT CAST(sora_signaling_urls AS VARCHAR) FROM zakuro_scenario",
                [],
                |row| row.get(0),
            )
            .expect("SELECT 失敗");
        assert_eq!(
            urls, "['wss://a.example.com/', 'wss://b.example.com/']",
            "signaling URL の配列が保存されていない"
        );

        // 空配列も VARCHAR[] の空リストとして入ること
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM zakuro_scenario", [], |row| row.get(0))
            .expect("カウント取得に失敗");
        assert_eq!(row_count, 1, "1 行だけ入るべき");
    }

    #[test]
    fn insert_zakuro_scenario_accepts_empty_signaling_urls() {
        // signaling URL が空の行も入れられること (List バインド非対応の回帰検証)
        let (_dir, conn) = setup_db();
        insert_zakuro_scenario(
            &conn,
            InsertZakuroScenarioRow {
                instance_id: 0,
                vcs: 1,
                duration: None,
                repeat_interval: None,
                max_retry: 0,
                retry_interval: 60.0,
                sora_signaling_urls: Vec::new(),
                sora_channel_id: String::new(),
                sora_role: "sendonly".into(),
            },
        )
        .expect("InsertZakuroScenario 失敗");
        let urls: String = conn
            .query_row(
                "SELECT CAST(sora_signaling_urls AS VARCHAR) FROM zakuro_scenario",
                [],
                |row| row.get(0),
            )
            .expect("SELECT 失敗");
        assert_eq!(urls, "[]", "空配列が保存されていない");
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

    /// connection_lifecycle へ確立済みの接続を記録できること
    #[test]
    fn insert_connection_lifecycle_stores_observed_times() {
        let (_dir, conn) = setup_db();
        insert_connection_lifecycle(
            &conn,
            InsertConnectionLifecycleRow {
                instance_id: 0,
                vc_id: 3,
                channel_id: "ch".into(),
                role: "sendonly".into(),
                connection_id: Some("c1".into()),
                session_id: Some("s1".into()),
                attempt_started_at: UNIX_EPOCH + Duration::from_secs(100),
                offer_received_at: Some(UNIX_EPOCH + Duration::from_secs(101)),
                webrtc_connected_at: Some(UNIX_EPOCH + Duration::from_secs(102)),
                ice_connected_at: Some(UNIX_EPOCH + Duration::from_secs(102)),
                ice_gathering_complete_at: Some(UNIX_EPOCH + Duration::from_secs(103)),
                first_video_sent_at: Some(UNIX_EPOCH + Duration::from_secs(104)),
                first_video_received_at: None,
                first_audio_sent_at: Some(UNIX_EPOCH + Duration::from_secs(104)),
                first_audio_received_at: None,
                first_delivery_report_at: Some(UNIX_EPOCH + Duration::from_secs(105)),
                samples: 42,
                last_media_activity_at: Some(UNIX_EPOCH + Duration::from_secs(150)),
                max_idle_samples: 5,
                disconnected_at: UNIX_EPOCH + Duration::from_secs(160),
                peer_connection_state: Some("closed"),
                ice_connection_state: Some("completed"),
                ice_gathering_state: Some("complete"),
                signaling_state: Some("stable"),
                end_reason: "duration-expired",
                outcome: "success",
                failure_reason: None,
                stalled: true,
            },
        )
        .expect("INSERT に失敗");

        let (vc_id, role, connection_id, end_reason, outcome, stalled): (
            i32,
            String,
            String,
            String,
            String,
            bool,
        ) = conn
            .query_row(
                "SELECT vc_id, role, connection_id, end_reason, outcome, stalled \
                 FROM connection_lifecycle",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("SELECT に失敗");
        assert_eq!(vc_id, 3, "vc_id が保存されること");
        assert_eq!(role, "sendonly", "role が保存されること");
        assert_eq!(connection_id, "c1", "connection_id が保存されること");
        assert_eq!(end_reason, "duration-expired", "終了理由が保存されること");
        assert_eq!(outcome, "success", "判定結果が保存されること");
        assert!(stalled, "メディアが止まった状態が保存されること");

        let failure_reason: Option<String> = conn
            .query_row(
                "SELECT failure_reason FROM connection_lifecycle WHERE vc_id = 3",
                [],
                |row| row.get(0),
            )
            .expect("SELECT に失敗");
        assert_eq!(
            failure_reason, None,
            "成功した接続の失敗理由は NULL であること"
        );

        // 確立しなかった接続では時刻が NULL のまま残ること
        insert_connection_lifecycle(
            &conn,
            InsertConnectionLifecycleRow {
                instance_id: 0,
                vc_id: 4,
                channel_id: "ch".into(),
                role: "recvonly".into(),
                connection_id: None,
                session_id: None,
                attempt_started_at: UNIX_EPOCH + Duration::from_secs(200),
                offer_received_at: None,
                webrtc_connected_at: None,
                ice_connected_at: None,
                ice_gathering_complete_at: None,
                first_video_sent_at: None,
                first_video_received_at: None,
                first_audio_sent_at: None,
                first_audio_received_at: None,
                first_delivery_report_at: None,
                samples: 0,
                last_media_activity_at: None,
                max_idle_samples: 0,
                disconnected_at: UNIX_EPOCH + Duration::from_secs(201),
                peer_connection_state: None,
                ice_connection_state: None,
                ice_gathering_state: None,
                signaling_state: None,
                end_reason: "build-failed",
                outcome: "failure",
                failure_reason: Some("build-failed"),
                stalled: false,
            },
        )
        .expect("INSERT に失敗");

        let (nulls, failure_reason): (i64, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(failure_reason) FROM connection_lifecycle \
                 WHERE vc_id = 4 AND connection_id IS NULL AND webrtc_connected_at IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("SELECT に失敗");
        assert_eq!(nulls, 1, "未確立の接続は NULL として保存されること");
        assert_eq!(
            failure_reason,
            Some("build-failed".to_string()),
            "構築に失敗した理由が保存されること"
        );
    }
}
