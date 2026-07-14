// ============================================================================
// Config / Writer / Client
// ============================================================================

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use duckdb::Connection;
use shiguredo_webrtc::{rtc_log_info, rtc_log_warning};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, ErrorMessage, Result};

use super::module::CHANNEL_CAPACITY;
use super::rows::{
    WriteCommand, insert_connection, insert_rtc_stats_codec, insert_rtc_stats_data_channel,
    insert_rtc_stats_inbound_rtp, insert_rtc_stats_media_source, insert_rtc_stats_outbound_rtp,
    insert_rtc_stats_remote_inbound_rtp, insert_rtc_stats_remote_outbound_rtp, insert_zakuro,
    insert_zakuro_scenario, update_zakuro_stop,
};
use super::schema::SCHEMA_SQL;

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
    reporter_handle: Option<tokio::task::JoinHandle<()>>,
    reporter_token: CancellationToken,
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
                    reporter_handle: None,
                    reporter_token: CancellationToken::new(),
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
        let reporter_token = CancellationToken::new();
        let reporter_handle = tokio::spawn(reporter_loop(reporter_dropped, reporter_token.clone()));

        Ok((
            Self {
                join_handle: Some(join_handle),
                reporter_handle: Some(reporter_handle),
                reporter_token,
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
    /// main 側の Sender を drop してから呼ぶ。
    ///
    /// 本メソッド自身も `DuckDBStatsWriter` が保持する `client` (Sender) を
    /// 先に drop する。これをしないとチャネルが閉じず writer の
    /// `recv().await` が永久に戻り、`join` がハングする。
    pub(crate) async fn join(self) -> Result<()> {
        // writer 終了条件は「全 Sender drop」。Self 内の client も Sender を持つ。
        drop(self.client);

        if let Some(h) = self.join_handle {
            h.await.map_err(|e| {
                AppError::Message(ErrorMessage::new(format!(
                    "duckdb writer task panicked: {e}"
                )))
            })?;
        }
        // reporter task を停止する
        self.reporter_token.cancel();
        if let Some(h) = self.reporter_handle {
            let _ = h.await;
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
    pub(crate) sender: Option<mpsc::Sender<WriteCommand>>,
    pub(crate) dropped_count: Arc<AtomicU64>,
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
        if let Some(tx) = &self.sender
            && let Err(e) = tx.send(cmd).await
        {
            rtc_log_warning!("[duckdb] send failed: channel closed, error={e}");
        }
    }
}

/// INSERT 連続エラーの閾値。超えたら writer を停止する
const MAX_CONSECUTIVE_ERRORS: u32 = 10;

/// writer task 本体の recv ループ
///
/// `recv().await` が `Some(cmd)` なら処理、`None` で break (全 Sender drop)。
/// break 後に `conn` は関数スコープ終了で自動 drop されファイルが close する
/// (DuckDB は drop 時に自動 flush するため CHECKPOINT 明示不要)。
async fn writer_run_loop(conn: Connection, mut cmd_rx: mpsc::Receiver<WriteCommand>) {
    let mut consecutive_errors: u32 = 0;
    loop {
        let Some(cmd) = cmd_rx.recv().await else {
            break;
        };
        if let Err(e) = dispatch_command(&conn, cmd) {
            consecutive_errors += 1;
            rtc_log_warning!("[duckdb] write failed: error={e}, consecutive={consecutive_errors}");
            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                rtc_log_warning!(
                    "[duckdb] too many consecutive write errors ({consecutive_errors}), stopping writer"
                );
                break;
            }
        } else {
            consecutive_errors = 0;
        }
    }
}

/// 1 コマンドを対応する INSERT / UPDATE に振り分ける
pub(crate) fn dispatch_command(conn: &Connection, cmd: WriteCommand) -> duckdb::Result<()> {
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
async fn reporter_loop(dropped_count: Arc<AtomicU64>, token: CancellationToken) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last: u64 = 0;
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = interval.tick() => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::SystemTime;

    use duckdb::Connection;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use crate::duckdb_stats::rows::{RtcStatsCodecRow, WriteCommand};
    use crate::duckdb_stats::schema::SCHEMA_SQL;

    /// 一時ディレクトリに `.db` ファイルを作りスキーマを投入するヘルパー
    fn setup_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("test.db");
        let conn = Connection::open(&path).expect("DuckDB open に失敗");
        conn.execute_batch(SCHEMA_SQL).expect("スキーマ投入に失敗");
        (dir, conn)
    }

    #[test]
    fn reporter_loop_stops_on_cancellation() {
        let rt = tokio::runtime::Runtime::new().expect("runtime を作成できること");
        rt.block_on(async {
            let token = CancellationToken::new();
            let dropped_count = Arc::new(AtomicU64::new(0));
            let handle = tokio::spawn(reporter_loop(dropped_count, token.clone()));
            token.cancel();
            handle
                .await
                .expect("reporter_loop が CancellationToken で停止すること");
        });
    }

    #[test]
    fn send_logs_error_on_closed_channel() {
        let rt = tokio::runtime::Runtime::new().expect("runtime を作成できること");
        rt.block_on(async {
            let (tx, rx) = mpsc::channel::<WriteCommand>(1);
            drop(rx);
            let client = DuckDBClient {
                sender: Some(tx),
                dropped_count: Arc::new(AtomicU64::new(0)),
            };
            // パニックせず正常に終了すること
            client
                .send(WriteCommand::UpdateZakuroStop {
                    stop_timestamp: SystemTime::now(),
                })
                .await;
        });
    }

    /// `DuckDBStatsWriter` が保持する Sender を join 時に drop しないと
    /// writer の recv が閉じずハングする。その回帰を防ぐ。
    #[test]
    fn join_completes_after_external_client_dropped() {
        let rt = tokio::runtime::Runtime::new().expect("runtime を作成できること");
        rt.block_on(async {
            let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
            let db_path = dir.path().join("join_test.db");
            let (writer, _version) = DuckDBStatsWriter::start(DuckDBWriterConfig {
                db_path,
                interval: Duration::from_secs(1),
                enabled: true,
            })
            .await
            .expect("DuckDBStatsWriter の起動に成功すること");

            // main 側と同じく外部 client を drop してから join する。
            // writer 内部の client も join 内で drop されないとここでハングする。
            let client = writer.client();
            drop(client);
            tokio::time::timeout(Duration::from_secs(5), writer.join())
                .await
                .expect("join がタイムアウトしないこと")
                .expect("join が成功すること");
        });
    }

    #[test]
    fn writer_run_loop_stops_after_consecutive_errors() {
        let rt = tokio::runtime::Runtime::new().expect("runtime を作成できること");
        rt.block_on(async {
            let (_dir, conn) = setup_db();
            // テーブルを削除して書き込みを失敗させる
            conn.execute_batch("DROP TABLE rtc_stats_codec")
                .expect("DROP 失敗");
            let (tx, rx) = mpsc::channel::<WriteCommand>(64);
            // InsertZakuro は zakuro テーブル (削除していないので成功)、
            // その後 InsertRtcStatsCodec を連続送信してエラーを蓄積させる
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                // 連続エラー閾値 (MAX_CONSECUTIVE_ERRORS = 10) を超えるまで送る
                for _ in 0..15 {
                    let _ = tx_clone
                        .send(WriteCommand::InsertRtcStatsCodec(Box::new(
                            RtcStatsCodecRow {
                                instance_id: 0,
                                timestamp: SystemTime::now(),
                                channel_id: "ch".into(),
                                session_id: "s".into(),
                                connection_id: "c".into(),
                                rtc_timestamp: Some(1.0),
                                stats_type: "codec".into(),
                                id: "X".into(),
                                mime_type: None,
                                payload_type: None,
                                clock_rate: None,
                                channels: None,
                                sdp_fmtp_line: None,
                            },
                        )))
                        .await;
                }
                drop(tx_clone);
            });
            // drop で全 sender が消える前に writer 側が連続エラーで停止することを確認
            writer_run_loop(conn, rx).await;
            // writer_run_loop から正常に抜ければパニックしていない
        });
    }
}
