// ============================================================================
// ファイル名生成
// ============================================================================

use std::fmt;
use std::time::SystemTime;

use nojson::{DisplayJson, JsonFormatter, RawJsonOwned, RawJsonValue};
use shiguredo_webrtc::{log, rtc_log_warning};

use super::module::unknown_types;
use super::rows::{
    ConnectionIds, RtcStatsCodecRow, RtcStatsDataChannelRow, RtcStatsInboundRtpRow,
    RtcStatsMediaSourceRow, RtcStatsOutboundRtpRow, RtcStatsRemoteInboundRtpRow,
    RtcStatsRemoteOutboundRtpRow, WriteCommand,
};
use super::writer::DuckDBClient;

/// DuckDB ファイル名 `zakuro_{YYYYMMDD}_{HHMMSS}_{mmm}.db` を UTC で生成する
///
/// `jiff` クレートで UTC 暦時刻を整形する。`mmm` はミリ秒 3 桁。
pub(crate) fn generate_filename() -> String {
    let ts = jiff::Timestamp::now();
    let date = ts.strftime("%Y%m%d").to_string();
    let time = ts.strftime("%H%M%S").to_string();
    // ミリ秒部分は subsec 経由で取り出す
    let millis = ts.as_millisecond() % 1000;
    format!("zakuro_{date}_{time}_{millis:03}.db")
}

// ============================================================================
// offer メッセージからの connection_id / session_id 抽出
// ============================================================================

/// `type == "offer"` メッセージから connection_id / session_id を抽出する
///
/// `type != "offer"`、JSON 不正、いずれかのキー欠落、値が文字列以外の場合は `None`。
/// 既存 `src/data_channel.rs::parse_data_channels` の `to_member().required().try_into()`
/// シーケンスに準じたスタイルで実装する。
pub(crate) fn parse_offer_ids(text: &str) -> Option<ConnectionIds> {
    let raw = RawJsonOwned::parse(text).ok()?;
    let v = raw.value();
    let ty: String = v.to_member("type").ok()?.required().ok()?.try_into().ok()?;
    if ty != "offer" {
        return None;
    }
    let connection_id: String = v
        .to_member("connection_id")
        .ok()?
        .required()
        .ok()?
        .try_into()
        .ok()?;
    let session_id: String = v
        .to_member("session_id")
        .ok()?
        .required()
        .ok()?
        .try_into()
        .ok()?;
    Some(ConnectionIds {
        connection_id,
        session_id,
    })
}

// ============================================================================
// RTCStats JSON 振り分け (VirtualClient 側から呼ぶ)
// ============================================================================

/// get_stats の戻り JSON をパースして各 `WriteCommand` に振り分け、`try_send` で投げる
///
/// `instance_id` / `vc_id` / `channel_id` は呼び出し側 (= VirtualClient) の固定値。
/// `ids` は offer 受信時に確定した `connection_id` / `session_id`。
///
/// 戻り値は投入した stats エントリ数 (未対応 type は含まない)。
pub(crate) fn dispatch_stats(
    instance_id: u32,
    vc_id: u32,
    channel_id: &str,
    ids: &ConnectionIds,
    client: &DuckDBClient,
    stats_text: &str,
    now: SystemTime,
) -> usize {
    let Ok(json) = RawJsonOwned::parse(stats_text) else {
        rtc_log_warning!(
            "[i{}/vc-{}][duckdb] get_stats JSON parse failed",
            instance_id,
            vc_id
        );
        return 0;
    };
    // RTCStats は配列の形で返る (Sora SDK の get_stats 仕様)
    let Ok(arr) = json.value().to_array() else {
        rtc_log_warning!(
            "[i{}/vc-{}][duckdb] get_stats JSON is not an array",
            instance_id,
            vc_id
        );
        return 0;
    };

    let mut count: usize = 0;
    for element in arr {
        let ty: Option<String> = element
            .to_member("type")
            .ok()
            .and_then(|m| m.required().ok())
            .and_then(|v| v.try_into().ok());
        let Some(ty) = ty else { continue };
        let id: Option<String> = element
            .to_member("id")
            .ok()
            .and_then(|m| m.required().ok())
            .and_then(|v| v.try_into().ok());
        let id = id.unwrap_or_default();
        let rtc_timestamp: Option<f64> = element
            .to_member("timestamp")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|v| v.try_into().ok());

        let common = StatsCommon {
            instance_id,
            timestamp: now,
            channel_id: channel_id.to_string(),
            session_id: ids.session_id.clone(),
            connection_id: ids.connection_id.clone(),
            rtc_timestamp,
            stats_type: ty.clone(),
            id,
        };

        let cmd: Option<WriteCommand> = match ty.as_str() {
            "codec" => {
                parse_codec(element, common).map(|r| WriteCommand::InsertRtcStatsCodec(Box::new(r)))
            }
            "inbound-rtp" => parse_inbound_rtp(element, common)
                .map(|r| WriteCommand::InsertRtcStatsInboundRtp(Box::new(r))),
            "outbound-rtp" => parse_outbound_rtp(element, common)
                .map(|r| WriteCommand::InsertRtcStatsOutboundRtp(Box::new(r))),
            "media-source" => parse_media_source(element, common)
                .map(|r| WriteCommand::InsertRtcStatsMediaSource(Box::new(r))),
            "remote-inbound-rtp" => parse_remote_inbound_rtp(element, common)
                .map(|r| WriteCommand::InsertRtcStatsRemoteInboundRtp(Box::new(r))),
            "remote-outbound-rtp" => parse_remote_outbound_rtp(element, common)
                .map(|r| WriteCommand::InsertRtcStatsRemoteOutboundRtp(Box::new(r))),
            "data-channel" => parse_data_channel(element, common)
                .map(|r| WriteCommand::InsertRtcStatsDataChannel(Box::new(r))),
            other => {
                // 未知 type は初回のみ warn (抑制用集合で管理)
                let mut set = unknown_types()
                    .lock()
                    .expect("UNKNOWN_TYPES mutex poisoned");
                if set.insert(other.to_string()) {
                    rtc_log_warning!("[duckdb] unknown rtc stats type seen first time: {}", other);
                }
                None
            }
        };
        if let Some(c) = cmd {
            client.try_send(c);
            count += 1;
        }
    }
    count
}

/// 共通列の組み立て用ヘルパー
struct StatsCommon {
    instance_id: u32,
    timestamp: SystemTime,
    channel_id: String,
    session_id: String,
    connection_id: String,
    rtc_timestamp: Option<f64>,
    stats_type: String,
    id: String,
}

/// JSON 要素からメンバーを取り出すヘルパー (Option<i64>)
fn get_i64(v: RawJsonValue<'_, '_>, key: &str) -> Option<i64> {
    v.to_member(key)
        .ok()?
        .optional()
        .and_then(|val| val.try_into().ok())
}

/// JSON 要素からメンバーを取り出すヘルパー (Option<f64>)
fn get_f64(v: RawJsonValue<'_, '_>, key: &str) -> Option<f64> {
    v.to_member(key)
        .ok()?
        .optional()
        .and_then(|val| val.try_into().ok())
}

/// JSON 要素からメンバーを取り出すヘルパー (Option<String>)
fn get_string(v: RawJsonValue<'_, '_>, key: &str) -> Option<String> {
    v.to_member(key)
        .ok()?
        .optional()
        .and_then(|val| val.try_into().ok())
}

/// JSON 要素からメンバーを取り出すヘルパー (Option<bool>)
fn get_bool(v: RawJsonValue<'_, '_>, key: &str) -> Option<bool> {
    v.to_member(key)
        .ok()?
        .optional()
        .and_then(|val| val.try_into().ok())
}

/// JSON 要素からメンバーを取り出すヘルパー (Option<i16>)
fn get_i16(v: RawJsonValue<'_, '_>, key: &str) -> Option<i16> {
    let iv = get_i64(v, key)?;
    iv.try_into().ok()
}

fn parse_codec(v: RawJsonValue<'_, '_>, c: StatsCommon) -> Option<RtcStatsCodecRow> {
    Some(RtcStatsCodecRow {
        instance_id: c.instance_id,
        timestamp: c.timestamp,
        channel_id: c.channel_id,
        session_id: c.session_id,
        connection_id: c.connection_id,
        rtc_timestamp: c.rtc_timestamp,
        stats_type: c.stats_type,
        id: c.id,
        mime_type: get_string(v, "mimeType"),
        payload_type: get_i64(v, "payloadType"),
        clock_rate: get_i64(v, "clockRate"),
        channels: get_i64(v, "channels"),
        sdp_fmtp_line: get_string(v, "sdpFmtpLine"),
    })
}

fn parse_inbound_rtp(v: RawJsonValue<'_, '_>, c: StatsCommon) -> Option<RtcStatsInboundRtpRow> {
    Some(RtcStatsInboundRtpRow {
        instance_id: c.instance_id,
        timestamp: c.timestamp,
        channel_id: c.channel_id,
        session_id: c.session_id,
        connection_id: c.connection_id,
        rtc_timestamp: c.rtc_timestamp,
        stats_type: c.stats_type,
        id: c.id,
        ssrc: get_i64(v, "ssrc"),
        kind: get_string(v, "kind"),
        transport_id: get_string(v, "transportId"),
        codec_id: get_string(v, "codecId"),
        packets_received: get_i64(v, "packetsReceived"),
        packets_lost: get_i64(v, "packetsLost"),
        bytes_received: get_i64(v, "bytesReceived"),
        jitter: get_f64(v, "jitter"),
        packets_received_with_ect1: get_i64(v, "packetsReceivedWithEct1"),
        packets_received_with_ce: get_i64(v, "packetsReceivedWithCe"),
        packets_reported_as_lost: get_i64(v, "packetsReportedAsLost"),
        packets_reported_as_lost_but_recovered: get_i64(v, "packetsReportedAsLostButRecovered"),
        last_packet_received_timestamp: get_f64(v, "lastPacketReceivedTimestamp"),
        header_bytes_received: get_i64(v, "headerBytesReceived"),
        packets_discarded: get_i64(v, "packetsDiscarded"),
        fec_bytes_received: get_i64(v, "fecBytesReceived"),
        fec_packets_received: get_i64(v, "fecPacketsReceived"),
        fec_packets_discarded: get_i64(v, "fecPacketsDiscarded"),
        nack_count: get_i64(v, "nackCount"),
        pli_count: get_i64(v, "pliCount"),
        fir_count: get_i64(v, "firCount"),
        track_identifier: get_string(v, "trackIdentifier"),
        mid: get_string(v, "mid"),
        remote_id: get_string(v, "remoteId"),
        frames_decoded: get_i64(v, "framesDecoded"),
        key_frames_decoded: get_i64(v, "keyFramesDecoded"),
        frames_rendered: get_i64(v, "framesRendered"),
        frames_dropped: get_i64(v, "framesDropped"),
        frame_width: get_i64(v, "frameWidth"),
        frame_height: get_i64(v, "frameHeight"),
        frames_per_second: get_f64(v, "framesPerSecond"),
        qp_sum: get_i64(v, "qpSum"),
        total_decode_time: get_f64(v, "totalDecodeTime"),
        total_inter_frame_delay: get_f64(v, "totalInterFrameDelay"),
        total_squared_inter_frame_delay: get_f64(v, "totalSquaredInterFrameDelay"),
        pause_count: get_i64(v, "pauseCount"),
        total_pauses_duration: get_f64(v, "totalPausesDuration"),
        freeze_count: get_i64(v, "freezeCount"),
        total_freezes_duration: get_f64(v, "totalFreezesDuration"),
        total_processing_delay: get_f64(v, "totalProcessingDelay"),
        estimated_playout_timestamp: get_f64(v, "estimatedPlayoutTimestamp"),
        jitter_buffer_delay: get_f64(v, "jitterBufferDelay"),
        jitter_buffer_target_delay: get_f64(v, "jitterBufferTargetDelay"),
        jitter_buffer_emitted_count: get_i64(v, "jitterBufferEmittedCount"),
        jitter_buffer_minimum_delay: get_f64(v, "jitterBufferMinimumDelay"),
        total_samples_received: get_i64(v, "totalSamplesReceived"),
        concealed_samples: get_i64(v, "concealedSamples"),
        silent_concealed_samples: get_i64(v, "silentConcealedSamples"),
        concealment_events: get_i64(v, "concealmentEvents"),
        inserted_samples_for_deceleration: get_i64(v, "insertedSamplesForDeceleration"),
        removed_samples_for_acceleration: get_i64(v, "removedSamplesForAcceleration"),
        audio_level: get_f64(v, "audioLevel"),
        total_audio_energy: get_f64(v, "totalAudioEnergy"),
        total_samples_duration: get_f64(v, "totalSamplesDuration"),
        frames_received: get_i64(v, "framesReceived"),
        decoder_implementation: get_string(v, "decoderImplementation"),
        playout_id: get_string(v, "playoutId"),
        power_efficient_decoder: get_bool(v, "powerEfficientDecoder"),
        frames_assembled_from_multiple_packets: get_i64(v, "framesAssembledFromMultiplePackets"),
        total_assembly_time: get_f64(v, "totalAssemblyTime"),
        retransmitted_packets_received: get_i64(v, "retransmittedPacketsReceived"),
        retransmitted_bytes_received: get_i64(v, "retransmittedBytesReceived"),
        rtx_ssrc: get_i64(v, "rtxSsrc"),
        fec_ssrc: get_i64(v, "fecSsrc"),
        total_corruption_probability: get_f64(v, "totalCorruptionProbability"),
        total_squared_corruption_probability: get_f64(v, "totalSquaredCorruptionProbability"),
        corruption_measurements: get_i64(v, "corruptionMeasurements"),
    })
}

fn parse_outbound_rtp(v: RawJsonValue<'_, '_>, c: StatsCommon) -> Option<RtcStatsOutboundRtpRow> {
    Some(RtcStatsOutboundRtpRow {
        instance_id: c.instance_id,
        timestamp: c.timestamp,
        channel_id: c.channel_id,
        session_id: c.session_id,
        connection_id: c.connection_id,
        rtc_timestamp: c.rtc_timestamp,
        stats_type: c.stats_type,
        id: c.id,
        ssrc: get_i64(v, "ssrc"),
        kind: get_string(v, "kind"),
        transport_id: get_string(v, "transportId"),
        codec_id: get_string(v, "codecId"),
        packets_sent: get_i64(v, "packetsSent"),
        bytes_sent: get_i64(v, "bytesSent"),
        packets_sent_with_ect1: get_i64(v, "packetsSentWithEct1"),
        mid: get_string(v, "mid"),
        media_source_id: get_string(v, "mediaSourceId"),
        remote_id: get_string(v, "remoteId"),
        rid: get_string(v, "rid"),
        encoding_index: get_i64(v, "encodingIndex"),
        header_bytes_sent: get_i64(v, "headerBytesSent"),
        retransmitted_packets_sent: get_i64(v, "retransmittedPacketsSent"),
        retransmitted_bytes_sent: get_i64(v, "retransmittedBytesSent"),
        rtx_ssrc: get_i64(v, "rtxSsrc"),
        target_bitrate: get_f64(v, "targetBitrate"),
        total_encoded_bytes_target: get_i64(v, "totalEncodedBytesTarget"),
        frame_width: get_i64(v, "frameWidth"),
        frame_height: get_i64(v, "frameHeight"),
        frames_per_second: get_f64(v, "framesPerSecond"),
        frames_sent: get_i64(v, "framesSent"),
        huge_frames_sent: get_i64(v, "hugeFramesSent"),
        frames_encoded: get_i64(v, "framesEncoded"),
        key_frames_encoded: get_i64(v, "keyFramesEncoded"),
        qp_sum: get_i64(v, "qpSum"),
        total_encode_time: get_f64(v, "totalEncodeTime"),
        total_packet_send_delay: get_f64(v, "totalPacketSendDelay"),
        quality_limitation_reason: get_string(v, "qualityLimitationReason"),
        quality_limitation_duration_none: get_f64(v, "qualityLimitationDurationNone"),
        quality_limitation_duration_cpu: get_f64(v, "qualityLimitationDurationCpu"),
        quality_limitation_duration_bandwidth: get_f64(v, "qualityLimitationDurationBandwidth"),
        quality_limitation_duration_other: get_f64(v, "qualityLimitationDurationOther"),
        quality_limitation_resolution_changes: get_i64(v, "qualityLimitationResolutionChanges"),
        nack_count: get_i64(v, "nackCount"),
        pli_count: get_i64(v, "pliCount"),
        fir_count: get_i64(v, "firCount"),
        encoder_implementation: get_string(v, "encoderImplementation"),
        power_efficient_encoder: get_bool(v, "powerEfficientEncoder"),
        active: get_bool(v, "active"),
        scalability_mode: get_string(v, "scalabilityMode"),
    })
}

fn parse_media_source(v: RawJsonValue<'_, '_>, c: StatsCommon) -> Option<RtcStatsMediaSourceRow> {
    Some(RtcStatsMediaSourceRow {
        instance_id: c.instance_id,
        timestamp: c.timestamp,
        channel_id: c.channel_id,
        session_id: c.session_id,
        connection_id: c.connection_id,
        rtc_timestamp: c.rtc_timestamp,
        stats_type: c.stats_type,
        id: c.id,
        track_identifier: get_string(v, "trackIdentifier"),
        kind: get_string(v, "kind"),
        audio_level: get_f64(v, "audioLevel"),
        total_audio_energy: get_f64(v, "totalAudioEnergy"),
        total_samples_duration: get_f64(v, "totalSamplesDuration"),
        echo_return_loss: get_f64(v, "echoReturnLoss"),
        echo_return_loss_enhancement: get_f64(v, "echoReturnLossEnhancement"),
        width: get_i64(v, "width"),
        height: get_i64(v, "height"),
        frames: get_i64(v, "frames"),
        frames_per_second: get_f64(v, "framesPerSecond"),
    })
}

fn parse_remote_inbound_rtp(
    v: RawJsonValue<'_, '_>,
    c: StatsCommon,
) -> Option<RtcStatsRemoteInboundRtpRow> {
    Some(RtcStatsRemoteInboundRtpRow {
        instance_id: c.instance_id,
        timestamp: c.timestamp,
        channel_id: c.channel_id,
        session_id: c.session_id,
        connection_id: c.connection_id,
        rtc_timestamp: c.rtc_timestamp,
        stats_type: c.stats_type,
        id: c.id,
        ssrc: get_i64(v, "ssrc"),
        kind: get_string(v, "kind"),
        transport_id: get_string(v, "transportId"),
        codec_id: get_string(v, "codecId"),
        packets_received: get_i64(v, "packetsReceived"),
        packets_received_with_ect1: get_i64(v, "packetsReceivedWithEct1"),
        packets_received_with_ce: get_i64(v, "packetsReceivedWithCe"),
        packets_reported_as_lost: get_i64(v, "packetsReportedAsLost"),
        packets_reported_as_lost_but_recovered: get_i64(v, "packetsReportedAsLostButRecovered"),
        packets_lost: get_i64(v, "packetsLost"),
        jitter: get_f64(v, "jitter"),
        local_id: get_string(v, "localId"),
        round_trip_time: get_f64(v, "roundTripTime"),
        total_round_trip_time: get_f64(v, "totalRoundTripTime"),
        fraction_lost: get_f64(v, "fractionLost"),
        round_trip_time_measurements: get_i64(v, "roundTripTimeMeasurements"),
        packets_with_bleached_ect1_marking: get_i64(v, "packetsWithBleachedEct1Marking"),
    })
}

fn parse_remote_outbound_rtp(
    v: RawJsonValue<'_, '_>,
    c: StatsCommon,
) -> Option<RtcStatsRemoteOutboundRtpRow> {
    Some(RtcStatsRemoteOutboundRtpRow {
        instance_id: c.instance_id,
        timestamp: c.timestamp,
        channel_id: c.channel_id,
        session_id: c.session_id,
        connection_id: c.connection_id,
        rtc_timestamp: c.rtc_timestamp,
        stats_type: c.stats_type,
        id: c.id,
        ssrc: get_i64(v, "ssrc"),
        kind: get_string(v, "kind"),
        transport_id: get_string(v, "transportId"),
        codec_id: get_string(v, "codecId"),
        packets_sent: get_i64(v, "packetsSent"),
        bytes_sent: get_i64(v, "bytesSent"),
        local_id: get_string(v, "localId"),
        remote_timestamp: get_f64(v, "remoteTimestamp"),
        reports_sent: get_i64(v, "reportsSent"),
        round_trip_time: get_f64(v, "roundTripTime"),
        total_round_trip_time: get_f64(v, "totalRoundTripTime"),
        round_trip_time_measurements: get_i64(v, "roundTripTimeMeasurements"),
    })
}

fn parse_data_channel(v: RawJsonValue<'_, '_>, c: StatsCommon) -> Option<RtcStatsDataChannelRow> {
    Some(RtcStatsDataChannelRow {
        instance_id: c.instance_id,
        timestamp: c.timestamp,
        channel_id: c.channel_id,
        session_id: c.session_id,
        connection_id: c.connection_id,
        rtc_timestamp: c.rtc_timestamp,
        stats_type: c.stats_type,
        id: c.id,
        label: get_string(v, "label"),
        protocol: get_string(v, "protocol"),
        data_channel_identifier: get_i16(v, "dataChannelIdentifier"),
        state: get_string(v, "state"),
        messages_sent: get_i64(v, "messagesSent"),
        bytes_sent: get_i64(v, "bytesSent"),
        messages_received: get_i64(v, "messagesReceived"),
        bytes_received: get_i64(v, "bytesReceived"),
    })
}

// ============================================================================
// config_json 構築 (DisplayJson 手書き + 機密情報マスク)
// ============================================================================

/// 機密情報をマスクすることを示すマーカー
///
/// `DisplayJson` を実装し、常に `"<masked>"` を JSON 文字列として出力する。
/// `config_json` 構築時に機密 4 フィールド (`metadata` / `signaling_notify_metadata` /
/// `client_cert` / `client_key`) が `Some(_)` のときにこの型の値を渡す。
struct MaskedJson;

impl DisplayJson for MaskedJson {
    fn fmt(&self, f: &mut JsonFormatter<'_, '_>) -> fmt::Result {
        f.inner_mut().write_str(r#""<masked>""#)
    }
}

/// `config_json` 文字列を構築する
///
/// トップレベルは `{"common": {...}, "instances": [{...}, ...]}`。
/// `Option<T>::None` のフィールドは JSON 出力から省略する。
/// 機密 4 フィールド (`metadata` / `signaling_notify_metadata` / `client_cert` /
/// `client_key`) は `Some(_)` のとき `"<masked>"` で出力する。
pub(crate) fn build_config_json(
    common: &crate::args::CommonArgs,
    instances: &[crate::args::InstanceArgs],
) -> String {
    let json = nojson::json(|f| {
        f.object(|f| {
            f.member("common", common_json(common))?;
            f.member(
                "instances",
                nojson::json(|f| {
                    f.array(|f| {
                        for inst in instances {
                            f.element(instance_json(inst))?;
                        }
                        Ok(())
                    })
                }),
            )
        })
    });
    json.to_string()
}

fn common_json(c: &crate::args::CommonArgs) -> impl DisplayJson + '_ {
    nojson::json(move |f: &mut JsonFormatter<'_, '_>| {
        f.object(|f| {
            f.member("instance_hatch_rate", c.instance_hatch_rate)?;
            if let Some(ref h) = c.http_host {
                f.member("http_host", h)?;
            }
            if let Some(p) = c.http_port {
                f.member("http_port", p)?;
            }
            if let Some(ref p) = c.openh264 {
                f.member("openh264", p)?;
            }
            f.member("insecure", c.insecure)?;
            if c.client_cert.is_some() {
                f.member("client_cert", MaskedJson)?;
            }
            if c.client_key.is_some() {
                f.member("client_key", MaskedJson)?;
            }
            f.member("duckdb_output_dir", c.duckdb_output_dir.as_str())?;
            f.member("duckdb_interval", c.duckdb_interval)?;
            f.member("no_duckdb_output", c.no_duckdb_output)?;
            // CLI / JSONC と同形の小文字文字列で出力する (Debug の PascalCase は使わない)
            f.member("log_level", severity_as_str(c.log_level))?;
            Ok(())
        })
    })
}

/// `log::Severity` を CLI / JSONC と同形の小文字文字列に変換する
fn severity_as_str(s: log::Severity) -> &'static str {
    match s {
        log::Severity::Verbose => "verbose",
        log::Severity::Info => "info",
        log::Severity::Warning => "warning",
        log::Severity::Error => "error",
        log::Severity::None => "none",
        // CLI / JSONC からは Raw を設定しない
        log::Severity::Raw(_) => unreachable!("log_level must not be Severity::Raw"),
    }
}

fn instance_json(i: &crate::args::InstanceArgs) -> impl DisplayJson + '_ {
    nojson::json(move |f: &mut JsonFormatter<'_, '_>| {
        f.object(|f| {
            // signaling_urls は配列
            f.member(
                "sora_signaling_urls",
                nojson::json(|f| {
                    f.array(|f| {
                        for u in &i.signaling_urls {
                            f.element(u.as_str())?;
                        }
                        Ok(())
                    })
                }),
            )?;
            f.member("sora_channel_id", i.channel_id.as_str())?;
            f.member("sora_role", i.role.as_sora_role())?;
            if let Some(ref v) = i.client_id {
                f.member("sora_client_id", v)?;
            }
            if let Some(ref v) = i.bundle_id {
                f.member("sora_bundle_id", v)?;
            }
            if i.metadata.is_some() {
                f.member("sora_metadata", MaskedJson)?;
            }
            if i.signaling_notify_metadata.is_some() {
                f.member("sora_signaling_notify_metadata", MaskedJson)?;
            }
            f.member("vcs", i.vcs)?;
            f.member("vcs_hatch_rate", i.vcs_hatch_rate)?;
            if let Some(v) = i.duration {
                f.member("duration", v)?;
            }
            if let Some(v) = i.repeat_interval {
                f.member("repeat_interval", v)?;
            }
            f.member("max_retry", i.max_retry)?;
            f.member("retry_interval", i.retry_interval)?;
            f.member("no_video_device", i.no_video_device)?;
            f.member("no_audio_device", i.no_audio_device)?;
            if let Some(ref v) = i.video_input_device {
                f.member("video_input_device", v)?;
            }
            // resolution は {width, height}
            f.member(
                "resolution",
                nojson::json(|f| {
                    f.object(|f| {
                        f.member("width", i.resolution.0)?;
                        f.member("height", i.resolution.1)
                    })
                }),
            )?;
            f.member("framerate", i.framerate)?;
            f.member("sandstorm", i.sandstorm)?;
            if let Some(ref v) = i.input_y4m {
                f.member("input_y4m", v)?;
            }
            if let Some(ref v) = i.input_mp4 {
                f.member("input_mp4", v)?;
            }
            if let Some(ref v) = i.input_wav {
                f.member("input_wav", v)?;
            }
            if let Some(ref v) = i.video_codec_type {
                f.member("sora_video_codec_type", v)?;
            }
            if let Some(v) = i.video_bit_rate {
                f.member("sora_video_bit_rate", v)?;
            }
            if let Some(ref v) = i.vp8_encoder {
                f.member("vp8_encoder", v)?;
            }
            if let Some(ref v) = i.vp9_encoder {
                f.member("vp9_encoder", v)?;
            }
            if let Some(ref v) = i.av1_encoder {
                f.member("av1_encoder", v)?;
            }
            if let Some(ref v) = i.h264_encoder {
                f.member("h264_encoder", v)?;
            }
            if let Some(ref v) = i.h265_encoder {
                f.member("h265_encoder", v)?;
            }
            f.member("audio", i.audio)?;
            if let Some(ref v) = i.audio_codec_type {
                f.member("sora_audio_codec_type", v)?;
            }
            if let Some(v) = i.audio_bit_rate {
                f.member("sora_audio_bit_rate", v)?;
            }
            if let Some(ref v) = i.data_channels {
                f.member("sora_data_channels", v)?;
            }
            if let Some(v) = i.data_channel_signaling {
                f.member("sora_data_channel_signaling", v)?;
            }
            if let Some(v) = i.ignore_disconnect_websocket {
                f.member("sora_ignore_disconnect_websocket", v)?;
            }
            if let Some(v) = i.disconnect_wait_timeout {
                f.member("sora_disconnect_wait_timeout", v)?;
            }
            if let Some(v) = i.simulcast {
                f.member("sora_simulcast", v)?;
            }
            if let Some(ref v) = i.simulcast_request_rid {
                f.member("sora_simulcast_request_rid", v)?;
            }
            if let Some(v) = i.spotlight {
                f.member("sora_spotlight", v)?;
            }
            if let Some(ref v) = i.spotlight_focus_rid {
                f.member("sora_spotlight_focus_rid", v)?;
            }
            if let Some(ref v) = i.spotlight_unfocus_rid {
                f.member("sora_spotlight_unfocus_rid", v)?;
            }
            if let Some(v) = i.scenario {
                let s = match v {
                    crate::scenario::ScenarioType::Reconnect => "reconnect",
                };
                f.member("scenario", s)?;
            }
            Ok(())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::SystemTime;

    use duckdb::Connection;
    use tokio::sync::mpsc;

    use crate::duckdb_stats::{
        clear_unknown_types_for_test, schema::SCHEMA_SQL, writer::dispatch_command,
    };

    /// 未知 RTCStats type の集合 (UNKNOWN_TYPES) を触るテストを直列化するミューテックス
    ///
    /// dispatch_stats の UNKNOWN_TYPES 集合 (global static) を操作するテスト同士が
    /// 並列実行されると clear / サイズ計測が干渉して不安定になるため、この
    /// ロックでテストを直列化する。
    static UNKNOWN_TYPES_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 一時ディレクトリに `.db` ファイルを作りスキーマを投入するヘルパー
    fn setup_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("test.db");
        let conn = Connection::open(&path).expect("DuckDB open に失敗");
        conn.execute_batch(SCHEMA_SQL).expect("スキーマ投入に失敗");
        (dir, conn)
    }

    // ---- B. RTCStats JSON 振り分け ----

    #[test]
    fn dispatch_stats_inserts_known_types() {
        let _guard = UNKNOWN_TYPES_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let (_dir, conn) = setup_db();
        let (tx, rx) = mpsc::channel::<WriteCommand>(64);
        let client = DuckDBClient {
            sender: Some(tx),
            dropped_count: Arc::new(AtomicU64::new(0)),
        };
        let ids = ConnectionIds {
            connection_id: "c1".into(),
            session_id: "s1".into(),
        };
        let stats = r#"[
            {"type":"codec","id":"C1","timestamp":1.0,"mimeType":"video/VP8","payloadType":96,"clockRate":90000},
            {"type":"inbound-rtp","id":"I1","timestamp":1.0,"ssrc":123,"kind":"video","packetsReceived":10},
            {"type":"outbound-rtp","id":"O1","timestamp":1.0,"ssrc":456,"kind":"video","packetsSent":20},
            {"type":"media-source","id":"M1","timestamp":1.0,"kind":"video","width":640,"height":480},
            {"type":"remote-inbound-rtp","id":"RI1","timestamp":1.0,"ssrc":123,"localId":"O1"},
            {"type":"remote-outbound-rtp","id":"RO1","timestamp":1.0,"ssrc":456,"localId":"I1"},
            {"type":"data-channel","id":"D1","timestamp":1.0,"label":"spam","state":"open"},
            {"type":"transport","id":"T1","timestamp":1.0}
        ]"#;
        clear_unknown_types_for_test();
        let count = dispatch_stats(0, 0, "ch", &ids, &client, stats, SystemTime::now());
        assert_eq!(
            count, 7,
            "既知 type 7 種が投入されるべき (transport は未対応)"
        );

        // writer 側で消費して各テーブルに 1 行ずつ入ることを確認
        // (tokio runtime 無しの同期テストなので blocking_recv は使えない。
        //  try_recv でチャネルが空になるまで消費する)
        let mut rx = rx;
        while let Ok(cmd) = rx.try_recv() {
            dispatch_command(&conn, cmd).expect("INSERT 失敗");
        }
        for (table, n) in [
            ("rtc_stats_codec", 1),
            ("rtc_stats_inbound_rtp", 1),
            ("rtc_stats_outbound_rtp", 1),
            ("rtc_stats_media_source", 1),
            ("rtc_stats_remote_inbound_rtp", 1),
            ("rtc_stats_remote_outbound_rtp", 1),
            ("rtc_stats_data_channel", 1),
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("カウント取得に失敗");
            assert_eq!(count, n, "{table} に {n} 行あるべき");
        }
    }

    #[test]
    fn dispatch_stats_unknown_type_logged_once() {
        let _guard = UNKNOWN_TYPES_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let (tx, _rx) = mpsc::channel::<WriteCommand>(64);
        let client = DuckDBClient {
            sender: Some(tx),
            dropped_count: Arc::new(AtomicU64::new(0)),
        };
        let ids = ConnectionIds {
            connection_id: "c1".into(),
            session_id: "s1".into(),
        };
        let unique = format!("unknown-type-{}-{}", module_path!(), line!());
        let stats = format!(r#"[{{"type":"{unique}","id":"X1","timestamp":1.0}}]"#);
        // このロックの間は他テストが UNKNOWN_TYPES に触れないため、
        // clear 後に投入して集合のサイズで「1 つだけ追加される (警告は 1 回のみ)」を検証できる
        clear_unknown_types_for_test();
        for _ in 0..100 {
            dispatch_stats(0, 0, "ch", &ids, &client, &stats, SystemTime::now());
        }
        let unknown_types = unknown_types();
        let set = unknown_types.lock().expect("UNKNOWN_TYPES mutex poisoned");
        assert_eq!(
            set.len(),
            1,
            "同一未知 type は集合に 1 つだけ追加されるべき"
        );
        assert!(
            set.contains(&unique),
            "この test 固有の unknown type が登録されるべき"
        );
    }

    // ---- D. config_json マスク ----

    #[test]
    fn build_config_json_masks_sensitive_fields() {
        use crate::args::{CommonArgs, InstanceArgs};
        use crate::scenario::ScenarioType;
        use sora_sdk::Role;
        let common = CommonArgs {
            instance_hatch_rate: 1.0,
            http_host: None,
            http_port: None,
            openh264: None,
            insecure: false,
            client_cert: Some("/secret/cert.pem".into()),
            client_key: Some("/secret/key.pem".into()),
            duckdb_output_dir: ".".into(),
            duckdb_interval: 1.0,
            no_duckdb_output: false,
            log_level: log::Severity::Info,
        };
        let inst = InstanceArgs {
            signaling_urls: vec!["wss://example.com/".into()],
            channel_id: "ch".into(),
            role: Role::SendOnly,
            client_id: None,
            bundle_id: None,
            metadata: Some(r#"{"token":"abc"}"#.into()),
            signaling_notify_metadata: Some(r#"{"k":"v"}"#.into()),
            vcs: 1,
            vcs_hatch_rate: 1.0,
            duration: None,
            repeat_interval: None,
            max_retry: 0,
            retry_interval: 60.0,
            no_video_device: false,
            no_audio_device: false,
            video_input_device: None,
            resolution: (640, 480),
            framerate: 30,
            sandstorm: false,
            input_y4m: None,
            input_mp4: None,
            input_wav: None,
            video_codec_type: None,
            video_bit_rate: None,
            vp8_encoder: None,
            vp9_encoder: None,
            av1_encoder: None,
            h264_encoder: None,
            h265_encoder: None,
            audio: true,
            audio_codec_type: None,
            audio_bit_rate: None,
            data_channels: None,
            data_channel_signaling: None,
            ignore_disconnect_websocket: None,
            disconnect_wait_timeout: None,
            simulcast: None,
            simulcast_request_rid: None,
            spotlight: None,
            spotlight_focus_rid: None,
            spotlight_unfocus_rid: None,
            scenario: Some(ScenarioType::Reconnect),
        };
        let json = build_config_json(&common, &[inst]);
        assert!(
            json.contains("\"<masked>\""),
            "機密フィールドがマスクされるべき"
        );
        assert!(
            !json.contains("/secret/cert.pem"),
            "client_cert の実値が漏れないべき"
        );
        assert!(
            !json.contains("/secret/key.pem"),
            "client_key の実値が漏れないべき"
        );
        assert!(
            !json.contains(r#""token":"abc""#),
            "metadata の実値が漏れないべき"
        );
        assert!(json.contains("sendonly"), "role の文字列が含まれるべき");
    }

    #[test]
    fn build_config_json_omits_none_fields() {
        use crate::args::{CommonArgs, InstanceArgs};
        use sora_sdk::Role;
        let common = CommonArgs {
            instance_hatch_rate: 1.0,
            http_host: None,
            http_port: None,
            openh264: None,
            insecure: false,
            client_cert: None,
            client_key: None,
            duckdb_output_dir: ".".into(),
            duckdb_interval: 1.0,
            no_duckdb_output: false,
            log_level: log::Severity::Info,
        };
        let inst = InstanceArgs {
            signaling_urls: vec!["wss://example.com/".into()],
            channel_id: "ch".into(),
            role: Role::SendOnly,
            client_id: None,
            bundle_id: None,
            metadata: None,
            signaling_notify_metadata: None,
            vcs: 1,
            vcs_hatch_rate: 1.0,
            duration: None,
            repeat_interval: None,
            max_retry: 0,
            retry_interval: 60.0,
            no_video_device: false,
            no_audio_device: false,
            video_input_device: None,
            resolution: (640, 480),
            framerate: 30,
            sandstorm: false,
            input_y4m: None,
            input_mp4: None,
            input_wav: None,
            video_codec_type: None,
            video_bit_rate: None,
            vp8_encoder: None,
            vp9_encoder: None,
            av1_encoder: None,
            h264_encoder: None,
            h265_encoder: None,
            audio: true,
            audio_codec_type: None,
            audio_bit_rate: None,
            data_channels: None,
            data_channel_signaling: None,
            ignore_disconnect_websocket: None,
            disconnect_wait_timeout: None,
            simulcast: None,
            simulcast_request_rid: None,
            spotlight: None,
            spotlight_focus_rid: None,
            spotlight_unfocus_rid: None,
            scenario: None,
        };
        let json = build_config_json(&common, &[inst]);
        // None のフィールドのキーは出力に含まれない
        assert!(
            !json.contains("client_cert"),
            "None の client_cert は省かれるべき"
        );
        assert!(
            !json.contains("metadata"),
            "None の metadata は省かれるべき"
        );
        assert!(
            !json.contains("duration"),
            "None の duration は省かれるべき"
        );
        // デフォルトの log_level は小文字 "info" で出力される
        assert!(
            json.contains(r#""log_level":"info""#),
            "デフォルト log_level は \"info\" で含まれるべき: {json}"
        );
        assert!(
            !json.contains(r#""log_level":"Info""#),
            "log_level に Debug 形式 (PascalCase) を使ってはならない"
        );
    }

    #[test]
    fn build_config_json_includes_encoder_implementation_fields() {
        // エンコーダー実装指定が値付きで config_json に含まれる
        use crate::args::{CommonArgs, InstanceArgs};
        use sora_sdk::Role;
        let common = CommonArgs {
            instance_hatch_rate: 1.0,
            http_host: None,
            http_port: None,
            openh264: None,
            insecure: false,
            client_cert: None,
            client_key: None,
            duckdb_output_dir: ".".into(),
            duckdb_interval: 1.0,
            no_duckdb_output: false,
            log_level: log::Severity::Info,
        };
        let inst = InstanceArgs {
            signaling_urls: vec!["wss://example.com/".into()],
            channel_id: "ch".into(),
            role: Role::SendOnly,
            client_id: None,
            bundle_id: None,
            metadata: None,
            signaling_notify_metadata: None,
            vcs: 1,
            vcs_hatch_rate: 1.0,
            duration: None,
            repeat_interval: None,
            max_retry: 0,
            retry_interval: 60.0,
            no_video_device: false,
            no_audio_device: false,
            video_input_device: None,
            resolution: (640, 480),
            framerate: 30,
            sandstorm: false,
            input_y4m: None,
            input_mp4: None,
            input_wav: None,
            video_codec_type: None,
            video_bit_rate: None,
            vp8_encoder: Some("internal".into()),
            vp9_encoder: None,
            av1_encoder: None,
            h264_encoder: Some("cisco_openh264".into()),
            h265_encoder: None,
            audio: true,
            audio_codec_type: None,
            audio_bit_rate: None,
            data_channels: None,
            data_channel_signaling: None,
            ignore_disconnect_websocket: None,
            disconnect_wait_timeout: None,
            simulcast: None,
            simulcast_request_rid: None,
            spotlight: None,
            spotlight_focus_rid: None,
            spotlight_unfocus_rid: None,
            scenario: None,
        };
        let json = build_config_json(&common, &[inst]);
        assert!(
            json.contains(r#""vp8_encoder":"internal""#),
            "vp8_encoder が config_json に含まれるべき: {json}"
        );
        assert!(
            json.contains(r#""h264_encoder":"cisco_openh264""#),
            "h264_encoder が config_json に含まれるべき: {json}"
        );
        assert!(
            !json.contains(r#""vp9_encoder""#),
            "None の vp9_encoder は省かれるべき: {json}"
        );
    }

    #[test]
    fn build_config_json_emits_warning_log_level_in_lowercase() {
        // Severity::Warning は "warning" (小文字) で出力し、"Warning" にはしない
        use crate::args::{CommonArgs, InstanceArgs};
        use sora_sdk::Role;
        let common = CommonArgs {
            instance_hatch_rate: 1.0,
            http_host: None,
            http_port: None,
            openh264: None,
            insecure: false,
            client_cert: None,
            client_key: None,
            duckdb_output_dir: ".".into(),
            duckdb_interval: 1.0,
            no_duckdb_output: false,
            log_level: log::Severity::Warning,
        };
        let inst = InstanceArgs {
            signaling_urls: vec!["wss://example.com/".into()],
            channel_id: "ch".into(),
            role: Role::SendOnly,
            client_id: None,
            bundle_id: None,
            metadata: None,
            signaling_notify_metadata: None,
            vcs: 1,
            vcs_hatch_rate: 1.0,
            duration: None,
            repeat_interval: None,
            max_retry: 0,
            retry_interval: 60.0,
            no_video_device: false,
            no_audio_device: false,
            video_input_device: None,
            resolution: (640, 480),
            framerate: 30,
            sandstorm: false,
            input_y4m: None,
            input_mp4: None,
            input_wav: None,
            video_codec_type: None,
            video_bit_rate: None,
            vp8_encoder: None,
            vp9_encoder: None,
            av1_encoder: None,
            h264_encoder: None,
            h265_encoder: None,
            audio: true,
            audio_codec_type: None,
            audio_bit_rate: None,
            data_channels: None,
            data_channel_signaling: None,
            ignore_disconnect_websocket: None,
            disconnect_wait_timeout: None,
            simulcast: None,
            simulcast_request_rid: None,
            spotlight: None,
            spotlight_focus_rid: None,
            spotlight_unfocus_rid: None,
            scenario: None,
        };
        let json = build_config_json(&common, &[inst]);
        assert!(
            json.contains(r#""log_level":"warning""#),
            "log_level=Warning は \"warning\" で出力されるべき: {json}"
        );
        assert!(
            !json.contains(r#""log_level":"Warning""#),
            "log_level に Debug 形式 (PascalCase) を使ってはならない"
        );
    }

    // ---- E. parse_offer_ids ----

    #[test]
    fn parse_offer_ids_extracts_connection_and_session() {
        let text = r#"{"type":"offer","connection_id":"c1","session_id":"s1","sdp":"v=0\r\n"}"#;
        let ids = parse_offer_ids(text).expect("offer から IDs を抽出できるべき");
        assert_eq!(ids.connection_id, "c1");
        assert_eq!(ids.session_id, "s1");
    }

    #[test]
    fn parse_offer_ids_returns_none_for_non_offer() {
        for ty in &["update", "re-offer", "notify", "answer"] {
            let text = format!(r#"{{"type":"{ty}","connection_id":"c","session_id":"s"}}"#);
            assert!(
                parse_offer_ids(&text).is_none(),
                "type={ty} は None を返すべき"
            );
        }
    }

    #[test]
    fn parse_offer_ids_returns_none_for_missing_keys() {
        // connection_id 欠落
        assert!(parse_offer_ids(r#"{"type":"offer","session_id":"s"}"#).is_none());
        // session_id 欠落
        assert!(parse_offer_ids(r#"{"type":"offer","connection_id":"c"}"#).is_none());
    }

    #[test]
    fn parse_offer_ids_returns_none_for_non_string_values() {
        // connection_id が整数
        assert!(
            parse_offer_ids(r#"{"type":"offer","connection_id":123,"session_id":"s"}"#).is_none()
        );
        // session_id が null
        assert!(
            parse_offer_ids(r#"{"type":"offer","connection_id":"c","session_id":null}"#).is_none()
        );
    }

    #[test]
    fn parse_offer_ids_returns_none_for_invalid_json() {
        assert!(parse_offer_ids("{not json").is_none());
    }

    // ---- ファイル名生成 ----

    #[test]
    fn generate_filename_matches_pattern() {
        let name = generate_filename();
        // zakuro_YYYYMMDD_HHMMSS_mmm.db 形式
        assert!(
            name.starts_with("zakuro_") && name.ends_with(".db"),
            "ファイル名が期待する前置/拡張子でない: {name}"
        );
        // セパレータ _ で 5 区画 (zakuro, date, time, millis, db)
        let parts: Vec<&str> = name.trim_end_matches(".db").split('_').collect();
        assert_eq!(parts.len(), 4, "ファイル名の区画数が期待と違う: {name}");
        assert_eq!(parts[1].len(), 8, "日付部分は 8 桁のべき: {name}");
        assert_eq!(parts[2].len(), 6, "時刻部分は 6 桁のべき: {name}");
        assert_eq!(parts[3].len(), 3, "ミリ秒部分は 3 桁のべき: {name}");
    }
}
