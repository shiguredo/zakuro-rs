// ============================================================================
// Config / Writer / Client
// ============================================================================

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use duckdb::{Appender, Connection};
use shiguredo_webrtc::{rtc_log_info, rtc_log_warning};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, ErrorMessage, Result};

use super::module::STATS_CHANNEL_CAPACITY;
use super::rows::{
    StatsSample, WriteCommand, append_rtc_stats_data_channel, append_rtc_stats_inbound_rtp,
    append_rtc_stats_media_source, append_rtc_stats_outbound_rtp,
    append_rtc_stats_remote_inbound_rtp, append_rtc_stats_remote_outbound_rtp, insert_connection,
    insert_connection_lifecycle, insert_rtc_stats_codec, insert_zakuro, insert_zakuro_scenario,
    update_zakuro_stop,
};
use super::schema::{
    INSERT_DATA_CHANNEL_SQL, INSERT_INBOUND_RTP_SQL, INSERT_MEDIA_SOURCE_SQL,
    INSERT_OUTBOUND_RTP_SQL, INSERT_REMOTE_INBOUND_RTP_SQL, INSERT_REMOTE_OUTBOUND_RTP_SQL,
    SCHEMA_SQL, insert_sql_columns,
};

/// 統計サンプルを 1 トランザクションにまとめる待ち時間
///
/// 1 秒間隔の tick を接続横断で 1 回のバルク INSERT にする。
/// この窓より短い間隔で同じ接続のサンプルが 2 回到着することは通常ない。
const FLUSH_INTERVAL: Duration = Duration::from_millis(200);

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
/// `start` で writer task を起動し、`client` 経由でコマンドを送る。
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
        // 制御コマンドは接続数に比例するだけなので、同期コンテキスト (シグナリングコールバック)
        // から待たずに送れる unbounded にする。統計サンプルの洪水で埋まらない。
        let (control_tx, control_rx) = mpsc::unbounded_channel::<WriteCommand>();
        let (stats_tx, stats_rx) = mpsc::channel::<StatsSample>(STATS_CHANNEL_CAPACITY);
        let dropped_count = Arc::new(AtomicU64::new(0));
        let dropped_for_writer = Arc::clone(&dropped_count);
        let db_path = config.db_path.clone();
        let interval = config.interval;

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
                interval.as_secs_f64()
            );

            // writer 本体: Handle::current().block_on で recv ループを回す
            let handle = tokio::runtime::Handle::current();
            let _entered = handle.enter();
            handle.block_on(writer_run_loop(
                conn,
                control_rx,
                stats_rx,
                dropped_for_writer,
            ));

            rtc_log_info!("[duckdb] writer stopped");
        });

        // init readiness を待つ (spawn_blocking 側で init_tx.send が呼ばれるまで await)
        let duckdb_version = init_rx.await.map_err(|_| {
            AppError::Message(ErrorMessage::new("duckdb writer aborted during init"))
        })??;

        let client = DuckDBClient {
            control: Some(control_tx),
            stats: Some(stats_tx),
            dropped_count,
        };

        // reporter task (dropped_count の定期 warn) は writer 本体とは別 task に分離する
        // (writer 本体 select に並べると最大スケール時に reporter が starvation するため)
        let reporter_dropped = Arc::clone(&client.dropped_count);
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
    /// 呼び出し側は事前に `client.send_control(UpdateZakuroStop)` で
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
/// `clone()` で複数箇所に配れる。`control == None` のときは disabled
/// (`--no-duckdb-output`) で全操作が no-op になる。
#[derive(Clone)]
pub(crate) struct DuckDBClient {
    /// 接続行、ライフサイクル、codec、起動情報。unbounded。
    pub(crate) control: Option<mpsc::UnboundedSender<WriteCommand>>,
    /// 接続 1 本 × 1 tick の統計サンプル。
    pub(crate) stats: Option<mpsc::Sender<StatsSample>>,
    /// 書けなかった統計サンプル数 (tick 単位。行数ではない)
    pub(crate) dropped_count: Arc<AtomicU64>,
}

impl DuckDBClient {
    /// disabled 状態の noop クライアントを生成する
    pub(crate) fn noop() -> Self {
        Self {
            control: None,
            stats: None,
            dropped_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// DuckDB 出力が有効かどうか
    pub(crate) fn is_enabled(&self) -> bool {
        self.control.is_some()
    }

    /// 制御コマンドを送る
    ///
    /// チャネルは unbounded なので、receiver が生きていれば欠落しない。
    /// 無効時は何もしない。
    pub(crate) fn send_control(&self, cmd: WriteCommand) {
        if let Some(tx) = &self.control
            && let Err(e) = tx.send(cmd)
        {
            rtc_log_warning!("[duckdb] control send failed: channel closed, error={e}");
        }
    }

    /// 統計サンプルを 1 tick ぶん送る
    ///
    /// 満杯または receiver が閉じているときはサンプル全体を捨て、
    /// `dropped_count` を 1 増やす。行単位では捨てない。
    pub(crate) fn try_send_stats(&self, sample: StatsSample) {
        let Some(tx) = &self.stats else {
            return;
        };
        if tx.try_send(sample).is_err() {
            self.dropped_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// INSERT 連続エラーの閾値。超えたら writer を停止する
const MAX_CONSECUTIVE_ERRORS: u32 = 10;

/// writer task 本体の受信ループ
///
/// 制御コマンドは欠落させない。統計サンプルは短い窓でまとめ、同じ接続の
/// 未書き込みサンプルが複数あれば最新だけを残す。フラッシュはテーブルごとの
/// Appender (バルク INSERT) で 1 トランザクションに入れる。
///
/// 両方の Sender が drop されるとループを抜ける。抜けたあと `conn` は
/// 関数スコープ終了で自動 drop され、ファイルが close する
/// (DuckDB は drop 時に自動 flush するため CHECKPOINT 明示不要)。
async fn writer_run_loop(
    conn: Connection,
    mut control_rx: mpsc::UnboundedReceiver<WriteCommand>,
    mut stats_rx: mpsc::Receiver<StatsSample>,
    dropped_count: Arc<AtomicU64>,
) {
    let mut consecutive_errors: u32 = 0;
    let mut control_open = true;
    let mut stats_open = true;

    loop {
        if !control_open && !stats_open {
            break;
        }

        let mut controls: Vec<WriteCommand> = Vec::new();
        let mut pending: HashMap<(u32, u32), StatsSample> = HashMap::new();
        let mut replaced: u64 = 0;

        tokio::select! {
            biased;
            cmd = control_rx.recv(), if control_open => {
                push_control(&mut controls, &mut control_open, cmd);
            }
            sample = stats_rx.recv(), if stats_open => {
                push_sample(&mut pending, &mut replaced, &mut stats_open, sample);
            }
        }

        if controls.is_empty() && pending.is_empty() {
            continue;
        }

        drain_available(
            &mut control_rx,
            &mut stats_rx,
            &mut control_open,
            &mut stats_open,
            &mut controls,
            &mut pending,
            &mut replaced,
        );

        if control_open || stats_open {
            let deadline = Instant::now() + FLUSH_INTERVAL;
            loop {
                let rest = deadline.saturating_duration_since(Instant::now());
                if rest.is_zero() {
                    break;
                }
                tokio::select! {
                    biased;
                    cmd = control_rx.recv(), if control_open => {
                        push_control(&mut controls, &mut control_open, cmd);
                    }
                    sample = stats_rx.recv(), if stats_open => {
                        push_sample(&mut pending, &mut replaced, &mut stats_open, sample);
                    }
                    _ = tokio::time::sleep(rest) => break,
                }
                if !control_open && !stats_open {
                    break;
                }
            }
            drain_available(
                &mut control_rx,
                &mut stats_rx,
                &mut control_open,
                &mut stats_open,
                &mut controls,
                &mut pending,
                &mut replaced,
            );
        }

        if controls.is_empty() && pending.values().all(|sample| !sample.has_rows()) {
            continue;
        }

        if let Err(e) = flush_writes(&conn, &controls, &pending) {
            consecutive_errors += 1;
            // フラッシュに失敗したサンプルは書き直さない。欠落として数える。
            let lost = pending.len() as u64;
            if lost > 0 {
                dropped_count.fetch_add(lost, Ordering::Relaxed);
            }
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
        if replaced > 0 {
            dropped_count.fetch_add(replaced, Ordering::Relaxed);
        }
    }
}

/// すでに届いているコマンドを、待たずに全部取り出す
fn drain_available(
    control_rx: &mut mpsc::UnboundedReceiver<WriteCommand>,
    stats_rx: &mut mpsc::Receiver<StatsSample>,
    control_open: &mut bool,
    stats_open: &mut bool,
    controls: &mut Vec<WriteCommand>,
    pending: &mut HashMap<(u32, u32), StatsSample>,
    replaced: &mut u64,
) {
    if *control_open {
        loop {
            match control_rx.try_recv() {
                Ok(cmd) => controls.push(cmd),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    *control_open = false;
                    break;
                }
            }
        }
    }
    if *stats_open {
        loop {
            match stats_rx.try_recv() {
                Ok(sample) => {
                    if absorb_sample(pending, sample) {
                        *replaced += 1;
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    *stats_open = false;
                    break;
                }
            }
        }
    }
}

fn push_control(
    controls: &mut Vec<WriteCommand>,
    control_open: &mut bool,
    cmd: Option<WriteCommand>,
) {
    match cmd {
        Some(cmd) => controls.push(cmd),
        None => *control_open = false,
    }
}

fn push_sample(
    pending: &mut HashMap<(u32, u32), StatsSample>,
    replaced: &mut u64,
    stats_open: &mut bool,
    sample: Option<StatsSample>,
) {
    match sample {
        Some(sample) => {
            if absorb_sample(pending, sample) {
                *replaced += 1;
            }
        }
        None => *stats_open = false,
    }
}

/// 同じ接続の未書き込みサンプルを最新で置き換える
///
/// 戻り値は、置き換えで 1 サンプル捨てたか。
fn absorb_sample(pending: &mut HashMap<(u32, u32), StatsSample>, sample: StatsSample) -> bool {
    let key = (sample.instance_id, sample.vc_id);
    pending.insert(key, sample).is_some()
}

/// 制御コマンドと統計サンプルを 1 トランザクションで書く
///
/// 統計テーブルは Appender でバルク INSERT する。`pk` は列リストから外し、
/// シーケンスの DEFAULT に任せる。
pub(crate) fn flush_writes(
    conn: &Connection,
    controls: &[WriteCommand],
    samples: &HashMap<(u32, u32), StatsSample>,
) -> duckdb::Result<()> {
    if controls.is_empty() && samples.values().all(|sample| !sample.has_rows()) {
        return Ok(());
    }
    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> duckdb::Result<()> {
        for command in controls {
            dispatch_command(conn, command)?;
        }
        append_samples(conn, samples)?;
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(error) => {
            if let Err(rollback_error) = conn.execute_batch("ROLLBACK") {
                rtc_log_warning!("[duckdb] rollback failed: error={rollback_error}");
            }
            return Err(error);
        }
    }
    Ok(())
}

/// 1 コマンドを対応する INSERT / UPDATE に振り分ける
fn dispatch_command(conn: &Connection, cmd: &WriteCommand) -> duckdb::Result<()> {
    match cmd {
        WriteCommand::InsertZakuro(row) => {
            insert_zakuro(conn, (**row).clone())?;
        }
        WriteCommand::UpdateZakuroStop { stop_timestamp } => {
            update_zakuro_stop(conn, *stop_timestamp)?;
        }
        WriteCommand::InsertZakuroScenario(row) => {
            insert_zakuro_scenario(conn, (**row).clone())?;
        }
        WriteCommand::InsertConnection(row) => {
            insert_connection(conn, (**row).clone())?;
        }
        WriteCommand::InsertConnectionLifecycle(row) => {
            insert_connection_lifecycle(conn, (**row).clone())?;
        }
        WriteCommand::InsertRtcStatsCodec(row) => {
            insert_rtc_stats_codec(conn, (**row).clone())?;
        }
    }
    Ok(())
}

/// 統計サンプルをテーブルごとに Appender へ流す
fn append_samples(
    conn: &Connection,
    samples: &HashMap<(u32, u32), StatsSample>,
) -> duckdb::Result<()> {
    append_rows(
        conn,
        "rtc_stats_inbound_rtp",
        INSERT_INBOUND_RTP_SQL,
        samples.values().flat_map(|sample| sample.inbound.iter()),
        append_rtc_stats_inbound_rtp,
    )?;
    append_rows(
        conn,
        "rtc_stats_outbound_rtp",
        INSERT_OUTBOUND_RTP_SQL,
        samples.values().flat_map(|sample| sample.outbound.iter()),
        append_rtc_stats_outbound_rtp,
    )?;
    append_rows(
        conn,
        "rtc_stats_media_source",
        INSERT_MEDIA_SOURCE_SQL,
        samples
            .values()
            .flat_map(|sample| sample.media_source.iter()),
        append_rtc_stats_media_source,
    )?;
    append_rows(
        conn,
        "rtc_stats_remote_inbound_rtp",
        INSERT_REMOTE_INBOUND_RTP_SQL,
        samples
            .values()
            .flat_map(|sample| sample.remote_inbound.iter()),
        append_rtc_stats_remote_inbound_rtp,
    )?;
    append_rows(
        conn,
        "rtc_stats_remote_outbound_rtp",
        INSERT_REMOTE_OUTBOUND_RTP_SQL,
        samples
            .values()
            .flat_map(|sample| sample.remote_outbound.iter()),
        append_rtc_stats_remote_outbound_rtp,
    )?;
    append_rows(
        conn,
        "rtc_stats_data_channel",
        INSERT_DATA_CHANNEL_SQL,
        samples
            .values()
            .flat_map(|sample| sample.data_channel.iter()),
        append_rtc_stats_data_channel,
    )?;
    Ok(())
}

/// 1 テーブル分を Appender で書く。行が無ければ何もしない
fn append_rows<'a, T: 'a>(
    conn: &Connection,
    table: &str,
    insert_sql: &str,
    rows: impl Iterator<Item = &'a T>,
    mut append_one: impl FnMut(&mut Appender<'_>, &T) -> duckdb::Result<()>,
) -> duckdb::Result<()> {
    let mut rows = rows.peekable();
    if rows.peek().is_none() {
        return Ok(());
    }
    let columns = insert_sql_columns(insert_sql);
    let column_refs: Vec<&str> = columns.iter().map(String::as_str).collect();
    let mut appender = conn.appender_with_columns(table, &column_refs)?;
    for row in rows {
        append_one(&mut appender, row)?;
    }
    appender.flush()
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
                        "[duckdb] dropped stats samples: total={}, since_last={}",
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

    use crate::duckdb_stats::rows::{ConnectionIds, RtcStatsCodecRow, WriteCommand};
    use crate::duckdb_stats::schema::SCHEMA_SQL;
    use crate::duckdb_stats::stats_json::parse_rtc_stats;

    /// 一時ディレクトリに `.db` ファイルを作りスキーマを投入するヘルパー
    fn setup_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("test.db");
        let conn = Connection::open(&path).expect("DuckDB open に失敗");
        conn.execute_batch(SCHEMA_SQL).expect("スキーマ投入に失敗");
        (dir, conn)
    }

    /// packetsReceived だけ違う inbound-rtp サンプルを作る
    fn inbound_sample(instance_id: u32, vc_id: u32, packets: i64) -> StatsSample {
        let stats = format!(
            r#"[{{"type":"inbound-rtp","id":"I1","timestamp":1.0,"ssrc":1,"kind":"video","packetsReceived":{packets}}}]"#
        );
        let ids = ConnectionIds {
            connection_id: format!("c-{instance_id}-{vc_id}"),
            session_id: "s".into(),
        };
        parse_rtc_stats(
            instance_id,
            vc_id,
            "ch",
            &ids,
            &stats,
            SystemTime::UNIX_EPOCH,
        )
        .sample
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
    fn send_control_logs_error_on_closed_channel() {
        let (tx, rx) = mpsc::unbounded_channel::<WriteCommand>();
        drop(rx);
        let client = DuckDBClient {
            control: Some(tx),
            stats: None,
            dropped_count: Arc::new(AtomicU64::new(0)),
        };
        // パニックせず正常に終了すること
        client.send_control(WriteCommand::UpdateZakuroStop {
            stop_timestamp: SystemTime::now(),
        });
    }

    /// 統計チャネルが満杯でも、制御コマンドは別チャネルなので送れる
    #[test]
    fn control_send_succeeds_while_stats_channel_is_full() {
        let (control_tx, mut control_rx) = mpsc::unbounded_channel::<WriteCommand>();
        let (stats_tx, _stats_rx) = mpsc::channel::<StatsSample>(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let client = DuckDBClient {
            control: Some(control_tx),
            stats: Some(stats_tx),
            dropped_count: Arc::clone(&dropped),
        };
        client.try_send_stats(StatsSample::empty(0, 0));
        client.try_send_stats(StatsSample::empty(0, 1));
        assert_eq!(
            dropped.load(Ordering::Relaxed),
            1,
            "満杯で捨てる単位はサンプル 1 つであること"
        );
        client.send_control(WriteCommand::UpdateZakuroStop {
            stop_timestamp: SystemTime::UNIX_EPOCH,
        });
        assert!(
            control_rx.try_recv().is_ok(),
            "統計チャネルが満杯でも制御コマンドは届くこと"
        );
    }

    /// 同じ接続のサンプルを 2 つ渡すと、後の方だけが残る
    #[test]
    fn absorb_sample_keeps_the_latest_per_connection() {
        let mut pending = HashMap::new();
        assert!(
            !absorb_sample(&mut pending, inbound_sample(0, 1, 10)),
            "初回の挿入は置き換えではないこと"
        );
        assert!(
            absorb_sample(&mut pending, inbound_sample(0, 1, 77)),
            "同じ接続の 2 回目は置き換えであること"
        );
        assert!(
            !absorb_sample(&mut pending, inbound_sample(0, 2, 5)),
            "別の接続は置き換えではないこと"
        );
        assert_eq!(pending.len(), 2, "接続 2 本分が残ること");
        let kept = pending.get(&(0, 1)).expect("接続 (0, 1) が残ること");
        assert_eq!(
            kept.inbound[0].packets_received,
            Some(77),
            "残るのは後から入れたサンプルであること"
        );
    }

    /// 複数接続の inbound 行が 1 回のバルク INSERT で全部残る
    #[test]
    fn flush_writes_appends_every_connection() {
        let (_dir, conn) = setup_db();
        let mut pending = HashMap::new();
        absorb_sample(&mut pending, inbound_sample(0, 1, 10));
        absorb_sample(&mut pending, inbound_sample(0, 2, 20));
        absorb_sample(&mut pending, inbound_sample(0, 1, 30));
        flush_writes(&conn, &[], &pending).expect("バルク INSERT が成功すること");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM rtc_stats_inbound_rtp", [], |row| {
                row.get(0)
            })
            .expect("行数の取得に失敗");
        assert_eq!(count, 2, "置き換え後の 2 接続ぶんだけ書かれること");
        let packets: i64 = conn
            .query_row(
                "SELECT packets_received FROM rtc_stats_inbound_rtp WHERE connection_id = 'c-0-1'",
                [],
                |row| row.get(0),
            )
            .expect("packets_received の取得に失敗");
        assert_eq!(packets, 30, "同じ接続は最新サンプルだけが書かれること");
        let pk: i64 = conn
            .query_row(
                "SELECT pk FROM rtc_stats_inbound_rtp ORDER BY pk LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("pk の取得に失敗");
        assert!(pk > 0, "pk はシーケンスで採番されること");
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
            let (control_tx, control_rx) = mpsc::unbounded_channel::<WriteCommand>();
            let (stats_tx, stats_rx) = mpsc::channel::<StatsSample>(8);
            drop(stats_tx);
            let tx_clone = control_tx.clone();
            tokio::spawn(async move {
                // 同じフラッシュにまとまって 1 回失敗しても、ループが戻ることを見る。
                for _ in 0..15 {
                    let _ = tx_clone.send(WriteCommand::InsertRtcStatsCodec(Box::new(
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
                    )));
                }
                drop(tx_clone);
            });
            drop(control_tx);
            writer_run_loop(conn, control_rx, stats_rx, Arc::new(AtomicU64::new(0))).await;
            // writer_run_loop から正常に抜ければパニックしていない
        });
    }

    /// writer ループは、溜まった同一接続のサンプルを最新だけ書く
    #[test]
    fn writer_run_loop_coalesces_queued_samples() {
        let rt = tokio::runtime::Runtime::new().expect("runtime を作成できること");
        rt.block_on(async {
            let (dir, conn) = setup_db();
            drop(conn);
            let path = dir.path().join("coalesce.db");
            // setup_db とは別ファイルにする。ループに渡した接続を閉じたあとに読み返す。
            let conn = Connection::open(&path).expect("DuckDB open に失敗");
            conn.execute_batch(SCHEMA_SQL).expect("スキーマ投入に失敗");
            let (control_tx, control_rx) = mpsc::unbounded_channel::<WriteCommand>();
            let (stats_tx, stats_rx) = mpsc::channel::<StatsSample>(8);
            stats_tx
                .try_send(inbound_sample(0, 1, 10))
                .expect("1 つ目のサンプルを送れること");
            stats_tx
                .try_send(inbound_sample(0, 1, 77))
                .expect("2 つ目のサンプルを送れること");
            drop(stats_tx);
            drop(control_tx);
            writer_run_loop(conn, control_rx, stats_rx, Arc::new(AtomicU64::new(0))).await;

            let conn = Connection::open(&path).expect("書き込み後の open に失敗");
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM rtc_stats_inbound_rtp", [], |row| {
                    row.get(0)
                })
                .expect("行数の取得に失敗");
            assert_eq!(count, 1, "同一接続のキューは 1 行に畳まれること");
            let packets: i64 = conn
                .query_row(
                    "SELECT packets_received FROM rtc_stats_inbound_rtp",
                    [],
                    |row| row.get(0),
                )
                .expect("packets_received の取得に失敗");
            assert_eq!(packets, 77, "残るのは後からキューしたサンプルであること");
        });
    }
}
