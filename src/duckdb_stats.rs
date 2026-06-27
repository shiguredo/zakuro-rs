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

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use duckdb::types::{TimeUnit, Value as DuckValue};
use duckdb::{Connection, ToSql};
use nojson::{DisplayJson, JsonFormatter, RawJsonOwned, RawJsonValue};
use shiguredo_webrtc::{rtc_log_info, rtc_log_warning};
use tokio::sync::mpsc;

use crate::error::{AppError, ErrorMessage, Result};

/// mpsc チャネルのバッファサイズ
/// 典型運用 (instances <= 4 × vcs <= 100) で 1 秒あたり ~2,800 commands を
/// 2 秒分超バッファできるサイズ。最大スケールでは drop が発生しうるが
/// 「サンプリング欠落の許容」を運用ポリシーとする
const CHANNEL_CAPACITY: usize = 8192;

/// 未知 RTCStats type の warn を初回のみ出すための全局集合
/// (transport / candidate-pair 等の未対応 type が毎秒 warn で洪水化するのを防ぐ)
static UNKNOWN_TYPES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn unknown_types() -> &'static Mutex<HashSet<String>> {
    UNKNOWN_TYPES.get_or_init(|| Mutex::new(HashSet::new()))
}

/// テスト用: 未知 type 集合のサイズを返す
#[cfg(test)]
pub(crate) fn unknown_types_size_for_test() -> usize {
    unknown_types()
        .lock()
        .expect("UNKNOWN_TYPES mutex poisoned")
        .len()
}

/// テスト用: 未知 type 集合をクリアする
#[cfg(test)]
pub(crate) fn clear_unknown_types_for_test() {
    unknown_types()
        .lock()
        .expect("UNKNOWN_TYPES mutex poisoned")
        .clear();
}

// ============================================================================
// DDL
// ============================================================================

/// シーケンス 8 個 + テーブル 10 個 + インデックス 9 個を 1 発で投入する DDL
///
/// `BEGIN; ... COMMIT;` で囲むことで `Connection::execute_batch` 1 回で投入する。
/// `instance_id INTEGER` 列を各 stats テーブルの `pk` 列の直後に挿入する
/// (C++ 版 zakuro の DDL に Rust 版固有の差分を加えた正本)。
const SCHEMA_SQL: &str = include_str!("duckdb_schema.sql");

// ============================================================================
// Config / Writer / Client
// ============================================================================

/// DuckDB writer の起動設定
#[derive(Debug, Clone)]
pub(crate) struct DuckDBWriterConfig {
    /// 生成済みの絶対パス (main 側で `output_dir.join(generate_filename())` で作る)
    pub(crate) db_path: PathBuf,
    /// VirtualClient の get_stats 呼び出し間隔
    pub(crate) interval: Duration,
    /// `--no-duckdb-output` 指定時は false
    pub(crate) enabled: bool,
}

/// DuckDB 統計書き込みのライフサイクルを管理する
///
/// `start` で writer task を起動し、`client` 経由で `WriteCommand` を送る。
/// disabled 時は `join_handle = None` で task を起動しない。
pub(crate) struct DuckDBStatsWriter {
    join_handle: Option<tokio::task::JoinHandle<()>>,
    client: DuckDBClient,
}

impl DuckDBStatsWriter {
    /// writer task を起動し、init readiness ハンドシェイクでスキーマ投入完了を待つ
    ///
    /// 戻り値の `String` は DuckDB のバージョン文字列 (テスト / `zakuro` テーブル用)。
    /// `config.enabled == false` のときは task を起動せず noop クライアントを返す。
    pub(crate) async fn start(config: DuckDBWriterConfig) -> Result<(Self, String)> {
        if !config.enabled {
            return Ok((
                Self {
                    join_handle: None,
                    client: DuckDBClient::noop(),
                },
                String::new(),
            ));
        }

        let (init_tx, init_rx) =
            tokio::sync::oneshot::channel::<std::result::Result<String, AppError>>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<WriteCommand>(CHANNEL_CAPACITY);
        let dropped_count = Arc::new(AtomicU64::new(0));
        let db_path = config.db_path.clone();

        let join_handle = tokio::task::spawn_blocking(move || {
            // Connection::open + execute_batch でスキーマ投入 + version 取得
            let init_result: std::result::Result<(Connection, String), AppError> = (|| {
                let conn = Connection::open(&db_path)?;
                conn.execute_batch(SCHEMA_SQL)?;
                let version = conn.version().unwrap_or_else(|_| "unknown".to_string());
                Ok((conn, version))
            })();

            let conn = match init_result {
                Ok(pair) => {
                    let (conn, ver) = pair;
                    let _ = init_tx.send(Ok(ver));
                    conn
                }
                Err(e) => {
                    // init 失敗時はファイルを削除して main 側に通知
                    let _ = init_tx.send(Err(e));
                    // ファイルが残ると次回起動で同名衝突する可能性があるため削除する
                    let _ = std::fs::remove_file(&db_path);
                    return;
                }
            };

            rtc_log_info!(
                "[duckdb] writer started: path={:?}, interval={:.2}s",
                db_path,
                config.interval.as_secs_f64()
            );

            // writer 本体: Handle::current().block_on で recv ループを回す
            let handle = tokio::runtime::Handle::current();
            let _entered = handle.enter();
            handle.block_on(writer_run_loop(conn, cmd_rx));

            rtc_log_info!("[duckdb] writer stopped");
        });

        // init readiness を待つ (spawn_blocking 側で init_tx.send が呼ばれるまで await)
        let duckdb_version = init_rx.await.map_err(|_| {
            AppError::Message(ErrorMessage::new("duckdb writer aborted during init"))
        })??;

        let client = DuckDBClient {
            sender: Some(cmd_tx),
            dropped_count,
        };

        // reporter task (dropped_count の定期 warn) は writer 本体とは別 task に分離する
        // (writer 本体 select に並べると最大スケール時に reporter が starvation するため)
        let reporter_dropped = client.dropped_count.clone();
        tokio::spawn(reporter_loop(reporter_dropped));

        Ok((
            Self {
                join_handle: Some(join_handle),
                client,
            },
            duckdb_version,
        ))
    }

    /// main 側の client を返す (InsertZakuro / Scenario / shutdown 用)
    pub(crate) fn client(&self) -> DuckDBClient {
        self.client.clone()
    }

    /// shutdown ハンドシェイク後に writer task の完了を待つ
    ///
    /// 呼び出し側は事前に `client.send(UpdateZakuroStop).await` で
    /// `stop_timestamp` UPDATE を送り、その後 `drop(client)` で
    /// 全 Sender を drop して writer の recv が None を返すようにしてから呼ぶ。
    pub(crate) async fn join(self) -> Result<()> {
        if let Some(h) = self.join_handle {
            h.await.map_err(|e| {
                AppError::Message(ErrorMessage::new(format!(
                    "duckdb writer task panicked: {e}"
                )))
            })?;
        }
        Ok(())
    }
}

/// VirtualClient / main 側から writer task へコマンドを送るハンドル
///
/// `clone()` で複数箇所に配れる。`sender == None` のときは disabled
/// (`--no-duckdb-output`) で全操作が no-op になる。
#[derive(Clone)]
pub(crate) struct DuckDBClient {
    sender: Option<mpsc::Sender<WriteCommand>>,
    dropped_count: Arc<AtomicU64>,
}

impl DuckDBClient {
    /// disabled 状態の noop クライアントを生成する
    pub(crate) fn noop() -> Self {
        Self {
            sender: None,
            dropped_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// DuckDB 出力が有効かどうか
    pub(crate) fn is_enabled(&self) -> bool {
        self.sender.is_some()
    }

    /// 満杯 / 無効時は drop し `dropped_count` をインクリメントする
    pub(crate) fn try_send(&self, cmd: WriteCommand) {
        if let Some(tx) = &self.sender {
            match tx.try_send(cmd) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    self.dropped_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// shutdown 経路用: 確実に送る (満杯時に待機する)
    /// 無効時は何もしない
    pub(crate) async fn send(&self, cmd: WriteCommand) {
        if let Some(tx) = &self.sender {
            let _ = tx.send(cmd).await;
        }
    }
}

/// writer task 本体の recv ループ
///
/// `recv().await` が `Some(cmd)` なら処理、`None` で break (全 Sender drop)。
/// break 後に `conn` は関数スコープ終了で自動 drop されファイルが close する
/// (DuckDB は drop 時に自動 flush するため CHECKPOINT 明示不要)。
async fn writer_run_loop(conn: Connection, mut cmd_rx: mpsc::Receiver<WriteCommand>) {
    loop {
        let Some(cmd) = cmd_rx.recv().await else {
            break;
        };
        if let Err(e) = dispatch_command(&conn, cmd) {
            rtc_log_warning!("[duckdb] write failed: error={}", e);
        }
    }
}

/// 1 コマンドを対応する INSERT / UPDATE に振り分ける
fn dispatch_command(conn: &Connection, cmd: WriteCommand) -> duckdb::Result<()> {
    match cmd {
        WriteCommand::InsertZakuro(row) => {
            insert_zakuro(conn, *row)?;
        }
        WriteCommand::UpdateZakuroStop { stop_timestamp } => {
            update_zakuro_stop(conn, stop_timestamp)?;
        }
        WriteCommand::InsertZakuroScenario(row) => {
            insert_zakuro_scenario(conn, *row)?;
        }
        WriteCommand::InsertConnection(row) => {
            insert_connection(conn, *row)?;
        }
        WriteCommand::InsertRtcStatsCodec(row) => {
            insert_rtc_stats_codec(conn, *row)?;
        }
        WriteCommand::InsertRtcStatsInboundRtp(row) => {
            insert_rtc_stats_inbound_rtp(conn, *row)?;
        }
        WriteCommand::InsertRtcStatsOutboundRtp(row) => {
            insert_rtc_stats_outbound_rtp(conn, *row)?;
        }
        WriteCommand::InsertRtcStatsMediaSource(row) => {
            insert_rtc_stats_media_source(conn, *row)?;
        }
        WriteCommand::InsertRtcStatsRemoteInboundRtp(row) => {
            insert_rtc_stats_remote_inbound_rtp(conn, *row)?;
        }
        WriteCommand::InsertRtcStatsRemoteOutboundRtp(row) => {
            insert_rtc_stats_remote_outbound_rtp(conn, *row)?;
        }
        WriteCommand::InsertRtcStatsDataChannel(row) => {
            insert_rtc_stats_data_channel(conn, *row)?;
        }
    }
    Ok(())
}

/// dropped_count を 5 秒ごとに warn 出力する reporter task
async fn reporter_loop(dropped_count: Arc<AtomicU64>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last: u64 = 0;
    loop {
        interval.tick().await;
        let total = dropped_count.load(Ordering::Relaxed);
        let delta = total - last;
        last = total;
        if delta > 0 {
            rtc_log_warning!(
                "[duckdb] dropped commands: total={}, since_last={}",
                total,
                delta
            );
        }
    }
}

// ============================================================================
// Row 構造体
// ============================================================================

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
fn system_time_to_duck(ts: SystemTime) -> DuckValue {
    let micros = ts
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime before UNIX_EPOCH is not supported")
        .as_micros() as i64;
    DuckValue::Timestamp(TimeUnit::Microsecond, micros)
}

fn insert_zakuro(conn: &Connection, row: InsertZakuroRow) -> duckdb::Result<()> {
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

fn update_zakuro_stop(conn: &Connection, stop_timestamp: SystemTime) -> duckdb::Result<()> {
    conn.execute(
        "UPDATE zakuro SET stop_timestamp = ?",
        [&system_time_to_duck(stop_timestamp) as &dyn ToSql],
    )?;
    Ok(())
}

fn insert_zakuro_scenario(conn: &Connection, row: InsertZakuroScenarioRow) -> duckdb::Result<()> {
    let urls: Vec<DuckValue> = row
        .sora_signaling_urls
        .into_iter()
        .map(DuckValue::Text)
        .collect();
    let urls_value = DuckValue::List(urls);
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

fn insert_connection(conn: &Connection, row: InsertConnectionRow) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
        &(row.vc_id as i32),
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

fn insert_rtc_stats_codec(conn: &Connection, row: RtcStatsCodecRow) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

fn insert_rtc_stats_inbound_rtp(
    conn: &Connection,
    row: RtcStatsInboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

fn insert_rtc_stats_outbound_rtp(
    conn: &Connection,
    row: RtcStatsOutboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

fn insert_rtc_stats_media_source(
    conn: &Connection,
    row: RtcStatsMediaSourceRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

fn insert_rtc_stats_remote_inbound_rtp(
    conn: &Connection,
    row: RtcStatsRemoteInboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

fn insert_rtc_stats_remote_outbound_rtp(
    conn: &Connection,
    row: RtcStatsRemoteOutboundRtpRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

fn insert_rtc_stats_data_channel(
    conn: &Connection,
    row: RtcStatsDataChannelRow,
) -> duckdb::Result<()> {
    let params: &[&dyn ToSql] = &[
        &(row.instance_id as i32),
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

// ============================================================================
// INSERT SQL 文字列定数 (列数が多いので定数に切り出し)
// ============================================================================

const INSERT_INBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_inbound_rtp (instance_id, \
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

const INSERT_OUTBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_outbound_rtp (instance_id, \
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

const INSERT_MEDIA_SOURCE_SQL: &str = "INSERT INTO rtc_stats_media_source (instance_id, \
  timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, \
  track_identifier, kind, audio_level, total_audio_energy, total_samples_duration, \
  echo_return_loss, echo_return_loss_enhancement, width, height, frames, \
  frames_per_second) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

const INSERT_REMOTE_INBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_remote_inbound_rtp \
  (instance_id, timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, \
  ssrc, kind, transport_id, codec_id, packets_received, packets_received_with_ect1, \
  packets_received_with_ce, packets_reported_as_lost, \
  packets_reported_as_lost_but_recovered, packets_lost, jitter, local_id, round_trip_time, \
  total_round_trip_time, fraction_lost, round_trip_time_measurements, \
  packets_with_bleached_ect1_marking) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

const INSERT_REMOTE_OUTBOUND_RTP_SQL: &str = "INSERT INTO rtc_stats_remote_outbound_rtp \
  (instance_id, timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, \
  ssrc, kind, transport_id, codec_id, packets_sent, bytes_sent, local_id, remote_timestamp, \
  reports_sent, round_trip_time, total_round_trip_time, round_trip_time_measurements) \
  VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

const INSERT_DATA_CHANNEL_SQL: &str = "INSERT INTO rtc_stats_data_channel (instance_id, \
  timestamp, channel_id, session_id, connection_id, rtc_timestamp, type, id, label, \
  protocol, data_channel_identifier, state, messages_sent, bytes_sent, messages_received, \
  bytes_received) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

// ============================================================================
// ファイル名生成
// ============================================================================

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
    fn fmt(&self, f: &mut JsonFormatter<'_, '_>) -> std::fmt::Result {
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
            Ok(())
        })
    })
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

// ============================================================================
// tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- A. スキーマ生成 ----

    /// 一時ディレクトリに `.db` ファイルを作りスキーマを投入するヘルパー
    fn setup_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("test.db");
        let conn = Connection::open(&path).expect("DuckDB open に失敗");
        conn.execute_batch(SCHEMA_SQL).expect("スキーマ投入に失敗");
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

    // ---- B. RTCStats JSON 振り分け ----

    #[test]
    fn dispatch_stats_inserts_known_types() {
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
        let (tx, _rx) = mpsc::channel::<WriteCommand>(64);
        let client = DuckDBClient {
            sender: Some(tx),
            dropped_count: Arc::new(AtomicU64::new(0)),
        };
        let ids = ConnectionIds {
            connection_id: "c1".into(),
            session_id: "s1".into(),
        };
        // テスト並列実行時に他テストが UNKNOWN_TYPES に干渉するのを避けるため、
        // clear は行わず投入前後の差分で検証する
        let unique = format!("unknown-type-{}-{}", module_path!(), line!());
        let stats = format!(r#"[{{"type":"{unique}","id":"X1","timestamp":1.0}}]"#);
        let size_before = unknown_types_size_for_test();
        // 100 回投入しても集合には 1 つだけ追加される
        for _ in 0..100 {
            dispatch_stats(0, 0, "ch", &ids, &client, &stats, SystemTime::now());
        }
        let size_after = unknown_types_size_for_test();
        assert_eq!(
            size_after - size_before,
            1,
            "同一未知 type は集合に 1 つだけ追加されるべき (before={}, after={})",
            size_before,
            size_after
        );
    }

    // ---- C. zakuro テーブル shutdown フロー ----

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
