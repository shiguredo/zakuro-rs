mod args;
mod data_channel;
mod error;
mod fake_video_capturer;
mod http_server;
mod json_rpc;
mod mp4_video_capturer;
mod openh264_video_codec;
mod stats;
mod video_device_capturer;
mod virtual_client;
mod y4m_reader;

use std::time::Duration;

use shiguredo_webrtc::{log, rtc_log_info, rtc_log_warning};
use sora_sdk::{AdmConfig, SoraClientContext, SoraClientContextConfig, VideoCodecPreference};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::{ErrorMessage, Result};
use crate::fake_video_capturer::{FakeVideoCapturer, FakeVideoCapturerConfig};
use crate::mp4_video_capturer::{
    Mp4PassthroughVideoCodecCapability, Mp4SampleReader, Mp4VideoCapturer,
};
use crate::stats::StatsCollector;
use crate::video_device_capturer::{VideoDeviceCapturer, VideoDeviceCapturerConfig};
use crate::virtual_client::VirtualClientConfig;

/// デバイス名または ID からデバイス ID を解決する
///
/// 名前の前方一致で検索し、見つからなければ ID として扱う。
fn resolve_device_id(name_or_id: &str) -> Result<String> {
    let device_list = shiguredo_video_device::VideoDeviceList::enumerate()?;
    // 名前で検索する
    for device in device_list.devices() {
        if let Ok(name) = device.name()
            && name == name_or_id
        {
            return Ok(device
                .unique_id()
                .unwrap_or_else(|_| name_or_id.to_string()));
        }
    }
    // ID として扱う
    for device in device_list.devices() {
        if let Ok(uid) = device.unique_id()
            && uid == name_or_id
        {
            return Ok(uid);
        }
    }
    // 見つからなかった場合はそのまま渡す（デバイス側でエラーになる）
    rtc_log_warning!(
        "Video device '{}' not found in enumeration, passing as-is",
        name_or_id
    );
    Ok(name_or_id.to_string())
}

fn build_video(args: &args::Args) -> Option<sora_sdk::Video> {
    if args.no_video_device {
        return Some(sora_sdk::Video::new_bool(false));
    }
    match args.video_codec_type.as_deref() {
        Some("vp8") => Some(sora_sdk::Video::new_vp8(args.video_bit_rate)),
        Some("vp9") => Some(sora_sdk::Video::new_vp9(args.video_bit_rate, None)),
        Some("av1") => Some(sora_sdk::Video::new_av1(args.video_bit_rate, None)),
        Some("h264") => Some(sora_sdk::Video::new_h264(args.video_bit_rate, None)),
        Some("h265") => Some(sora_sdk::Video::new_h265(args.video_bit_rate, None)),
        _ => {
            if args.video_bit_rate.is_some() {
                Some(sora_sdk::Video::new_vp8(args.video_bit_rate))
            } else {
                None
            }
        }
    }
}

fn build_audio(args: &args::Args) -> Option<sora_sdk::Audio> {
    if args.no_audio_device || !args.audio {
        return Some(sora_sdk::Audio::new_bool(false));
    }
    match args.audio_codec_type.as_deref() {
        Some("opus") => Some(sora_sdk::Audio::new_opus(args.audio_bit_rate, None)),
        _ => {
            if args.audio_bit_rate.is_some() {
                Some(sora_sdk::Audio::new_opus(args.audio_bit_rate, None))
            } else {
                None
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    log::log_to_debug(log::Severity::Info);
    log::enable_timestamps();
    log::enable_threads();

    let args = args::parse_args()?;

    rtc_log_info!(
        "zakuro: vcs={} hatch_rate={} duration={:?} repeat_interval={:?}",
        args.vcs,
        args.vcs_hatch_rate,
        args.duration,
        args.repeat_interval,
    );

    // MP4 パススルー時はコーデック能力をカスタマイズする
    let mp4_sample_slot = if let Some(ref mp4_path) = args.input_mp4 {
        let reader = Mp4SampleReader::new(mp4_path)?;
        let expected_codec =
            mp4_video_capturer::parse_video_codec_type(args.video_codec_type.as_deref().unwrap())
                .ok_or_else(|| ErrorMessage::new("--sora-video-codec-type の値が不正です"))?;
        if reader.codec_type() != expected_codec {
            return Err(ErrorMessage::new(format!(
                "MP4 ファイルのコーデック ({:?}) と --sora-video-codec-type ({:?}) が一致しません",
                reader.codec_type(),
                expected_codec,
            ))
            .into());
        }
        let slot = mp4_video_capturer::new_sample_slot();
        Some((reader, slot))
    } else {
        None
    };

    // OpenH264 ライブラリのロード
    let openh264_lib = if let Some(ref path) = args.openh264 {
        Some(openh264_video_codec::load_openh264_library(path)?)
    } else {
        None
    };

    let context_config = {
        let mut config = SoraClientContextConfig {
            adm_config: AdmConfig::NoAudioDevice,
            ..Default::default()
        };

        // MP4 パススルーコーデック能力の登録
        if let Some((_, ref slot)) = mp4_sample_slot {
            let codec_type = mp4_video_capturer::parse_video_codec_type(
                args.video_codec_type.as_deref().unwrap(),
            )
            .unwrap();
            let mp4_capability: Box<dyn sora_sdk::VideoCodecCapability> = Box::new(
                Mp4PassthroughVideoCodecCapability::new(codec_type, slot.clone()),
            );
            let mp4_preference = VideoCodecPreference::new_from_capability(mp4_capability.as_ref());
            config.video_codec_preference.merge(&mp4_preference);
            config.video_codec_capabilities.push(mp4_capability);
        }

        // OpenH264 コーデック能力の登録
        if let Some(lib) = openh264_lib {
            let openh264_capability: Box<dyn sora_sdk::VideoCodecCapability> =
                Box::new(openh264_video_codec::Openh264VideoCodecCapability::new(lib));
            let openh264_preference =
                VideoCodecPreference::new_from_capability(openh264_capability.as_ref());
            config.video_codec_preference.merge(&openh264_preference);
            config.video_codec_capabilities.push(openh264_capability);
        }

        config
    };

    let context = SoraClientContext::new_with_config(context_config)?;

    let token = CancellationToken::new();

    // 映像キャプチャ（映像有効時のみ）
    let mut _fake_capturer = None;
    let mut _device_capturer = None;
    let mut _mp4_capturer = None;
    let video_source = if !args.no_video_device && args.role.wants_send() {
        if let Some((reader, slot)) = mp4_sample_slot {
            // MP4 パススルーキャプチャ
            let mut capturer = Mp4VideoCapturer::new();
            capturer.start(reader, slot)?;
            let source = capturer.video_source();
            _mp4_capturer = Some(capturer);
            Some(source)
        } else if let Some(ref device_name) = args.video_input_device {
            // 実デバイスキャプチャ
            let device_id = resolve_device_id(device_name)?;
            let config = VideoDeviceCapturerConfig {
                device_id: Some(device_id),
                width: args.resolution.0,
                height: args.resolution.1,
                fps: args.framerate as i32,
            };
            let mut capturer = VideoDeviceCapturer::new(config)?;
            capturer.start()?;
            let source = capturer.video_source();
            _device_capturer = Some(capturer);
            Some(source)
        } else {
            // フェイク映像キャプチャ
            let config = FakeVideoCapturerConfig {
                width: args.resolution.0,
                height: args.resolution.1,
                fps: args.framerate as i32,
                sandstorm: args.sandstorm,
                y4m_path: args
                    .fake_video_capture
                    .as_ref()
                    .map(std::path::PathBuf::from),
            };
            let mut capturer = FakeVideoCapturer::new(config)?;
            capturer.start()?;
            let source = capturer.video_source();
            _fake_capturer = Some(capturer);
            Some(source)
        }
    } else {
        None
    };

    let stats = StatsCollector::new(args.vcs, token.clone());
    let stats_tx = stats.event_tx();

    // DataChannel メッセージング設定のパース
    let (connect_data_channels, message_channels) = if let Some(ref dc_json) = args.data_channels {
        let (connect, msg) = data_channel::parse_data_channels(dc_json)?;
        (Some(connect), msg)
    } else {
        (None, Vec::new())
    };

    let vc_config = VirtualClientConfig {
        signaling_urls: args.signaling_urls.clone(),
        channel_id: args.channel_id.clone(),
        role: args.role,
        duration: args.duration,
        repeat_interval: args.repeat_interval,
        max_retry: args.max_retry,
        retry_interval: args.retry_interval,
        video: build_video(&args),
        audio: build_audio(&args),
        connect_data_channels,
        message_channels,
        data_channel_signaling: args.data_channel_signaling,
        ignore_disconnect_websocket: args.ignore_disconnect_websocket,
        simulcast: args.simulcast,
        simulcast_request_rid: args.simulcast_request_rid.clone(),
        spotlight: args.spotlight,
        spotlight_focus_rid: args.spotlight_focus_rid.clone(),
        spotlight_unfocus_rid: args.spotlight_unfocus_rid.clone(),
    };

    let mut clients = JoinSet::new();

    // hatch rate 制御
    let hatch_start = tokio::time::Instant::now();
    let interval_per_client = Duration::from_secs_f64(1.0 / args.vcs_hatch_rate);

    for i in 0..args.vcs {
        if token.is_cancelled() {
            break;
        }

        // hatch タイミングまで待機（初回は即座に起動）
        if i > 0 {
            let target = hatch_start + interval_per_client * i;
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = tokio::time::sleep_until(target) => {}
            }
        }

        rtc_log_info!("仮想クライアント {} を起動します", i);

        let child_token = token.child_token();
        clients.spawn(virtual_client::run(
            i,
            context.clone(),
            video_source.clone(),
            vc_config.clone(),
            child_token,
            stats_tx.clone(),
        ));
    }

    // main 側の stats_tx を drop して、全クライアント終了時に channel が閉じるようにする
    drop(stats_tx);

    // HTTP サーバーの起動
    if let (Some(host), Some(port)) = (&args.http_host, args.http_port) {
        let server = http_server::HttpServer::bind(host, port, token.clone())
            .await
            .map_err(|e| error::ErrorMessage::new(format!("HTTP server bind failed: {e}")))?;
        let handler = http_server::DefaultHandler;
        tokio::spawn(async move {
            server.run(handler).await;
        });
    }

    // Ctrl+C で CancellationToken を発火
    let shutdown_token = token.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        rtc_log_info!("Ctrl+C を受信しました。シャットダウンします...");
        shutdown_token.cancel();
    });

    // 全仮想クライアントの完了を待機
    while let Some(result) = clients.join_next().await {
        if let Err(e) = result {
            rtc_log_warning!("仮想クライアントタスクがパニックしました: {}", e);
        }
    }

    // 統計タスク等を停止
    token.cancel();

    rtc_log_info!("zakuro: 全ての仮想クライアントが終了しました");

    Ok(())
}
