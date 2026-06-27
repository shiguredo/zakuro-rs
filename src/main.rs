mod args;
mod data_channel;
mod duckdb_stats;
mod error;
mod fake_audio_capturer;
mod fake_video_capturer;
mod http_server;
mod json_rpc;
mod nop_video_decoder;
mod openh264_video_codec;
mod scenario;
mod stats;
mod video_device_capturer;
mod virtual_client;
mod wav_reader;
mod y4m_reader;

use std::time::Duration;

use shiguredo_openh264::Openh264Library;
use shiguredo_webrtc::{log, rtc_log_info, rtc_log_warning};
use sora_sdk::{
    AdmConfig, JsonString, Mp4PassthroughVideoCodecCapability, Mp4SampleReader, Mp4VideoCapturer,
    SoraConnectionContext, SoraConnectionContextConfig, VideoCodecPreference,
};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tokio_util::time::DelayQueue;

use crate::args::{CommonArgs, InstanceArgs};
use crate::duckdb_stats::WriteCommand;
use crate::error::{ErrorMessage, Result};
use crate::fake_video_capturer::{FakeVideoCapturer, FakeVideoCapturerConfig};
use crate::stats::{StatsCollector, StatsEvent};
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

fn build_video(args: &InstanceArgs) -> Option<sora_sdk::Video> {
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

fn build_audio(args: &InstanceArgs) -> Option<sora_sdk::Audio> {
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

fn main() -> Result<()> {
    // `FakeAudioCapturer` などの libwebrtc 由来オブジェクトが !Send のため、
    // instance ごとの future は LocalSet 上で `spawn_local` する必要がある
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| ErrorMessage::new(format!("Failed to build tokio runtime: {e}")))?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async_main())
}

async fn async_main() -> Result<()> {
    log::log_to_debug(log::Severity::Info);
    log::enable_timestamps();
    log::enable_threads();

    let (common, instance_args_vec, config_path) = args::parse_args()?;

    let total_vcs: u32 = instance_args_vec.iter().map(|i| i.vcs).sum();
    let instances_count = instance_args_vec.len() as u32;

    rtc_log_info!(
        "zakuro: instances={} instance-hatch-rate={} total-vcs={}",
        instances_count,
        common.instance_hatch_rate,
        total_vcs,
    );

    // OpenH264 ライブラリのロード (プロセス全体で 1 回)
    let openh264_lib: Option<Openh264Library> = if let Some(ref path) = common.openh264 {
        Some(openh264_video_codec::load_openh264_library(path)?)
    } else {
        None
    };
    // OpenH264 ランタイムバージョン (ロード後に取得可能、zakuro テーブル用)
    let openh264_runtime_version: Option<String> =
        openh264_lib.as_ref().map(|lib| lib.runtime_version());

    // mTLS PEM の読み込み (プロセス全体で 1 回)
    let client_cert_pem: Option<String> = if let Some(ref path) = common.client_cert {
        Some(
            std::fs::read_to_string(path)
                .map_err(|e| ErrorMessage::new(format!("Failed to read client cert: {e}")))?,
        )
    } else {
        None
    };
    let client_key_pem: Option<String> = if let Some(ref path) = common.client_key {
        Some(
            std::fs::read_to_string(path)
                .map_err(|e| ErrorMessage::new(format!("Failed to read client key: {e}")))?,
        )
    } else {
        None
    };

    // DuckDB ファイルパスの生成 (UTC タイムスタンプ付き、1 プロセス 1 ファイル)
    // --no-duckdb-output 指定時はファイルを生成せず noop クライアントになる
    let duckdb_enabled = !common.no_duckdb_output;
    let duckdb_db_path = if duckdb_enabled {
        let dir = std::path::Path::new(&common.duckdb_output_dir);
        let filename = duckdb_stats::generate_filename();
        let path = dir.join(&filename);
        // 同名ファイル存在は起動エラー (ミリ秒単位で衝突することは通常無いが念のため)
        if path.exists() {
            return Err(ErrorMessage::new(format!(
                "DuckDB file already exists: {}",
                path.display()
            ))
            .into());
        }
        Some(path)
    } else {
        None
    };

    // DuckDB writer の起動 (init readiness ハンドシェイクでスキーマ投入完了を待つ)
    let duckdb_config = duckdb_stats::DuckDBWriterConfig {
        db_path: duckdb_db_path.unwrap_or_default(),
        interval: Duration::from_secs_f64(common.duckdb_interval),
        enabled: duckdb_enabled,
    };
    let (duckdb_writer, duckdb_version) =
        duckdb_stats::DuckDBStatsWriter::start(duckdb_config).await?;
    let duckdb_client = duckdb_writer.client();

    // zakuro テーブルへの起動情報 INSERT
    if duckdb_client.is_enabled() {
        let config_mode = if config_path.is_some() {
            "JSONC"
        } else {
            "ARGS"
        };
        let config_json = duckdb_stats::build_config_json(&common, &instance_args_vec);
        duckdb_client.try_send(WriteCommand::InsertZakuro(Box::new(
            duckdb_stats::InsertZakuroRow {
                version: env!("CARGO_PKG_VERSION").to_string(),
                sora_sdk_version: None, // sora_sdk に公開 version() 関数が無いため NULL
                webrtc_version: Some(shiguredo_webrtc::version().to_string()),
                openh264_version: openh264_runtime_version,
                duckdb_version: Some(duckdb_version),
                environment: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
                config_mode: config_mode.to_string(),
                config_json,
                start_timestamp: std::time::SystemTime::now(),
            },
        )));
        // 各 InstanceArgs ごとに zakuro_scenario へ 1 行 INSERT
        for (i, inst) in instance_args_vec.iter().enumerate() {
            duckdb_client.try_send(WriteCommand::InsertZakuroScenario(Box::new(
                duckdb_stats::InsertZakuroScenarioRow {
                    instance_id: i as u32,
                    vcs: inst.vcs,
                    duration: inst.duration,
                    repeat_interval: inst.repeat_interval,
                    max_retry: inst.max_retry,
                    retry_interval: inst.retry_interval,
                    sora_signaling_urls: inst.signaling_urls.clone(),
                    sora_channel_id: inst.channel_id.clone(),
                    sora_role: inst.role.as_sora_role().to_string(),
                },
            )));
        }
    }

    let token = CancellationToken::new();

    let stats = StatsCollector::new(total_vcs, instances_count, token.clone());
    let stats_tx = stats.event_tx();

    // Ctrl+C ハンドラを先に起動 (DelayQueue poll 中のキャンセル経路を確保)
    let shutdown_token = token.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        rtc_log_info!("Ctrl+C received, shutting down...");
        shutdown_token.cancel();
    });

    // HTTP サーバーの起動 (両方ある場合のみ、Ctrl+C ハンドラ起動と DelayQueue 構築の間)
    if let (Some(host), Some(port)) = (&common.http_host, common.http_port) {
        let server = http_server::HttpServer::bind(host, port, token.clone())
            .await
            .map_err(|e| ErrorMessage::new(format!("HTTP server bind failed: {e}")))?;
        let handler = http_server::DefaultHandler;
        tokio::spawn(async move {
            server.run(handler).await;
        });
    }

    // hatch rate 制御の DelayQueue を構築
    let hatch_start = tokio::time::Instant::now();
    let interval = Duration::from_secs_f64(1.0 / common.instance_hatch_rate);
    let mut delay: DelayQueue<u32> = DelayQueue::new();
    for i in 0..instances_count {
        delay.insert(i, interval * i);
    }

    // instance 引数は起動時に 1 度だけ消費するため Option<InstanceArgs> でラップして take する
    let mut pending: Vec<Option<InstanceArgs>> = instance_args_vec.into_iter().map(Some).collect();
    // JoinSet の Item を (instance_id, Result<()>) にすることで、正常終了 / Err 経路で
    // instance_id を取り出せるようにする。JoinError 経路 (panic) は instance_id 取得不可。
    // FakeAudioCapturer などの !Send 型を future が保持するため `spawn_local` を使う。
    let mut instances: JoinSet<(u32, Result<()>)> = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            maybe_expired = delay.next() => {
                // DelayQueue が空になれば全 instance 起動完了
                let Some(expired) = maybe_expired else { break };
                let i = expired.into_inner();
                rtc_log_info!(
                    "Starting zakuro instance {} at +{:.2}s",
                    i,
                    hatch_start.elapsed().as_secs_f64(),
                );
                let instance = pending[i as usize]
                    .take()
                    .expect("logical invariant: each instance_id is dispatched once via DelayQueue and taken on first dispatch");
                let task_token = token.child_token();
                let common_cloned = common.clone();
                let openh264_lib_cloned = openh264_lib.clone();
                let client_cert_pem_cloned = client_cert_pem.clone();
                let client_key_pem_cloned = client_key_pem.clone();
                let stats_tx_cloned = stats_tx.clone();
                let duckdb_client_cloned = duckdb_client.clone();
                let duckdb_interval = Duration::from_secs_f64(common.duckdb_interval);
                instances.spawn_local(async move {
                    let result = run_zakuro_instance(
                        i,
                        common_cloned,
                        instance,
                        openh264_lib_cloned,
                        client_cert_pem_cloned,
                        client_key_pem_cloned,
                        task_token,
                        stats_tx_cloned,
                        duckdb_client_cloned,
                        duckdb_interval,
                    ).await;
                    (i, result)
                });
            }
        }
    }

    // loop を抜けた経路は 2 通り:
    //   (1) token.cancelled() (Ctrl+C 等): aggregator / reporter は token.cancelled で先に break する。
    //       後続の drop(stats_tx) と token.cancel() は idempotent。
    //   (2) DelayQueue::next() が None (全 instance 起動完了): 以降は aggregator が channel close を
    //       見て break する必要があるため、main 側の stats_tx を drop する。
    drop(stats_tx);

    while let Some(joined) = instances.join_next().await {
        match joined {
            Ok((id, Ok(()))) => rtc_log_info!("Zakuro instance {} finished", id),
            Ok((id, Err(e))) => {
                rtc_log_warning!("Zakuro instance {} failed: {}", id, e);
            }
            Err(e) => rtc_log_warning!("Zakuro instance task panicked: {}", e),
        }
    }

    // 経路 (2) で reporter (定期統計出力) を停止する。経路 (1) では既に cancel 済みだが
    // token.cancel() は idempotent なため二度呼び出しても問題ない。
    token.cancel();

    // DuckDB writer の shutdown ハンドシェイク
    // 1. stop_timestamp UPDATE を確実に送る (try_send だと満杯時に drop されるため send.await)
    // 2. main 側の client を drop して全 Sender を drop (writer の recv が None を返す)
    // 3. writer task の完了を待つ (stop_timestamp UPDATE 完了を保証)
    if duckdb_client.is_enabled() {
        duckdb_client
            .send(WriteCommand::UpdateZakuroStop {
                stop_timestamp: std::time::SystemTime::now(),
            })
            .await;
    }
    drop(duckdb_client);
    duckdb_writer.join().await?;

    rtc_log_info!("zakuro: all Zakuro instances finished");

    Ok(())
}

/// 1 つの Zakuro インスタンスを実行する
///
/// `SoraConnectionContext` 構築 / 映像キャプチャ初期化 / vcs 個の仮想クライアントの
/// `vcs-hatch-rate` 制御スポーン・完了待機まで担当する。
#[expect(clippy::too_many_arguments)]
async fn run_zakuro_instance(
    instance_id: u32,
    common: CommonArgs,
    instance: InstanceArgs,
    openh264_lib: Option<Openh264Library>,
    client_cert_pem: Option<String>,
    client_key_pem: Option<String>,
    token: CancellationToken,
    stats_tx: mpsc::Sender<StatsEvent>,
    duckdb_client: crate::duckdb_stats::DuckDBClient,
    duckdb_interval: Duration,
) -> Result<()> {
    rtc_log_info!(
        "Zakuro instance {}: vcs={} vcs-hatch-rate={} duration={:?} repeat_interval={:?}",
        instance_id,
        instance.vcs,
        instance.vcs_hatch_rate,
        instance.duration,
        instance.repeat_interval,
    );

    // MP4 パススルー時はコーデック能力をカスタマイズする
    let mp4_reader = if let Some(ref mp4_path) = instance.input_mp4 {
        let reader = Mp4SampleReader::new(mp4_path)
            .map_err(|e| ErrorMessage::new(format!("Failed to read MP4 file: {e}")))?;
        let expected_codec =
            args::parse_video_codec_type(instance.video_codec_type.as_deref().expect(
                "guarded by InstanceArgs validation: input_mp4 requires sora-video-codec-type",
            ))
            .ok_or_else(|| ErrorMessage::new("--sora-video-codec-type の値が不正です"))?;
        if reader.codec_type() != expected_codec {
            return Err(ErrorMessage::new(format!(
                "MP4 ファイルのコーデック ({:?}) と --sora-video-codec-type ({:?}) が一致しません",
                reader.codec_type(),
                expected_codec,
            ))
            .into());
        }
        Some(reader)
    } else {
        None
    };

    // フェイク音声キャプチャの初期化
    // 音声有効かつフェイク映像モード時にビープ音または WAV ファイル再生を行う
    let use_fake_audio = !instance.no_audio_device
        && instance.audio
        && instance.role.wants_send()
        && instance.input_mp4.is_none()
        && instance.video_input_device.is_none();
    // WAV モードでは映像連動ビープが意味を持たないため、ビープトリガーは生成しない
    let use_wav_audio = use_fake_audio && instance.input_wav.is_some();
    let beep_trigger = if use_fake_audio && !use_wav_audio {
        Some(fake_audio_capturer::BeepTrigger::new())
    } else {
        None
    };
    // WAV モードの場合は事前にファイルを開いて 48kHz モノラルにリサンプル済みのサンプル列を保持する
    let wav_source = if use_wav_audio {
        let wav_path = instance
            .input_wav
            .as_ref()
            .expect("use_wav_audio は input_wav の存在を含意する");
        Some(wav_reader::WavReader::open(wav_path)?)
    } else {
        None
    };

    // FakeAudioCapturer は内部スレッドから SoraConnectionContext 由来の AudioDeviceModule に
    // 触れ続けるため、Rust の RAII 逆順 Drop を利用して context より先に capturer を Drop させる
    // 必要がある。そのため context を先に宣言し、capturer は後で late-bind する。
    // context_config 構築時には capturer の audio_device_module() ハンドルが必要なので、
    // 構築ブロック内で一時生成して Option として持ち出す。
    let (context_config, pending_audio_capturer): (
        SoraConnectionContextConfig,
        Option<fake_audio_capturer::FakeAudioCapturer>,
    ) = {
        let mut config = SoraConnectionContextConfig {
            adm_config: AdmConfig::NoAudioDevice,
            ..Default::default()
        };

        // フェイク音声 ADM の登録 (WAV モードまたはビープモード)
        let fake_source = if let Some(reader) = wav_source {
            Some(fake_audio_capturer::FakeAudioSource::Wav(reader))
        } else {
            beep_trigger
                .as_ref()
                .map(|t| fake_audio_capturer::FakeAudioSource::Beep(t.clone()))
        };
        let pending = if let Some(source) = fake_source {
            let mut capturer = fake_audio_capturer::FakeAudioCapturer::new(source);
            capturer.start();
            config.adm_config = AdmConfig::UseExternal(capturer.audio_device_module());
            Some(capturer)
        } else {
            None
        };

        // MP4 パススルーコーデック能力の登録
        if let Some(ref reader) = mp4_reader {
            let mp4_capability: Box<dyn sora_sdk::VideoCodecCapability> =
                Box::new(Mp4PassthroughVideoCodecCapability::new(reader.codec_type()));
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

        // NopVideoDecoder の登録 (受信映像をデコードせず廃棄する)
        if instance.role.wants_recv() {
            let nop_capability: Box<dyn sora_sdk::VideoCodecCapability> =
                Box::new(nop_video_decoder::NopVideoDecoderCapability);
            let nop_preference = VideoCodecPreference::new_from_capability(nop_capability.as_ref());
            config.video_codec_preference.merge(&nop_preference);
            config.video_codec_capabilities.push(nop_capability);
        }

        (config, pending)
    };

    // context を先に宣言 (= Drop は最後)
    let context = SoraConnectionContext::new_with_config(context_config)?;

    // context より「後に」 capturer 系を宣言する (= Drop は context より先)
    // pending_audio_capturer を late-bind することで宣言順序を保つ
    let _fake_audio_capturer = pending_audio_capturer;
    let mut _fake_capturer: Option<FakeVideoCapturer> = None;
    let mut _device_capturer: Option<VideoDeviceCapturer> = None;
    let mut _mp4_capturer: Option<Mp4VideoCapturer> = None;
    let video_source = if !instance.no_video_device && instance.role.wants_send() {
        if let Some(reader) = mp4_reader {
            // MP4 パススルーキャプチャ
            let capturer = Mp4VideoCapturer::new(reader)
                .map_err(|e| ErrorMessage::new(format!("Failed to start MP4 capturer: {e}")))?;
            let source = capturer.video_source();
            _mp4_capturer = Some(capturer);
            Some(source)
        } else if let Some(ref device_name) = instance.video_input_device {
            // 実デバイスキャプチャ
            let device_id = resolve_device_id(device_name)?;
            let config = VideoDeviceCapturerConfig {
                device_id: Some(device_id),
                width: instance.resolution.0,
                height: instance.resolution.1,
                fps: instance.framerate as i32,
            };
            let mut capturer = VideoDeviceCapturer::new(config)?;
            capturer.start()?;
            let source = capturer.video_source();
            _device_capturer = Some(capturer);
            Some(source)
        } else {
            // フェイク映像キャプチャ
            let config = FakeVideoCapturerConfig {
                width: instance.resolution.0,
                height: instance.resolution.1,
                fps: instance.framerate as i32,
                sandstorm: instance.sandstorm,
                y4m_path: instance.input_y4m.as_ref().map(std::path::PathBuf::from),
                beep_trigger: beep_trigger.clone(),
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

    // DataChannel メッセージング設定のパース
    let (connect_data_channels, message_channels) =
        if let Some(ref dc_json) = instance.data_channels {
            let (connect, msg) = data_channel::parse_data_channels(dc_json)?;
            (Some(connect), msg)
        } else {
            (None, Vec::new())
        };

    // メタデータの JSON パース
    let metadata =
        if let Some(ref s) = instance.metadata {
            Some(s.parse::<JsonString>().map_err(|e| {
                ErrorMessage::new(format!("--sora-metadata の JSON が不正です: {e}"))
            })?)
        } else {
            None
        };
    let signaling_notify_metadata = if let Some(ref s) = instance.signaling_notify_metadata {
        Some(s.parse::<JsonString>().map_err(|e| {
            ErrorMessage::new(format!(
                "--sora-signaling-notify-metadata の JSON が不正です: {e}"
            ))
        })?)
    } else {
        None
    };

    let vc_config = VirtualClientConfig {
        signaling_urls: instance.signaling_urls.clone(),
        channel_id: instance.channel_id.clone(),
        role: instance.role,
        client_id: instance.client_id.clone(),
        bundle_id: instance.bundle_id.clone(),
        metadata,
        signaling_notify_metadata,
        duration: instance.duration,
        repeat_interval: instance.repeat_interval,
        max_retry: instance.max_retry,
        retry_interval: instance.retry_interval,
        video: build_video(&instance),
        audio: build_audio(&instance),
        connect_data_channels,
        message_channels,
        data_channel_signaling: instance.data_channel_signaling,
        ignore_disconnect_websocket: instance.ignore_disconnect_websocket,
        disconnect_wait_timeout: instance
            .disconnect_wait_timeout
            .map(Duration::from_secs_f64),
        simulcast: instance.simulcast,
        simulcast_request_rid: instance.simulcast_request_rid.clone(),
        spotlight: instance.spotlight,
        spotlight_focus_rid: instance.spotlight_focus_rid.clone(),
        spotlight_unfocus_rid: instance.spotlight_unfocus_rid.clone(),
        insecure: common.insecure,
        client_cert: client_cert_pem,
        client_key: client_key_pem,
        scenario: instance.scenario.map(scenario::build_scenario),
        duckdb_client,
        duckdb_interval,
    };

    // vcs-hatch-rate 制御の DelayQueue を構築
    let vc_hatch_start = tokio::time::Instant::now();
    let vc_interval = Duration::from_secs_f64(1.0 / instance.vcs_hatch_rate);
    let mut vc_delay: DelayQueue<u32> = DelayQueue::new();
    for i in 0..instance.vcs {
        vc_delay.insert(i, vc_interval * i);
    }

    let mut clients: JoinSet<()> = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            maybe_expired = vc_delay.next() => {
                let Some(expired) = maybe_expired else { break };
                let vc_id = expired.into_inner();
                rtc_log_info!(
                    "[i{}/vc-{}] starting virtual client (+{:.2}s)",
                    instance_id,
                    vc_id,
                    vc_hatch_start.elapsed().as_secs_f64(),
                );
                let child_token = token.child_token();
                clients.spawn_local(virtual_client::run(
                    instance_id,
                    vc_id,
                    context.clone(),
                    video_source.clone(),
                    vc_config.clone(),
                    child_token,
                    stats_tx.clone(),
                ));
            }
        }
    }

    // vc 群の完了を待機
    while let Some(result) = clients.join_next().await {
        if let Err(e) = result {
            rtc_log_warning!("[i{}] virtual client task panicked: {}", instance_id, e,);
        }
    }

    Ok(())
}
