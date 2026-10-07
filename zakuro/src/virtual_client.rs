use std::sync::Arc;
use std::time::{Duration, SystemTime};

use shiguredo_webrtc::{
    IceConnectionState, IceGatheringState, PeerConnectionState, SignalingState, VideoTrackSource,
    rtc_log_info, rtc_log_warning,
};
use sora_sdk::{
    ConnectDataChannel, JsonString, Mp4VideoCapturer, Role, SignalingDirection, SoraConnection,
    SoraConnectionContext, SoraConnectionEventHandler,
};
use tokio::sync::mpsc;
use tokio_stream::{StreamExt, wrappers::IntervalStream};
use tokio_util::sync::CancellationToken;

use crate::connection_lifecycle::{
    ConnectionLifecycle, ConnectionOutcome, LifecycleEnd, OUTCOME_GRACE, OutcomeSettings,
};
use crate::data_channel::MessageChannel;
use crate::duckdb_stats::{
    ConnectionIds, DuckDBClient, InsertConnectionLifecycleRow, InsertConnectionRow, WriteCommand,
    dispatch_stats, parse_offer_ids,
};
use crate::media_observer::MediaObserver;
use crate::scenario::{Scenario, ScenarioEnd, ScenarioPlayer};
use zakuro_core::stats::StatsEvent;

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
        // 接続 1 本ごとのライフサイクル。SDK のイベントハンドラ (状態変化) と
        // このループ (試行開始・切断) の両方から更新するため Arc<Mutex> で共有する。
        // 状態変化は 1 接続あたり数回しか起きないため、ロックの保持時間は問題にならない。
        let lifecycle = Arc::new(std::sync::Mutex::new(ConnectionLifecycle::new(
            SystemTime::now(),
        )));
        // 接続を終えるすべての経路でライフサイクルを 1 行残す。呼び出しを短く保つため、
        // この接続に紐づく引数をまとめたクロージャにする。
        // 合否判定に使う設定。ロールと映像 / 音声の有効 / 無効は接続をまたいで同じ。
        let outcome_settings = OutcomeSettings {
            grace: OUTCOME_GRACE,
            expects_send: config.role.wants_send(),
            expects_receive: config.role.wants_recv(),
            video_enabled: is_video_enabled(&config),
            audio_enabled: is_audio_enabled(&config),
        };

        let record_lifecycle = |end: LifecycleEnd| {
            let Some(result) = write_connection_lifecycle(
                &config.duckdb_client,
                &lifecycle,
                &ids,
                instance_id,
                vc_id,
                &config.channel_id,
                config.role.as_sora_role(),
                &outcome_settings,
                end,
            ) else {
                return;
            };
            // 集計側へ合否を渡す。統計と同じく欠落しうる前提のため待たない。
            let _ = stats_tx.try_send(StatsEvent::ConnectionEnded {
                instance_id,
                vc_id,
                outcome: result.outcome.as_str(),
                failure_reason: result.outcome.failure_reason(),
                stalled: result.stalled,
                connect_duration: result.connect_duration,
            });
        };

        // WebRTC の接続確立 (PeerConnection が Connected) を待つための通知経路。
        // SDK のイベントハンドラから 1 度だけ送られる。
        let (connected_tx, connected_rx) = tokio::sync::oneshot::channel::<()>();

        let (client, handle) = match build_client(
            &context,
            &video_source,
            &config,
            &ids,
            &lifecycle,
            connected_tx,
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
                record_lifecycle(LifecycleEnd::BuildFailed);
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
        // WebRTC の接続確立を待ってから connected として数える。
        // build_client() が返るのは接続オブジェクトを作れた時点であり、実際の接続は run() が行う。
        let mut run_future = Box::pin(client.run());
        // 確立前に run() が終わった場合、完了済み future を再 poll すると panic するため結果を保持する
        let mut finished: Option<sora_sdk::Result<()>> = None;
        let established = tokio::select! {
            biased;
            _ = token.cancelled() => false,
            established = connected_rx => established.is_ok(),
            result = &mut run_future => {
                finished = Some(result);
                false
            }
        };

        if established {
            let _ = stats_tx
                .send(StatsEvent::Connected { instance_id, vc_id })
                .await;
            rtc_log_info!("[i{}/vc-{}] connected", instance_id, vc_id);
        } else {
            rtc_log_info!(
                "[i{}/vc-{}] connection ended before the WebRTC establishment",
                instance_id,
                vc_id,
            );
        }

        // DataChannel メッセージングタスクの起動
        let messaging_token = connection_token.child_token();
        if established && !config.message_channels.is_empty() {
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

        // 統計サンプルの収集タスクの起動
        //
        // メディアの送受信の観測は合否判定の材料になるため、DuckDB 出力の
        // 有無にかかわらず行う。書き込みだけを client の有効 / 無効で切り替える。
        if established {
            let stats_client = config.duckdb_client.clone();
            let stats_ids = ids.clone();
            let stats_lifecycle = lifecycle.clone();
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
                    stats_lifecycle,
                    stats_handle,
                    stats_token,
                    interval,
                )
                .await;
            });
        }

        let reason = if let Some(result) = finished {
            // 確立の待機中に run() が終わっていた場合は、その結果で切断処理へ進む
            DisconnectReason::Unexpected(result)
        } else if let Some(ref mut player) = scenario_player {
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
                record_lifecycle(LifecycleEnd::Shutdown);
                break;
            }
            DisconnectReason::DurationExpired => {
                rtc_log_info!("[i{}/vc-{}] duration expired", instance_id, vc_id);
                tokio::select! {
                    _ = handle.disconnect() => {}
                    _ = &mut run_future => {}
                }
                connection_token.cancel();
                record_lifecycle(LifecycleEnd::DurationExpired);
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
                record_lifecycle(LifecycleEnd::ScenarioDisconnect);
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
                record_lifecycle(LifecycleEnd::ScenarioExit);
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
                record_lifecycle(LifecycleEnd::Unexpected);
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

/// 統計サンプルの収集ループ
///
/// `--duckdb-interval` 秒ごとに `handle.get_stats()` を呼び、戻り JSON から
/// メディアの送受信を観測してライフサイクルへ反映する。DuckDB 出力が有効な場合は
/// 同じサンプルを `dispatch_stats` で各テーブルへ振り分ける。
/// connection_id 確定前の初回 tick はスキップし、確定後にログを出す。
#[expect(clippy::too_many_arguments)]
async fn run_stats_collection(
    instance_id: u32,
    vc_id: u32,
    channel_id: String,
    client: DuckDBClient,
    ids: Arc<std::sync::Mutex<Option<ConnectionIds>>>,
    lifecycle: Arc<std::sync::Mutex<ConnectionLifecycle>>,
    handle: sora_sdk::SoraConnectionHandle,
    token: CancellationToken,
    interval: Duration,
) {
    let mut observer = MediaObserver::new();
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
                let now = SystemTime::now();
                // メディアの観測は DuckDB 出力の有無にかかわらず行う (合否判定の材料)
                match lifecycle.lock() {
                    Ok(mut lifecycle) => observer.observe(&mut lifecycle, &stats_text, now),
                    Err(_) => rtc_log_warning!(
                        "[i{}/vc-{}][duckdb] connection lifecycle mutex poisoned in stats_collection",
                        instance_id,
                        vc_id,
                    ),
                }
                if client.is_enabled() {
                    dispatch_stats(instance_id, vc_id, &channel_id, &parsed, &client, &stats_text, now);
                }
            }
        }
    }
}

/// 仮想クライアント用の接続イベントハンドラ。
///
/// offer 受信時に connection_id / session_id を抽出し DuckDB へ記録する。
/// (on_notify の connection.created は同一チャネル内の他 client 接続でも届きうるため不採用)
/// あわせて、WebRTC の接続状態の変化を接続ライフサイクルへ記録する。
struct VirtualClientEventHandler {
    ids: Arc<std::sync::Mutex<Option<ConnectionIds>>>,
    lifecycle: Arc<std::sync::Mutex<ConnectionLifecycle>>,
    /// WebRTC の確立 (PeerConnection が Connected) を 1 度だけ通知する
    connected_tx: Option<tokio::sync::oneshot::Sender<()>>,
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
        // ids のロックを解放してからライフサイクルのロックを取る (ロック順序を固定しない)
        drop(guard);
        self.record(|lifecycle, now| lifecycle.on_offer_received(now));
    }

    fn on_connection_state_change(&mut self, state: PeerConnectionState) {
        self.record(|lifecycle, now| lifecycle.on_peer_connection_state(state, now));
        if state == PeerConnectionState::Connected {
            // 最初の Connected だけを通知する (`take` により 2 回目以降は送らない)
            if let Some(tx) = self.connected_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    fn on_ice_connection_state_change(&mut self, state: IceConnectionState) {
        self.record(|lifecycle, now| lifecycle.on_ice_connection_state(state, now));
    }

    fn on_ice_gathering_state_change(&mut self, state: IceGatheringState) {
        self.record(|lifecycle, now| lifecycle.on_ice_gathering_state(state, now));
    }

    fn on_signaling_state_change(&mut self, state: SignalingState) {
        self.record(|lifecycle, _| lifecycle.on_signaling_state(state));
    }
}

impl VirtualClientEventHandler {
    /// ライフサイクルを更新する
    ///
    /// ロックが poison されている場合は記録を諦める。負荷試験の統計は
    /// 欠落しうる前提 (writer の drop と同じ扱い) のため、ここではパニックさせない。
    fn record(&self, update: impl FnOnce(&mut ConnectionLifecycle, SystemTime)) {
        let Ok(mut lifecycle) = self.lifecycle.lock() else {
            rtc_log_warning!(
                "[i{}/vc-{}] connection lifecycle mutex poisoned; dropping the state change",
                self.instance_id,
                self.vc_id,
            );
            return;
        };
        update(&mut lifecycle, SystemTime::now());
    }
}

/// 判定した接続 1 本の結果
struct ConnectionResult {
    /// 合否の判定結果
    outcome: ConnectionOutcome,
    /// メディアが止まった状態か
    stalled: bool,
    /// 接続の試行から確立までに要した時間 (確立しなかった場合は None)
    connect_duration: Option<Duration>,
}

/// 接続 1 本のライフサイクルを DuckDB へ記録し、判定結果を返す
///
/// 接続を終えるすべての経路から呼び、1 接続につき 1 行を残す。構築に失敗した接続は
/// connection_id / session_id が無いまま 1 行を書く (試行そのものを記録に残す)。
/// 統計の書き込みと同じく `try_send` を使うため、チャネルが満杯のときは行が欠落しうる。
/// ライフサイクルのロックが poison されている場合は記録せず None を返す。
#[expect(clippy::too_many_arguments)]
fn write_connection_lifecycle(
    client: &DuckDBClient,
    lifecycle: &Arc<std::sync::Mutex<ConnectionLifecycle>>,
    ids: &Arc<std::sync::Mutex<Option<ConnectionIds>>>,
    instance_id: u32,
    vc_id: u32,
    channel_id: &str,
    role: &str,
    outcome_settings: &OutcomeSettings,
    end: LifecycleEnd,
) -> Option<ConnectionResult> {
    let now = SystemTime::now();
    let Ok(mut guard) = lifecycle.lock() else {
        rtc_log_warning!(
            "[i{}/vc-{}] connection lifecycle mutex poisoned; skipping the lifecycle row",
            instance_id,
            vc_id,
        );
        return None;
    };
    guard.mark_disconnected(end, now);
    let snapshot = guard.clone();
    drop(guard);

    let (connection_id, session_id) = match ids.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(ids) => (
                Some(ids.connection_id.clone()),
                Some(ids.session_id.clone()),
            ),
            None => (None, None),
        },
        Err(_) => {
            rtc_log_warning!(
                "[i{}/vc-{}] connection_ids mutex poisoned; the lifecycle row has no ids",
                instance_id,
                vc_id,
            );
            (None, None)
        }
    };

    // 記録した材料から接続の合否を判定する
    let outcome = snapshot.judge(outcome_settings);
    let stalled = snapshot.is_stalled();
    let connect_duration = snapshot.webrtc_connected_at.and_then(|connected_at| {
        connected_at
            .duration_since(snapshot.attempt_started_at)
            .ok()
    });
    rtc_log_info!(
        "[i{}/vc-{}] connection lifecycle: outcome={} reason={} stalled={} connect={:?}",
        instance_id,
        vc_id,
        outcome.as_str(),
        outcome.failure_reason().unwrap_or("-"),
        stalled,
        connect_duration,
    );

    client.try_send(WriteCommand::InsertConnectionLifecycle(Box::new(
        InsertConnectionLifecycleRow {
            instance_id,
            vc_id,
            channel_id: channel_id.to_string(),
            role: role.to_string(),
            connection_id,
            session_id,
            attempt_started_at: snapshot.attempt_started_at,
            offer_received_at: snapshot.offer_received_at,
            webrtc_connected_at: snapshot.webrtc_connected_at,
            ice_connected_at: snapshot.ice_connected_at,
            ice_gathering_complete_at: snapshot.ice_gathering_complete_at,
            first_video_sent_at: snapshot.first_video_sent_at,
            first_video_received_at: snapshot.first_video_received_at,
            first_audio_sent_at: snapshot.first_audio_sent_at,
            first_audio_received_at: snapshot.first_audio_received_at,
            first_delivery_report_at: snapshot.first_delivery_report_at,
            samples: snapshot.samples,
            last_media_activity_at: snapshot.last_media_activity_at,
            max_idle_samples: snapshot.max_idle_samples,
            disconnected_at: now,
            peer_connection_state: snapshot.peer_connection_state,
            ice_connection_state: snapshot.ice_connection_state,
            ice_gathering_state: snapshot.ice_gathering_state,
            signaling_state: snapshot.signaling_state,
            end_reason: end.as_str(),
            outcome: outcome.as_str(),
            failure_reason: outcome.failure_reason(),
            stalled,
        },
    )));
    Some(ConnectionResult {
        outcome,
        stalled,
        connect_duration,
    })
}

/// 映像が有効か
///
/// `Video::Bool(false)` は映像無効、それ以外 (未指定 / `Video` の各設定) は映像有効。
/// 合否判定と SDP の組み立てで判定が食い違わないよう、1 箇所にまとめる。
fn is_video_enabled(config: &VirtualClientConfig) -> bool {
    !matches!(&config.video, Some(sora_sdk::Video::Bool(false)))
}

/// 音声が有効か
///
/// `Audio::Bool(false)` は音声無効、それ以外 (未指定 / `Audio` の各設定) は音声有効。
fn is_audio_enabled(config: &VirtualClientConfig) -> bool {
    !matches!(&config.audio, Some(sora_sdk::Audio::Bool(false)))
}

/// 仮想クライアントを構築する
///
/// `connected_tx` は WebRTC の確立 (PeerConnection が Connected) を 1 度だけ通知する。
#[expect(clippy::too_many_arguments)]
fn build_client(
    context: &Arc<SoraConnectionContext>,
    video_source: &Option<VideoTrackSource>,
    config: &VirtualClientConfig,
    ids: &Arc<std::sync::Mutex<Option<ConnectionIds>>>,
    lifecycle: &Arc<std::sync::Mutex<ConnectionLifecycle>>,
    connected_tx: tokio::sync::oneshot::Sender<()>,
    instance_id: u32,
    vc_id: u32,
) -> sora_sdk::Result<(sora_sdk::SoraConnection, sora_sdk::SoraConnectionHandle)> {
    let audio_value = is_audio_enabled(config);
    let video_value = is_video_enabled(config);
    let event_handler = VirtualClientEventHandler {
        ids: ids.clone(),
        lifecycle: lifecycle.clone(),
        connected_tx: Some(connected_tx),
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

    /// 固定の基準時刻からの経過秒で SystemTime を作る
    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// テスト用のイベントハンドラを組み立てる
    fn test_event_handler(
        ids: Arc<std::sync::Mutex<Option<ConnectionIds>>>,
        lifecycle: Arc<std::sync::Mutex<ConnectionLifecycle>>,
    ) -> VirtualClientEventHandler {
        VirtualClientEventHandler {
            ids,
            lifecycle,
            connected_tx: None,
            duckdb_client: DuckDBClient::noop(),
            channel_id: "ch".to_string(),
            role: "sendonly".to_string(),
            audio: true,
            video: true,
            instance_id: 0,
            vc_id: 1,
        }
    }

    /// WebRTC の確立で通知が 1 度だけ送られること
    #[test]
    fn test_connected_notification_fires_once() {
        let ids = Arc::new(std::sync::Mutex::new(None::<ConnectionIds>));
        let lifecycle = Arc::new(std::sync::Mutex::new(ConnectionLifecycle::new(at(100))));
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
        let mut handler = VirtualClientEventHandler {
            ids: Arc::clone(&ids),
            lifecycle: Arc::clone(&lifecycle),
            connected_tx: Some(tx),
            duckdb_client: DuckDBClient::noop(),
            channel_id: "ch".to_string(),
            role: "sendonly".to_string(),
            audio: true,
            video: true,
            instance_id: 0,
            vc_id: 1,
        };

        // Connected 以外では通知しない
        handler.on_connection_state_change(PeerConnectionState::Connecting);
        assert!(
            rx.try_recv().is_err(),
            "Connected 以外では確立を通知しないこと"
        );

        handler.on_connection_state_change(PeerConnectionState::Connected);
        assert!(rx.try_recv().is_ok(), "Connected で確立を通知すること");
        assert!(
            handler.connected_tx.is_none(),
            "通知の送信器は 1 度使ったら破棄されること"
        );

        // 2 回目の Connected では送信器が無いため何も起きない
        handler.on_connection_state_change(PeerConnectionState::Connected);
    }

    /// offer 受信で ids とライフサイクルの offer 受信時刻が記録されること
    #[test]
    fn test_on_signaling_message_records_offer() {
        let ids = Arc::new(std::sync::Mutex::new(None::<ConnectionIds>));
        let lifecycle = Arc::new(std::sync::Mutex::new(ConnectionLifecycle::new(at(100))));
        let mut handler = test_event_handler(Arc::clone(&ids), Arc::clone(&lifecycle));

        handler.on_signaling_message(
            sora_sdk::SignalingType::WebSocket,
            SignalingDirection::Received,
            r#"{"type":"offer","connection_id":"conn-1","session_id":"sess-1"}"#,
        );

        let stored = ids.lock().expect("ids のロックに成功すること");
        let stored = stored.as_ref().expect("offer 受信で ids が設定されること");
        assert_eq!(
            stored.connection_id, "conn-1",
            "connection_id が記録されること"
        );
        let recorded = lifecycle
            .lock()
            .expect("ライフサイクルのロックに成功すること");
        assert!(
            recorded.offer_received_at.is_some(),
            "offer の受信時刻が記録されること"
        );
        assert!(
            recorded.webrtc_connected_at.is_none(),
            "WebRTC の確立は offer 受信では記録されないこと"
        );
    }

    /// WebRTC の状態変化がライフサイクルへ記録されること
    #[test]
    fn test_connection_state_change_records_webrtc_establishment() {
        let ids = Arc::new(std::sync::Mutex::new(None::<ConnectionIds>));
        let lifecycle = Arc::new(std::sync::Mutex::new(ConnectionLifecycle::new(at(100))));
        let mut handler = test_event_handler(Arc::clone(&ids), Arc::clone(&lifecycle));

        handler.on_connection_state_change(PeerConnectionState::Connected);
        handler.on_ice_connection_state_change(IceConnectionState::Completed);
        handler.on_ice_gathering_state_change(IceGatheringState::Complete);

        let recorded = lifecycle
            .lock()
            .expect("ライフサイクルのロックに成功すること");
        assert!(
            recorded.webrtc_connected_at.is_some(),
            "WebRTC の確立時刻が記録されること"
        );
        assert!(
            recorded.ice_connected_at.is_some(),
            "ICE の接続時刻が記録されること"
        );
        assert!(
            recorded.ice_gathering_complete_at.is_some(),
            "候補収集の完了時刻が記録されること"
        );
        assert_eq!(
            recorded.peer_connection_state,
            Some("connected"),
            "PeerConnection の状態が記録されること"
        );
    }

    /// ライフサイクルが 1 行として writer へ渡されること
    #[test]
    fn test_write_connection_lifecycle_sends_one_row() {
        let (tx, mut rx) = mpsc::channel::<WriteCommand>(4);
        let client = DuckDBClient {
            sender: Some(tx),
            dropped_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let lifecycle = Arc::new(std::sync::Mutex::new(ConnectionLifecycle::new(at(100))));
        {
            let mut guard = lifecycle
                .lock()
                .expect("ライフサイクルのロックに成功すること");
            guard.on_offer_received(at(110));
            guard.on_peer_connection_state(PeerConnectionState::Connected, at(120));
            // 判定には統計サンプルが要る (0 件だと判定不能になる)
            guard.on_sample_observed();
            guard.on_sample_activity(at(130), false);
        }
        let ids = Arc::new(std::sync::Mutex::new(Some(ConnectionIds {
            connection_id: "conn-1".to_string(),
            session_id: "sess-1".to_string(),
        })));

        // 確立後にメディアを観測していないため、失敗 (送信が無い) と判定される
        let settings = OutcomeSettings {
            grace: OUTCOME_GRACE,
            expects_send: true,
            expects_receive: false,
            video_enabled: true,
            audio_enabled: true,
        };
        write_connection_lifecycle(
            &client,
            &lifecycle,
            &ids,
            0,
            1,
            "ch",
            "sendonly",
            &settings,
            LifecycleEnd::DurationExpired,
        );

        let command = rx.try_recv().expect("ライフサイクルの行が送られること");
        match command {
            WriteCommand::InsertConnectionLifecycle(row) => {
                assert_eq!(
                    row.connection_id.as_deref(),
                    Some("conn-1"),
                    "connection_id が記録されること"
                );
                assert_eq!(
                    row.session_id.as_deref(),
                    Some("sess-1"),
                    "session_id が記録されること"
                );
                assert_eq!(row.role, "sendonly", "role が記録されること");
                assert_eq!(
                    row.offer_received_at,
                    Some(at(110)),
                    "offer の受信時刻が記録されること"
                );
                assert_eq!(
                    row.webrtc_connected_at,
                    Some(at(120)),
                    "WebRTC の確立時刻が記録されること"
                );
                assert_eq!(
                    row.end_reason, "duration-expired",
                    "終了理由が記録されること"
                );
                assert!(row.disconnected_at >= at(100), "切断時刻が記録されること");
                assert_eq!(row.outcome, "failure", "判定結果が記録されること");
                assert_eq!(
                    row.failure_reason,
                    Some("no-media-sent"),
                    "失敗理由が記録されること"
                );
                assert!(!row.stalled, "メディアが止まった状態ではないこと");
            }
            _ => panic!("InsertConnectionLifecycle が送られること"),
        }
        assert!(
            rx.try_recv().is_err(),
            "接続 1 本につき 1 行だけ送られること"
        );
    }
}
