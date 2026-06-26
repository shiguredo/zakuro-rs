use std::sync::Arc;
use std::time::Duration;

use shiguredo_webrtc::{VideoTrackSource, rtc_log_info, rtc_log_warning};
use sora_sdk::{ConnectDataChannel, JsonString, Role, SoraConnection, SoraConnectionContext};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::data_channel::MessageChannel;
use crate::scenario::{Scenario, ScenarioPlayer};
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
}

enum DisconnectReason {
    Shutdown,
    DurationExpired,
    ScenarioDisconnect,
    Unexpected(sora_sdk::Result<()>),
}

pub(crate) async fn run(
    instance_id: u32,
    vc_id: u32,
    context: Arc<SoraConnectionContext>,
    video_source: Option<VideoTrackSource>,
    config: VirtualClientConfig,
    token: CancellationToken,
    stats_tx: mpsc::Sender<StatsEvent>,
) {
    let mut retry_count: u32 = 0;
    let mut scenario_player = config.scenario.clone().map(ScenarioPlayer::new);

    loop {
        let connection_token = token.child_token();

        let (client, handle) = match build_client(&context, &video_source, &config) {
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

        let mut run_future = Box::pin(client.run());

        let reason = if let Some(ref mut player) = scenario_player {
            // シナリオモード: シナリオの Disconnect 操作まで実行する
            tokio::select! {
                biased;
                _ = token.cancelled() => DisconnectReason::Shutdown,
                _ = player.run_until_disconnect(&token) => DisconnectReason::ScenarioDisconnect,
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

fn build_client(
    context: &Arc<SoraConnectionContext>,
    video_source: &Option<VideoTrackSource>,
    config: &VirtualClientConfig,
) -> sora_sdk::Result<(sora_sdk::SoraConnection, sora_sdk::SoraConnectionHandle)> {
    let mut builder = SoraConnection::builder(
        context.clone(),
        config.signaling_urls.clone(),
        config.channel_id.clone(),
        config.role,
    )
    .on_notify(|_text| {})
    .on_push(|_text| {})
    .on_track(|_transceiver| {})
    .on_remove_track(|_receiver| {});

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
