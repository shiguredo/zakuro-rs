-- DuckDB スキーマ (zakuro-rs)
-- C++ 版 zakuro の DDL をベースとし、以下の差分を加える:
-- - 全 stats テーブル (connection / rtc_stats_*) に instance_id INTEGER 列を pk の直後に挿入
-- - zakuro テーブルは 1 行レコード前提のため instance_id 列を持たない
-- - zakuro_scenario は instance 単位 1 行で instance_id を 1 列目に持つ
-- - config_mode の値は "ARGS" / "JSONC" (C++ 版は "ARGS" / "YAML")
-- - C++ 版固有の boost_version / cli11_version / cmake_version / blend2d_version / yaml_cpp_version 列は省略
-- - sora_sdk_version 列を zakuro テーブルに追加 (C++ 版は sora_cpp_sdk_version)

BEGIN;

-- シーケンス
CREATE SEQUENCE connection_pk_seq;
CREATE SEQUENCE rtc_stats_codec_pk_seq;
CREATE SEQUENCE rtc_stats_inbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_outbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_media_source_pk_seq;
CREATE SEQUENCE rtc_stats_remote_inbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_remote_outbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_data_channel_pk_seq;

-- zakuro: 起動情報 (1 行のみ、instance_id 列なし)
CREATE TABLE zakuro (
    id INTEGER PRIMARY KEY DEFAULT 0 CHECK (id = 0),
    version VARCHAR,
    sora_sdk_version VARCHAR,
    webrtc_version VARCHAR,
    openh264_version VARCHAR,
    duckdb_version VARCHAR,
    environment VARCHAR,
    config_mode VARCHAR,
    config_json JSON,
    start_timestamp TIMESTAMP,
    stop_timestamp TIMESTAMP
);

-- zakuro_scenario: 各 instance のシナリオ設定
CREATE TABLE zakuro_scenario (
    instance_id INTEGER,
    vcs INTEGER,
    duration DOUBLE,
    repeat_interval DOUBLE,
    max_retry INTEGER,
    retry_interval DOUBLE,
    sora_signaling_urls VARCHAR[],
    sora_channel_id VARCHAR,
    sora_role VARCHAR
);

-- connection: 接続情報 (各 Sora connection 1 行、offer 受信時にのみ書く)
CREATE TABLE connection (
    pk BIGINT PRIMARY KEY DEFAULT nextval('connection_pk_seq'),
    instance_id INTEGER,
    vc_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    connection_id VARCHAR,
    session_id VARCHAR,
    role VARCHAR,
    audio BOOLEAN,
    video BOOLEAN,
    websocket_connected BOOLEAN,
    datachannel_connected BOOLEAN
);

-- rtc_stats_codec: codec 統計 (重複は ON CONFLICT で抑制)
CREATE TABLE rtc_stats_codec (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_codec_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    mime_type VARCHAR,
    payload_type BIGINT,
    clock_rate BIGINT,
    channels BIGINT,
    sdp_fmtp_line VARCHAR,
    UNIQUE(connection_id, id, mime_type, payload_type, clock_rate, channels, sdp_fmtp_line)
);

-- rtc_stats_inbound_rtp: 受信 RTP ストリーム統計
CREATE TABLE rtc_stats_inbound_rtp (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_inbound_rtp_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    ssrc BIGINT,
    kind VARCHAR,
    transport_id VARCHAR,
    codec_id VARCHAR,
    packets_received BIGINT,
    packets_lost BIGINT,
    bytes_received BIGINT,
    jitter DOUBLE,
    packets_received_with_ect1 BIGINT,
    packets_received_with_ce BIGINT,
    packets_reported_as_lost BIGINT,
    packets_reported_as_lost_but_recovered BIGINT,
    last_packet_received_timestamp DOUBLE,
    header_bytes_received BIGINT,
    packets_discarded BIGINT,
    fec_bytes_received BIGINT,
    fec_packets_received BIGINT,
    fec_packets_discarded BIGINT,
    nack_count BIGINT,
    pli_count BIGINT,
    fir_count BIGINT,
    track_identifier VARCHAR,
    mid VARCHAR,
    remote_id VARCHAR,
    frames_decoded BIGINT,
    key_frames_decoded BIGINT,
    frames_rendered BIGINT,
    frames_dropped BIGINT,
    frame_width BIGINT,
    frame_height BIGINT,
    frames_per_second DOUBLE,
    qp_sum BIGINT,
    total_decode_time DOUBLE,
    total_inter_frame_delay DOUBLE,
    total_squared_inter_frame_delay DOUBLE,
    pause_count BIGINT,
    total_pauses_duration DOUBLE,
    freeze_count BIGINT,
    total_freezes_duration DOUBLE,
    total_processing_delay DOUBLE,
    estimated_playout_timestamp DOUBLE,
    jitter_buffer_delay DOUBLE,
    jitter_buffer_target_delay DOUBLE,
    jitter_buffer_emitted_count BIGINT,
    jitter_buffer_minimum_delay DOUBLE,
    total_samples_received BIGINT,
    concealed_samples BIGINT,
    silent_concealed_samples BIGINT,
    concealment_events BIGINT,
    inserted_samples_for_deceleration BIGINT,
    removed_samples_for_acceleration BIGINT,
    audio_level DOUBLE,
    total_audio_energy DOUBLE,
    total_samples_duration DOUBLE,
    frames_received BIGINT,
    decoder_implementation VARCHAR,
    playout_id VARCHAR,
    power_efficient_decoder BOOLEAN,
    frames_assembled_from_multiple_packets BIGINT,
    total_assembly_time DOUBLE,
    retransmitted_packets_received BIGINT,
    retransmitted_bytes_received BIGINT,
    rtx_ssrc BIGINT,
    fec_ssrc BIGINT,
    total_corruption_probability DOUBLE,
    total_squared_corruption_probability DOUBLE,
    corruption_measurements BIGINT
);

-- rtc_stats_outbound_rtp: 送信 RTP ストリーム統計
-- psnrSum / psnrMeasurements は record<DOMString, double> 型のため未対応
CREATE TABLE rtc_stats_outbound_rtp (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_outbound_rtp_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    ssrc BIGINT,
    kind VARCHAR,
    transport_id VARCHAR,
    codec_id VARCHAR,
    packets_sent BIGINT,
    bytes_sent BIGINT,
    packets_sent_with_ect1 BIGINT,
    mid VARCHAR,
    media_source_id VARCHAR,
    remote_id VARCHAR,
    rid VARCHAR,
    encoding_index BIGINT,
    header_bytes_sent BIGINT,
    retransmitted_packets_sent BIGINT,
    retransmitted_bytes_sent BIGINT,
    rtx_ssrc BIGINT,
    target_bitrate DOUBLE,
    total_encoded_bytes_target BIGINT,
    frame_width BIGINT,
    frame_height BIGINT,
    frames_per_second DOUBLE,
    frames_sent BIGINT,
    huge_frames_sent BIGINT,
    frames_encoded BIGINT,
    key_frames_encoded BIGINT,
    qp_sum BIGINT,
    total_encode_time DOUBLE,
    total_packet_send_delay DOUBLE,
    quality_limitation_reason VARCHAR,
    quality_limitation_duration_none DOUBLE,
    quality_limitation_duration_cpu DOUBLE,
    quality_limitation_duration_bandwidth DOUBLE,
    quality_limitation_duration_other DOUBLE,
    quality_limitation_resolution_changes BIGINT,
    nack_count BIGINT,
    pli_count BIGINT,
    fir_count BIGINT,
    encoder_implementation VARCHAR,
    power_efficient_encoder BOOLEAN,
    active BOOLEAN,
    scalability_mode VARCHAR
);

-- rtc_stats_media_source: メディアソース統計
CREATE TABLE rtc_stats_media_source (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_media_source_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    track_identifier VARCHAR,
    kind VARCHAR,
    audio_level DOUBLE,
    total_audio_energy DOUBLE,
    total_samples_duration DOUBLE,
    echo_return_loss DOUBLE,
    echo_return_loss_enhancement DOUBLE,
    width BIGINT,
    height BIGINT,
    frames BIGINT,
    frames_per_second DOUBLE
);

-- rtc_stats_remote_inbound_rtp: リモート受信 RTP 統計
CREATE TABLE rtc_stats_remote_inbound_rtp (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_remote_inbound_rtp_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    ssrc BIGINT,
    kind VARCHAR,
    transport_id VARCHAR,
    codec_id VARCHAR,
    packets_received BIGINT,
    packets_received_with_ect1 BIGINT,
    packets_received_with_ce BIGINT,
    packets_reported_as_lost BIGINT,
    packets_reported_as_lost_but_recovered BIGINT,
    packets_lost BIGINT,
    jitter DOUBLE,
    local_id VARCHAR,
    round_trip_time DOUBLE,
    total_round_trip_time DOUBLE,
    fraction_lost DOUBLE,
    round_trip_time_measurements BIGINT,
    packets_with_bleached_ect1_marking BIGINT
);

-- rtc_stats_remote_outbound_rtp: リモート送信 RTP 統計
CREATE TABLE rtc_stats_remote_outbound_rtp (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_remote_outbound_rtp_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    ssrc BIGINT,
    kind VARCHAR,
    transport_id VARCHAR,
    codec_id VARCHAR,
    packets_sent BIGINT,
    bytes_sent BIGINT,
    local_id VARCHAR,
    remote_timestamp DOUBLE,
    reports_sent BIGINT,
    round_trip_time DOUBLE,
    total_round_trip_time DOUBLE,
    round_trip_time_measurements BIGINT
);

-- rtc_stats_data_channel: データチャネル統計
CREATE TABLE rtc_stats_data_channel (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_data_channel_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    label VARCHAR,
    protocol VARCHAR,
    data_channel_identifier SMALLINT,
    state VARCHAR,
    messages_sent BIGINT,
    bytes_sent BIGINT,
    messages_received BIGINT,
    bytes_received BIGINT
);

-- インデックス
CREATE INDEX idx_connection_id ON connection(connection_id);
CREATE INDEX idx_connection_composite ON connection(channel_id, timestamp);
CREATE INDEX idx_rtc_stats_codec_composite ON rtc_stats_codec(instance_id, channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_inbound_rtp_composite ON rtc_stats_inbound_rtp(instance_id, channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_outbound_rtp_composite ON rtc_stats_outbound_rtp(instance_id, channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_media_source_composite ON rtc_stats_media_source(instance_id, channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_remote_inbound_rtp_composite ON rtc_stats_remote_inbound_rtp(instance_id, channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_remote_outbound_rtp_composite ON rtc_stats_remote_outbound_rtp(instance_id, channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_data_channel_composite ON rtc_stats_data_channel(instance_id, channel_id, connection_id, timestamp);

COMMIT;
