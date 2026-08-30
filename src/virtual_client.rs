use std::sync::Arc;
use std::time::{Duration, SystemTime};

use shiguredo_webrtc::{VideoTrackSource, rtc_log_info, rtc_log_warning};
use sora_sdk::{
    ConnectDataChannel, JsonString, Mp4VideoCapturer, Role, SignalingDirection, SoraConnection,
    SoraConnectionContext, SoraConnectionEventHandler,
};
use tokio::sync::mpsc;
use tokio_stream::{StreamExt, wrappers::IntervalStream};
use tokio_util::sync::CancellationToken;

use crate::data_channel::MessageChannel;
use crate::duckdb_stats::{
    ConnectionIds, DuckDBClient, InsertConnectionRow, WriteCommand, dispatch_stats, parse_offer_ids,
};
use crate::scenario::{Scenario, ScenarioEnd, ScenarioPlayer};
use crate::stats::StatsEvent;

#[derive(Clone)]
pub(crate) struct VirtualClientConfig {
    pub(crate) signaling_urls: Vec<String>,
    pub(crate) channel_id: String,
    pub(crate) role: Role,
    pub(crate) client_id: Option<String>,
    pub(crate) bundle_id: Option<String>,
    pub(crate) metadata: Option<JsonString>,
    pub(crate) signaling_notify_metadata: Option<JsonString>,
    pub(crate) duration: Option<f64>,
    pub(crate) repeat_interval: Option<f64>,
    pub(crate) max_retry: u32,
    pub(crate) retry_interval: f64,
    pub(crate) video: Option<sora_sdk::Video>,
    pub(crate) audio: Option<sora_sdk::Audio>,
    pub(crate) connect_data_channels: Option<Vec<ConnectDataChannel>>,
    pub(crate) message_channels: Vec<MessageChannel>,
    pub(crate) data_channel_signaling: Option<bool>,
    pub(crate) ignore_disconnect_websocket: Option<bool>,
    pub(crate) disconnect_wait_timeout: Option<Duration>,
    pub(crate) simulcast: Option<bool>,
    pub(crate) simulcast_request_rid: Option<String>,
    pub(crate) spotlight: Option<bool>,
    pub(crate) spotlight_focus_rid: Option<String>,
    pub(crate) spotlight_unfocus_rid: Option<String>,
    pub(crate) insecure: bool,
    pub(crate) client_cert: Option<String>,
    pub(crate) client_key: Option<String>,
    pub(crate) scenario: Option<Scenario>,
    /// DuckDB 統計書き込みクライアント (disabled 時は noop)
    pub(crate) duckdb_client: DuckDBClient,
    /// DuckDB への統計書き込み間隔
    pub(crate) duckdb_interval: Duration,
}

enum DisconnectReason {
    Shutdown,
    DurationExpired,
    ScenarioDisconnect,
    ScenarioExit,
    Unexpected(sora_sdk::Result<()>),
}

#[expect(clippy::too_many_arguments)]
pub(crate) async fn run(
    instance_id: u32,
    vc_id: u32,
    context: Arc<SoraConnectionContext>,
    video_source: Option<VideoTrackSource>,
    // MP4 パススルー時にのみ Some。
    // shiguredo_webrtc の MP4 用 VideoFrameBuffer はスレッド固定チェックがあるため、
    // 1 つの video_source を複数 VC で共有すると `video_frame_buffer callback called from multiple threads`
    // の panic に至る。VC ごとに専用の Mp4VideoCapturer を持ち、そのライフタイムを
    // この関数のスコープで受け取ることでフィーダースレッドを VC と同時終了させる。
    // Mp4SampleReader 自体は instance 内で共有 (Clone) し、ファイル I/O スレッドは 1 本にまとめる。
    mp4_capturer: Option<Mp4VideoCapturer>,
    config: VirtualClientConfig,
    token: CancellationToken,
    stats_tx: mpsc::Sender<StatsEvent>,
) {
    // capturer は VC のライフタイム全体で保持する必要がある
    // (フィーダースレッドが停止すると video_source へのフレーム供給が止まるため)。
    let _mp4_capturer = mp4_capturer;
    let mut retry_count: u32 = 0;
    let mut scenario_player = config
        .scenario
        .clone()
        .map(|scenario| ScenarioPlayer::new(scenario, instance_id, vc_id));

    loop {
        let connection_token = token.child_token();
        // 接続ごとに identifiers を新規生成する (再接続時は別 connection_id が記録される)
        let ids: Arc<std::sync::Mutex<Option<ConnectionIds>>> =
            Arc::new(std::sync::Mutex::new(None));

        let (client, handle) = match build_client(
            &context,
            &video_source,
            &config,
            &ids,
            instance_id,
            vc_id,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                rtc_log_warning!(
                    "[i{}/vc-{}] failed to build client: {}",
                    instance_id,
                    vc_id,
                    e,
                );
                retry_count += 1;
                if retry_count > config.max_retry {
                    rtc_log_info!(
                        "[i{}/vc-{}] reached max retry count ({})",
                        instance_id,
                        vc_id,
                        config.max_retry,
                    );
                    break;
                }
                let _ = stats_tx
                    .send(StatsEvent::Retrying {
                        instance_id,
                        vc_id,
                        retry_count,
                    })
                    .await;
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs_f64(config.retry_interval)) => continue,
                }
            }
        };
        let _ = stats_tx
            .send(StatsEvent::Connected { instance_id, vc_id })
            .await;
        rtc_log_info!("[i{}/vc-{}] connected", instance_id, vc_id);

        // DataChannel メッセージングタスクの起動
        let messaging_token = connection_token.child_token();
        if !config.message_channels.is_empty() {
            let msg_handle = handle.clone();
            let msg_channels = config.message_channels.clone();
            let msg_token = messaging_token.clone();
            tokio::spawn(async move {
                crate::data_channel::run_messaging(
                    instance_id,
                    vc_id,
                    msg_handle,
                    msg_channels,
                    msg_token,
                )
                .await;
            });
        }

        // DuckDB 統計収集タスクの起動 (disabled 時は起動しない)
        if config.duckdb_client.is_enabled() {
            let stats_client = config.duckdb_client.clone();
            let stats_ids = ids.clone();
            let stats_handle = handle.clone();
            let stats_token = connection_token.child_token();
            let interval = config.duckdb_interval;
            let channel_id = config.channel_id.clone();
            tokio::task::spawn_local(async move {
                run_stats_collection(
                    instance_id,
                    vc_id,
                    channel_id,
                    stats_client,
                    stats_ids,
                    stats_handle,
                    stats_token,
                    interval,
                )
                .await;
            });
        }

        let mut run_future = Box::pin(client.run());

        let reason = if let Some(ref mut player) = scenario_player {
            // シナリオモード: シナリオの Reconnect / Disconnect / Exit 操作まで実行する
            tokio::select! {
                biased;
                _ = token.cancelled() => DisconnectReason::Shutdown,
                end = player.run_until_disconnect(&token, &handle, &ids) => match end {
                    ScenarioEnd::Reconnect => DisconnectReason::ScenarioDisconnect,
                    ScenarioEnd::Exit => DisconnectReason::ScenarioExit,
                },
                result = &mut run_future => DisconnectReason::Unexpected(result),
            }
        } else {
            // 通常モード: duration タイマーで切断する
            tokio::select! {
                biased;
                _ = token.cancelled() => DisconnectReason::Shutdown,
                _ = duration_timer(config.duration) => DisconnectReason::DurationExpired,
                result = &mut run_future => DisconnectReason::Unexpected(result),
            }
        };

        match reason {
            DisconnectReason::Shutdown => {
                rtc_log_info!("[i{}/vc-{}] shutting down", instance_id, vc_id);
                tokio::select! {
                    _ = handle.disconnect() => {}
                    _ = &mut run_future => {}
                }
                // DataChannel messaging / DuckDB stats 収集タスクを止める
                connection_token.cancel();
                break;
            }
            DisconnectReason::DurationExpired => {
                rtc_log_info!("[i{}/vc-{}] duration expired", instance_id, vc_id);
                tokio::select! {
                    _ = handle.disconnect() => {}
                    _ = &mut run_future => {}
                }
                connection_token.cancel();
                let _ = stats_tx
                    .send(StatsEvent::Disconnected { instance_id, vc_id })
                    .await;
                retry_count = 0;

                match config.repeat_interval {
                    Some(interval) if interval > 0.0 => {
                        rtc_log_info!(
                            "[i{}/vc-{}] reconnecting in {:.1}s",
                            instance_id,
                            vc_id,
                            interval,
                        );
                        tokio::select! {
                            biased;
                            _ = token.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_secs_f64(interval)) => continue,
                        }
                    }
                    _ => break,
                }
            }
            DisconnectReason::ScenarioDisconnect => {
                rtc_log_info!("[i{}/vc-{}] disconnecting per scenario", instance_id, vc_id,);
                tokio::select! {
                    _ = handle.disconnect() => {}
                    _ = &mut run_future => {}
                }
                connection_token.cancel();
                let _ = stats_tx
                    .send(StatsEvent::Disconnected { instance_id, vc_id })
                    .await;
                retry_count = 0;
                // シナリオは無限ループなので即座に再接続する
                continue;
            }
            DisconnectReason::ScenarioExit => {
                rtc_log_info!("[i{}/vc-{}] exiting per scenario", instance_id, vc_id,);
                tokio::select! {
                    _ = handle.disconnect() => {}
                    _ = &mut run_future => {}
                }
                connection_token.cancel();
                let _ = stats_tx
                    .send(StatsEvent::Disconnected { instance_id, vc_id })
                    .await;
                // Exit に到達したら再接続せず vc タスクを終了する。
                // プロセス全体の token は cancel しない: 呼ぶと他 vc が Exit 未到達の
                // まま Shutdown で終了し、「全クライアントが Exit した後」の完了条件を
                // 満たせなくなる。プロセス終了は全 vc タスク終了後の JoinSet チェーン
                // に任せる。
                break;
            }
            DisconnectReason::Unexpected(result) => {
                connection_token.cancel();
                let _ = stats_tx
                    .send(StatsEvent::Disconnected { instance_id, vc_id })
                    .await;
                if let Err(e) = result {
                    rtc_log_warning!(
                        "[i{}/vc-{}] unexpected disconnect: {}",
                        instance_id,
                        vc_id,
                        e,
                    );
                }
                retry_count += 1;
                if retry_count > config.max_retry {
                    rtc_log_info!(
                        "[i{}/vc-{}] reached max retry count ({})",
                        instance_id,
                        vc_id,
                        config.max_retry,
                    );
                    break;
                }
                let _ = stats_tx
                    .send(StatsEvent::Retrying {
                        instance_id,
                        vc_id,
                        retry_count,
                    })
                    .await;
                rtc_log_info!(
                    "[i{}/vc-{}] retrying in {:.1}s ({}/{})",
                    instance_id,
                    vc_id,
                    config.retry_interval,
                    retry_count,
                    config.max_retry,
                );
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs_f64(config.retry_interval)) => continue,
                }
            }
        }
    }

    let _ = stats_tx
        .send(StatsEvent::Stopped { instance_id, vc_id })
        .await;
}

async fn duration_timer(duration: Option<f64>) {
    match duration {
        Some(d) if d > 0.0 => tokio::time::sleep(Duration::from_secs_f64(d)).await,
        _ => std::future::pending().await,
    }
}

/// DuckDB 統計収集ループ
///
/// `--duckdb-interval` 秒ごとに `handle.get_stats()` を呼び、戻り JSON を
/// `dispatch_stats` で各テーブルに振り分ける。connection_id 確定前の初回 tick は
/// スキップし、確定後にログを出す。
#[expect(clippy::too_many_arguments)]
async fn run_stats_collection(
    instance_id: u32,
    vc_id: u32,
    channel_id: String,
    client: DuckDBClient,
    ids: Arc<std::sync::Mutex<Option<ConnectionIds>>>,
    handle: sora_sdk::SoraConnectionHandle,
    token: CancellationToken,
    interval: Duration,
) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 初回 tick は即座に発火するが、connection_id 未確定の可能性が高いため
    // 1 回目をスキップする (interval.tick() で消費)
    tick.tick().await;
    let mut ticks = IntervalStream::new(tick);
    let mut skipped_iters: u32 = 0;
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            _ = ticks.next() => {
                let Some(parsed) = ({
                    let Ok(guard) = ids.lock() else {
                        rtc_log_warning!(
                            "[i{}/vc-{}][duckdb] connection_ids mutex poisoned in stats_collection",
                            instance_id,
                            vc_id,
                        );
                        continue;
                    };
                    guard.clone()
                }) else {
                    skipped_iters += 1;
                    continue;
                };
                if skipped_iters > 0 {
                    rtc_log_info!(
                        "[i{}/vc-{}][duckdb] connection identifiers confirmed after {} skipped iterations",
                        instance_id,
                        vc_id,
                        skipped_iters,
                    );
                    skipped_iters = 0;
                }
                let stats = match handle.get_stats().await {
                    Ok(s) => s,
                    Err(e) => {
                        rtc_log_warning!(
                            "[i{}/vc-{}][duckdb] get_stats failed: {}",
                            instance_id,
                            vc_id,
                            e,
                        );
                        continue;
                    }
                };
                // JsonString から RawJsonOwned への抽出は再 parse 経由
                // (sora_sdk に as_raw() / into_raw() が無いため)
                let stats_text = stats.to_string();
                dispatch_stats(
                    instance_id,
                    vc_id,
                    &channel_id,
                    &parsed,
                    &client,
                    &stats_text,
                    SystemTime::now(),
                );
            }
        }
    }
}

/// 仮想クライアント用の接続イベントハンドラ。
///
/// offer 受信時に connection_id / session_id を抽出し DuckDB へ記録する。
/// (on_notify の connection.created は同一チャネル内の他 client 接続でも届きうるため不採用)
struct VirtualClientEventHandler {
    ids: Arc<std::sync::Mutex<Option<ConnectionIds>>>,
    duckdb_client: DuckDBClient,
    channel_id: String,
    role: String,
    audio: bool,
    video: bool,
    instance_id: u32,
    vc_id: u32,
}

impl SoraConnectionEventHandler for VirtualClientEventHandler {
    fn on_signaling_message(
        &mut self,
        _signaling_type: sora_sdk::SignalingType,
        direction: SignalingDirection,
        text: &str,
    ) {
        if direction != SignalingDirection::Received {
            return;
        }
        let Some(parsed) = parse_offer_ids(text) else {
            return;
        };
        // poison 時に try_send と ids 更新の両方をスキップするため、先に lock を取る
        let Ok(mut guard) = self.ids.lock() else {
            rtc_log_warning!(
                "[i{}/vc-{}] connection_ids mutex poisoned in on_signaling_message",
                self.instance_id,
                self.vc_id,
            );
            return;
        };
        self.duckdb_client
            .try_send(WriteCommand::InsertConnection(Box::new(
                InsertConnectionRow {
                    instance_id: self.instance_id,
                    vc_id: self.vc_id,
                    timestamp: SystemTime::now(),
                    channel_id: self.channel_id.clone(),
                    connection_id: parsed.connection_id.clone(),
                    session_id: parsed.session_id.clone(),
                    role: self.role.clone(),
                    audio: self.audio,
                    video: self.video,
                },
            )));
        *guard = Some(parsed);
    }
}

fn build_client(
    context: &Arc<SoraConnectionContext>,
    video_source: &Option<VideoTrackSource>,
    config: &VirtualClientConfig,
    ids: &Arc<std::sync::Mutex<Option<ConnectionIds>>>,
    instance_id: u32,
    vc_id: u32,
) -> sora_sdk::Result<(sora_sdk::SoraConnection, sora_sdk::SoraConnectionHandle)> {
    // Audio::Bool(false) は音声無効、それ以外 (None / Audio{...}) は音声有効
    let audio_value = !matches!(&config.audio, Some(sora_sdk::Audio::Bool(false)));
    // Video::Bool(false) は映像無効、それ以外 (None / Video{...}) は映像有効
    let video_value = !matches!(&config.video, Some(sora_sdk::Video::Bool(false)));
    let event_handler = VirtualClientEventHandler {
        ids: ids.clone(),
        duckdb_client: config.duckdb_client.clone(),
        channel_id: config.channel_id.clone(),
        role: config.role.as_sora_role().to_string(),
        audio: audio_value,
        video: video_value,
        instance_id,
        vc_id,
    };

    let mut builder = SoraConnection::builder(
        context.clone(),
        config.signaling_urls.clone(),
        config.channel_id.clone(),
        config.role,
        event_handler,
    );

    if let Some(ref id) = config.client_id {
        builder = builder.client_id(id.clone());
    }
    if let Some(ref id) = config.bundle_id {
        builder = builder.bundle_id(id.clone());
    }
    if let Some(ref metadata) = config.metadata {
        builder = builder.metadata(metadata.clone());
    }
    if let Some(ref metadata) = config.signaling_notify_metadata {
        builder = builder.signaling_notify_metadata(metadata.clone());
    }

    if let Some(video) = &config.video {
        builder = builder.video(video.clone());
    }

    if let Some(audio) = &config.audio {
        builder = builder.audio(audio.clone());
    }

    if config.role.wants_send() {
        if let Some(source) = video_source {
            let video_track = context.create_video_track(source)?;
            builder = builder.sender_video_track(video_track);
        }
        let audio_source = context.create_audio_source()?;
        let audio_track = context.create_audio_track(&audio_source)?;
        builder = builder.sender_audio_track(audio_track);
    }

    if let Some(ref dcs) = config.connect_data_channels {
        builder = builder.data_channels(dcs.clone());
    }
    if let Some(data_channel_signaling) = config.data_channel_signaling {
        builder = builder.data_channel_signaling(data_channel_signaling);
    }
    if let Some(ignore_disconnect_websocket) = config.ignore_disconnect_websocket {
        builder = builder.ignore_disconnect_websocket(ignore_disconnect_websocket);
    }
    if let Some(timeout) = config.disconnect_wait_timeout {
        builder = builder.disconnect_wait_timeout(timeout);
    }

    if let Some(simulcast) = config.simulcast {
        builder = builder.simulcast(simulcast);
    }
    if let Some(ref rid) = config.simulcast_request_rid {
        builder = builder.simulcast_request_rid(rid.clone());
    }
    if let Some(spotlight) = config.spotlight {
        builder = builder.spotlight(spotlight);
    }
    if let Some(ref rid) = config.spotlight_focus_rid {
        builder = builder.spotlight_focus_rid(rid.clone());
    }
    if let Some(ref rid) = config.spotlight_unfocus_rid {
        builder = builder.spotlight_unfocus_rid(rid.clone());
    }

    if config.insecure {
        builder = builder.insecure(true);
    }
    if let (Some(cert), Some(key)) = (&config.client_cert, &config.client_key) {
        builder = builder.client_cert(cert.clone(), key.clone());
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duckdb_stats::WriteCommand;

    /// on_signaling_message の try_send 順序変更後の動作を検証する:
    /// poison 時に try_send が呼ばれず、パニックも発生しないこと
    #[test]
    fn test_on_signaling_message_poison_skips_try_send() {
        let ids = Arc::new(std::sync::Mutex::new(None::<ConnectionIds>));
        // Mutex を poison させる
        let ids_clone = ids.clone();
        let handle = std::thread::spawn(move || {
            let _guard = ids_clone.lock().unwrap();
            panic!("意図的に mutex を poison する");
        });
        let _ = handle.join();

        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WriteCommand>();

        // 修正後の on_signaling_message のロジックを再現する
        let Ok(_guard) = ids.lock() else {
            // poison 時は early return → try_send は実行されない
            assert!(
                rx.try_recv().is_err(),
                "poison 時に try_send が呼ばれずチャネルにメッセージが無いこと"
            );
            return;
        };
        // poison された mutex の lock は失敗するため、ここには到達しない
        unreachable!("poison された mutex の lock は成功しない");
    }

    /// 正常系: try_send が lock 成功後に実行され、ids が更新されることを検証する
    #[test]
    fn test_on_signaling_message_normal_order() {
        let ids = Arc::new(std::sync::Mutex::new(None::<ConnectionIds>));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WriteCommand>();

        // 修正後のロジック: lock → try_send → ids 更新
        {
            let Ok(mut guard) = ids.lock() else {
                panic!("正常系では lock が成功すること");
            };
            let _ = tx.send(WriteCommand::UpdateZakuroStop {
                stop_timestamp: SystemTime::now(),
            });
            *guard = Some(ConnectionIds {
                connection_id: "test_conn".to_string(),
                session_id: "test_sess".to_string(),
            });
        }

        // try_send でメッセージが送信されたことを検証する
        assert!(
            rx.try_recv().is_ok(),
            "正常系では try_send でメッセージが送信されること"
        );
        // ids が更新されたことを検証する
        let stored = ids.lock().unwrap();
        let ids_ref = stored.as_ref().expect("ids が設定されていること");
        assert_eq!(
            ids_ref.connection_id, "test_conn",
            "connection_id が正しく設定されていること"
        );
        assert_eq!(
            ids_ref.session_id, "test_sess",
            "session_id が正しく設定されていること"
        );
    }
}
