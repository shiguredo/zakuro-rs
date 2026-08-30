mod args;
mod cmd_fmt;
mod cmd_lint;
mod data_channel;
mod diagnostic;
mod duckdb_stats;
mod error;
mod fake_audio_capturer;
mod fake_video_capturer;
mod http_server;
mod json_rpc;
mod jsonc_fmt;
mod mp4_audio;
mod nop_video_decoder;
mod openh264_video_codec;
mod scenario;
mod stats;
mod video_codec_capability;
mod video_device_capturer;
mod virtual_client;
mod wav_reader;
mod y4m_reader;

use std::process::ExitCode;
use std::time::Duration;

use shiguredo_openh264::Openh264Library;
use shiguredo_webrtc::{VideoCodecType, log, rtc_log_info, rtc_log_warning};
use sora_sdk::{
    AdmConfig, CodecDirection, JsonString, Mp4SampleReader, Mp4VideoCapturer,
    SoraConnectionContext, SoraConnectionContextConfig, VideoCodecCapability,
    VideoCodecImplementation, VideoCodecPreference,
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
    for device in &device_list {
        if let Ok(name) = device.name()
            && name == name_or_id
        {
            return Ok(device
                .unique_id()
                .unwrap_or_else(|_| name_or_id.to_string()));
        }
    }
    // ID として扱う
    for device in &device_list {
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
        Some("vp9") => Some(sora_sdk::Video::new_vp9(
            args.video_bit_rate,
            args.sora_video_vp9_params.clone(),
        )),
        Some("av1") => Some(sora_sdk::Video::new_av1(
            args.video_bit_rate,
            args.sora_video_av1_params.clone(),
        )),
        Some("h264") => Some(sora_sdk::Video::new_h264(
            args.video_bit_rate,
            args.sora_video_h264_params.clone(),
        )),
        Some("h265") => Some(sora_sdk::Video::new_h265(
            args.video_bit_rate,
            args.sora_video_h265_params.clone(),
        )),
        _ => {
            if args.video_bit_rate.is_some() {
                Some(sora_sdk::Video::new_vp8(args.video_bit_rate))
            } else {
                None
            }
        }
    }
}

/// MP4 パススルー時に、connect へ載せる映像コーデックパラメータをファイル実値で補完する。
///
/// Sora は offerer のため、offer の `profile-level-id` 等と bitstream 実値が合わないと
/// クライアント側の SDP answer で video m-line が reject される。
/// CLI / 設定で未指定のフィールドだけを、`passthrough_capability` の required format から埋める。
/// 明示指定されたフィールドは上書きしない。
fn apply_mp4_passthrough_video_params(instance: &mut InstanceArgs, reader: &Mp4SampleReader) {
    let mut formats = reader
        .passthrough_capability()
        .get_supported_formats(CodecDirection::Encoder);
    let Some(format) = formats.first_mut() else {
        return;
    };
    let params: std::collections::HashMap<String, String> =
        format.parameters_mut().iter().collect();

    match reader.codec_type() {
        VideoCodecType::H264 => {
            let Some(plid) = params.get("profile-level-id") else {
                return;
            };
            let mut h264 = instance.sora_video_h264_params.clone().unwrap_or_default();
            if h264.profile_level_id.is_some() {
                return;
            }
            rtc_log_info!(
                "MP4 passthrough: filling connect h264_params.profile_level_id={}",
                plid
            );
            h264.profile_level_id = Some(plid.clone());
            instance.sora_video_h264_params = Some(h264);
        }
        VideoCodecType::Av1 => {
            let mut av1 = instance.sora_video_av1_params.clone().unwrap_or_default();
            let mut filled = Vec::new();
            if av1.profile.is_none()
                && let Some(profile) = params.get("profile").and_then(|s| s.parse().ok())
            {
                av1.profile = Some(profile);
                filled.push(format!("profile={profile}"));
            }
            if av1.level_idx.is_none()
                && let Some(level_idx) = params.get("level-idx").and_then(|s| s.parse().ok())
            {
                av1.level_idx = Some(level_idx);
                filled.push(format!("level_idx={level_idx}"));
            }
            if av1.tier.is_none()
                && let Some(tier) = params.get("tier").and_then(|s| s.parse().ok())
            {
                av1.tier = Some(tier);
                filled.push(format!("tier={tier}"));
            }
            if filled.is_empty() {
                return;
            }
            rtc_log_info!(
                "MP4 passthrough: filling connect av1_params ({})",
                filled.join(", ")
            );
            instance.sora_video_av1_params = Some(av1);
        }
        // VP8 / VP9 / H265 のパススルー required format は現状追加パラメータを持たない。
        VideoCodecType::Vp8
        | VideoCodecType::Vp9
        | VideoCodecType::H265
        | VideoCodecType::Generic
        | VideoCodecType::Unknown(_) => {}
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

/// `--vp8-encoder` 等に指定された CLI 値から sora_sdk の実装名と説明文を解決する
///
/// 対応表は C++ 版 zakuro (互換目標) の `util.cpp` 内の `video_codec_implementation_map`
/// と同一である。値の許容は args.rs の `parse_video_codec_implementation` が行うので、
/// 許容値の変更時はあちらと同期させること。戻り値は VideoCodecPreference への
/// エントリ設定にそのまま使う。
fn resolve_video_codec_implementation(value: &str) -> (&'static str, &'static str) {
    match value {
        "internal" => ("internal", "WebRTC built-in VideoCodecFactory"),
        "cisco_openh264" => ("cisco_openh264", "OpenH264 Software Codec"),
        "intel_vpl" => ("vpl", "Intel VPL"),
        "nvidia_video_codec" => ("nvcodec", "NVIDIA NVENC/NVDEC"),
        "amd_amf" => ("amf", "AMD AMF"),
        // args.rs の parse_video_codec_implementation が 5 値のみ受理するため到達しない
        _ => unreachable!("unexpected video codec implementation: {value}"),
    }
}

/// コーデック個別のエンコーダー実装指定の spec リストを構築する
///
/// 起動時検証 (verify_video_encoder_implementation_specs) と instance 実行時の反映
/// (run_zakuro_instance) の両方でこの対応表を使い、コーデックとオプション名の対応を
/// 1 箇所に固定する。
fn encoder_implementation_specs(
    instance: &InstanceArgs,
) -> [(&'static str, VideoCodecType, Option<&str>); 5] {
    [
        (
            "vp8-encoder",
            VideoCodecType::Vp8,
            instance.vp8_encoder.as_deref(),
        ),
        (
            "vp9-encoder",
            VideoCodecType::Vp9,
            instance.vp9_encoder.as_deref(),
        ),
        (
            "av1-encoder",
            VideoCodecType::Av1,
            instance.av1_encoder.as_deref(),
        ),
        (
            "h264-encoder",
            VideoCodecType::H264,
            instance.h264_encoder.as_deref(),
        ),
        (
            "h265-encoder",
            VideoCodecType::H265,
            instance.h265_encoder.as_deref(),
        ),
    ]
}

/// コーデック別のエンコーダー実装指定を VideoCodecPreference に反映する
///
/// 指定された実装に対応する capability が登録されていない場合、または
/// その capability が指定コーデックのエンコーダーをサポートしていない場合は
/// エラーを返す (sora_sdk の validate_video_codec_preference が new_with_config で
/// 失敗するが、原因が分かるエラーメッセージを出すために事前に検証する)。
/// 反映対象は Encoder 方向のみで、Decoder 方向 (NopVideoDecoder 等) は変更しない。
fn apply_video_encoder_implementation_specs(
    preference: &mut VideoCodecPreference,
    capabilities: &[Box<dyn VideoCodecCapability>],
    specs: &[(&'static str, VideoCodecType, Option<&str>)],
) -> Result<()> {
    for (option_name, codec_type, value) in specs {
        let Some(value) = *value else { continue };
        let (implementation_name, description) = resolve_video_codec_implementation(value);
        let implementation = VideoCodecImplementation::new(implementation_name, description);
        // 実装名が capabilities に登録されていることと、指定コーデックのエンコーダーを
        // 提供できることの両方を確認する
        let capability = capabilities
            .iter()
            .find(|cap| cap.get_implementation().name() == implementation_name);
        let reason = if value == "cisco_openh264" {
            // Openh264VideoCodecCapability は Encoder 方向に必ず H.264 を提供するため、
            // capability 存在 + H.264 指定で is_supported=false にはならない。
            // また H.264 への cisco_openh264 指定は --openh264 未指定だと
            // parse_args_from_argv が先に拒否するため、ここに到達するのは
            // capability 登録済みの場合のみである (分岐は防御として残す)
            if capability.is_none() && *codec_type == VideoCodecType::H264 {
                "--openh264 を指定すると利用できます"
            } else {
                "OpenH264 は H.264 エンコーダーのみサポートします"
            }
        } else if capability.is_some() {
            // 実装は存在するが指定コーデックのエンコーダーを提供していない
            "この実装は指定したコーデックのエンコーダーをサポートしていません"
        } else {
            "対応するコーデック実装が登録されていません"
        };
        if !capability.is_some_and(|cap| cap.is_supported(CodecDirection::Encoder, *codec_type)) {
            return Err(ErrorMessage::new(format!(
                "--{option_name} に指定した実装 '{value}' は利用できません ({reason})"
            ))
            .into());
        }
        preference
            .get_or_add(CodecDirection::Encoder, *codec_type, implementation.clone())
            .set_implementation(implementation);
    }
    Ok(())
}

/// コーデック個別のエンコーダー実装指定を起動前に検証する
///
/// instance 起動後のコーデック検証エラーはインスタンス単位の警告でプロセスが継続するため、
/// 起動前に検証して無効な指定はプロセス全体のエラーにする。
/// capability の構成は run_zakuro_instance の構築ブロックと同じく
/// (既定 + OpenH264 + NopVideoDecoder) である。MP4 パススルーの capability は
/// エンコーダー実装指定と排他のため考える必要がない。
fn verify_video_encoder_implementation_specs(
    instances: &[InstanceArgs],
    openh264_lib: Option<&Openh264Library>,
) -> Result<()> {
    // エンコーダー実装指定を持つ instance が 1 つも無ければ検証は不要
    let has_spec = instances.iter().any(|i| {
        i.vp8_encoder.is_some()
            || i.vp9_encoder.is_some()
            || i.av1_encoder.is_some()
            || i.h264_encoder.is_some()
            || i.h265_encoder.is_some()
    });
    if !has_spec {
        return Ok(());
    }
    for instance in instances {
        // 検証用の config は apply 後の preference を破棄するため、validate は行わない
        let mut config = SoraConnectionContextConfig {
            adm_config: AdmConfig::NoAudioDevice,
            ..Default::default()
        };
        if let Some(lib) = openh264_lib {
            let capability: Box<dyn sora_sdk::VideoCodecCapability> = Box::new(
                openh264_video_codec::Openh264VideoCodecCapability::new((*lib).clone()),
            );
            config.video_codec_capabilities.push(capability);
        }
        if instance.role.wants_recv() {
            let capability: Box<dyn sora_sdk::VideoCodecCapability> =
                Box::new(nop_video_decoder::NopVideoDecoderCapability);
            config.video_codec_capabilities.push(capability);
        }
        apply_video_encoder_implementation_specs(
            &mut config.video_codec_preference,
            &config.video_codec_capabilities,
            &encoder_implementation_specs(instance),
        )?;
    }
    Ok(())
}

fn main() -> ExitCode {
    // `zakuro lint` / `zakuro fmt` は負荷試験を起動しない
    for try_run in [cmd_lint::try_run, cmd_fmt::try_run] {
        match try_run() {
            Ok(Some(code)) => return code,
            Ok(None) => {}
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
        }
    }

    match run_load_test() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run_load_test() -> Result<()> {
    // `FakeAudioCapturer` などの libwebrtc 由来オブジェクトが !Send のため、
    // instance ごとの future は LocalSet 上で `spawn_local` する必要がある
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| ErrorMessage::new(format!("Failed to build tokio runtime: {e}")))?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async_main())
}

async fn async_main() -> Result<()> {
    // libwebrtc のログ初期化は最初のログ出力前に 1 回だけ有効。
    // パース中の rtc_log_* より前に CLI / JSONC から --log-level を覗き見て適用する。
    // (旧 API の log_to_debug / enable_timestamps / enable_threads は削除された)
    {
        let early_log_level = args::peek_log_level();
        let mut log_config = log::LoggingConfig::new();
        log_config.set_min_severity(early_log_level);
        log_config.set_debug_severity(early_log_level);
        log_config.set_log_timestamp(true);
        log_config.set_log_thread(true);
        let _ = log::initialize_logging(log_config);
    }

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

    // コーデック個別のエンコーダー実装指定の事前検証
    // (instance 起動後の検証エラーはインスタンス単位の警告でプロセスが継続するため、
    //  起動前に検証して無効な指定はプロセス全体のエラーにする)
    verify_video_encoder_implementation_specs(&instance_args_vec, openh264_lib.as_ref())?;

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
    // 1 回目: 通常の graceful shutdown。2 回目: 何かにブロックしていても強制終了する。
    let shutdown_token = token.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        rtc_log_info!("Ctrl+C received, shutting down...");
        shutdown_token.cancel();

        let _ = tokio::signal::ctrl_c().await;
        rtc_log_warning!("Ctrl+C received again, forcing process exit");
        std::process::exit(130);
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
    mut instance: InstanceArgs,
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
            .map_err(|e| ErrorMessage::new(format!("MP4 ファイルの読み込みエラー: {e}")))?;
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
        // connect のコーデックパラメータを MP4 実値で補完し、Sora offer と揃える。
        apply_mp4_passthrough_video_params(&mut instance, &reader);
        Some(reader)
    } else {
        None
    };

    // フェイク音声キャプチャの初期化
    // 音声有効かつ送信ロールかつ --no-audio-device でないときに、
    // フェイク映像モードでは連続自動生成 PCM / WAV ファイル再生、
    // MP4 パススルー時は MP4 内の音声トラック再生を行う
    let wants_send_audio =
        !instance.no_audio_device && instance.audio && instance.role.wants_send();
    let use_fake_audio =
        wants_send_audio && instance.input_mp4.is_none() && instance.video_input_device.is_none();
    let use_wav_audio = use_fake_audio && instance.input_wav.is_some();
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

    // MP4 パススルー時の音声トラック調査
    // (--input-mp4 かつ送信ロールかつ音声有効かつ --no-audio-device でないときのみ。
    //  - 対応音声トラックが無い・未対応コーデックの場合は映像のみで続行する
    //  - 音声トラックが 2 本以上の場合は起動時エラーになる)
    let mp4_audio_result = if wants_send_audio {
        if let Some(ref mp4_path) = instance.input_mp4 {
            Some(mp4_audio::inspect_mp4_audio(
                std::path::Path::new(mp4_path),
                common.fdk_aac_lib.as_deref(),
            )?)
        } else {
            None
        }
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

        // フェイク音声 ADM の登録 (WAV / Safari 相当の連続自動生成 / MP4 音声)
        let fake_source = if let Some(reader) = wav_source {
            Some(fake_audio_capturer::FakeAudioSource::Wav(reader))
        } else if let Some(result) = mp4_audio_result {
            match result {
                mp4_audio::Mp4AudioTrackResult::Supported(source) => {
                    Some(fake_audio_capturer::FakeAudioSource::Mp4Audio(source))
                }
                // 音声トラックが無い・未対応の場合は映像のみで続行する (警告は調査時に出力済み)
                mp4_audio::Mp4AudioTrackResult::NoAudioTrack
                | mp4_audio::Mp4AudioTrackResult::Unsupported => None,
            }
        } else if use_fake_audio {
            Some(fake_audio_capturer::FakeAudioSource::Generated(
                fake_audio_capturer::GeneratedAudio::new(),
            ))
        } else {
            None
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
                Box::new(reader.passthrough_capability());
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

        // コーデック個別のエンコーダー実装指定 (--vp8-encoder 等) の反映
        apply_video_encoder_implementation_specs(
            &mut config.video_codec_preference,
            &config.video_codec_capabilities,
            &encoder_implementation_specs(&instance),
        )?;

        (config, pending)
    };

    // mp4_reader は capability 登録後も保持する。
    // sora_sdk 2026.2.0-canary.1 以降は Mp4SampleReader を Clone でき、
    // demux 結果とファイル I/O スレッドを instance 内で共有できる。
    // VC ごとの Mp4VideoCapturer には clone を渡す。

    // context を先に宣言 (= Drop は最後)
    let context = SoraConnectionContext::new_with_config(context_config)?;

    // context より「後に」 capturer 系を宣言する (= Drop は context より先)
    // pending_audio_capturer を late-bind することで宣言順序を保つ。
    // なお MP4 パススルー時の Mp4VideoCapturer は VC ごとに生成し
    // virtual_client::run のスコープで保持されるため、この宣言順序保証は非適用
    // (Mp4VideoCapturer は context 由来のリソースを持たないため実害はない)。
    let _fake_audio_capturer = pending_audio_capturer;
    let mut _fake_capturer: Option<FakeVideoCapturer> = None;
    let mut _device_capturer: Option<VideoDeviceCapturer> = None;
    // MP4 パススルー時は VC ごとに Mp4VideoCapturer を作るためこのスコープでは共有 video_source を持たない。
    // Mp4SampleReader は instance で 1 つ共有し、各 capturer へ clone する。
    let video_source = if !instance.no_video_device && instance.role.wants_send() {
        if instance.input_mp4.is_some() {
            // MP4 パススルーは VC ごとに個別の Mp4VideoCapturer / video_source を持つ。
            // (VideoFrameBuffer のスレッド固定制約のため video_source 自体は共有できない)
            None
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

                // MP4 パススルー時は instance 共有の Mp4SampleReader を clone し、
                // VC ごとに Mp4VideoCapturer (フィーダースレッド + video_source) を起動する。
                // video_source 自体を複数 encoder に共有すると
                // shiguredo_webrtc の VideoFrameBuffer スレッド固定チェックに引っかかり
                // panic するため、capturer / video_source は VC ごとに分離する。
                // Mp4SampleReader は Clone で demux 結果とファイル I/O スレッドを共有する。
                let (per_vc_source, per_vc_mp4_capturer) =
                    if let Some(ref reader) = mp4_reader {
                        if !instance.no_video_device && instance.role.wants_send() {
                            let capturer = Mp4VideoCapturer::new(reader.clone()).map_err(|e| {
                                ErrorMessage::new(format!(
                                    "vc-{vc_id} 用の MP4 キャプチャの開始エラー: {e}"
                                ))
                            })?;
                            let source = capturer.video_source();
                            (Some(source), Some(capturer))
                        } else {
                            // input_mp4 は指定されているが送信しない (recvonly 等) 場合は何もしない
                            (None, None)
                        }
                    } else {
                        (video_source.clone(), None)
                    };

                let child_token = token.child_token();
                clients.spawn_local(virtual_client::run(
                    instance_id,
                    vc_id,
                    context.clone(),
                    per_vc_source,
                    per_vc_mp4_capturer,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nop_video_decoder::NopVideoDecoderCapability;
    use sora_sdk::{InternalVideoCodecCapability, Role};

    /// テスト用の最小限の InstanceArgs を構築する
    fn minimal_instance_args() -> InstanceArgs {
        InstanceArgs {
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
            sora_video_vp9_params: None,
            sora_video_av1_params: None,
            sora_video_h264_params: None,
            sora_video_h265_params: None,
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
        }
    }

    #[test]
    fn build_video_omits_params_when_unspecified() {
        // params 未指定時は Video に params が含まれない (現行と同じ)
        let mut args = minimal_instance_args();
        args.video_codec_type = Some("vp9".into());
        let video = build_video(&args).expect("no_video_device=false では Video が返るべき");
        let json = nojson::Json(&video).to_string();
        assert!(
            !json.contains("vp9_params"),
            "params 未指定では vp9_params が含まれないべき: {json}"
        );
    }

    #[test]
    fn build_video_includes_parsed_params() {
        // params 指定時は Video の connect メッセージ用 JSON に各コーデックの params が含まれる
        let mut args = minimal_instance_args();
        args.video_codec_type = Some("vp9".into());
        args.sora_video_vp9_params = Some(sora_sdk::VideoVP9Params {
            profile_id: Some(2),
        });
        let video = build_video(&args).expect("no_video_device=false では Video が返るべき");
        let json = nojson::Json(&video).to_string();
        assert!(
            json.contains("\"vp9_params\""),
            "params 指定時は vp9_params が含まれるべき: {json}"
        );
        assert!(
            json.contains("\"profile_id\":2"),
            "profile_id が含まれるべき: {json}"
        );

        let mut args = minimal_instance_args();
        args.video_codec_type = Some("av1".into());
        args.sora_video_av1_params = Some(sora_sdk::VideoAV1Params {
            profile: Some(1),
            level_idx: Some(5),
            tier: Some(0),
        });
        let video = build_video(&args).expect("no_video_device=false では Video が返るべき");
        let json = nojson::Json(&video).to_string();
        assert!(
            json.contains("\"av1_params\"") && json.contains("\"profile\":1"),
            "params 指定時は av1_params (profile=1) が含まれるべき: {json}"
        );

        let mut args = minimal_instance_args();
        args.video_codec_type = Some("h264".into());
        args.sora_video_h264_params = Some(sora_sdk::VideoH264Params {
            profile_level_id: Some("42e01f".into()),
            b_frame: Some(true),
        });
        let video = build_video(&args).expect("no_video_device=false では Video が返るべき");
        let json = nojson::Json(&video).to_string();
        assert!(
            json.contains("\"h264_params\"") && json.contains("\"profile_level_id\":\"42e01f\""),
            "params 指定時は h264_params (profile_level_id) が含まれるべき: {json}"
        );

        let mut args = minimal_instance_args();
        args.video_codec_type = Some("h265".into());
        args.sora_video_h265_params = Some(sora_sdk::VideoH265Params {
            level_id: None,
            profile_id: Some(1),
            tier_flag: Some(0),
            tx_mode: Some("SRST".into()),
            b_frame: None,
        });
        let video = build_video(&args).expect("no_video_device=false では Video が返るべき");
        let json = nojson::Json(&video).to_string();
        assert!(
            json.contains("\"h265_params\"") && json.contains("\"tx_mode\":\"SRST\""),
            "params 指定時は h265_params (tx_mode=SRST) が含まれるべき: {json}"
        );
        assert!(
            !json.contains("level_id"),
            "level_id は出力されないべき: {json}"
        );
    }

    /// テスト用フィクスチャへのパスを返す。
    fn testdata(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name)
    }

    #[test]
    fn apply_mp4_passthrough_video_params_fills_h264_profile_level_id() {
        // H.264 MP4 で profile_level_id 未指定なら、avcC 由来の値を connect 用に埋める。
        // fixture は High Profile Level 2.1 (profile-level-id=640015)。
        let reader = Mp4SampleReader::new(testdata("mp4-video-h264.mp4"))
            .expect("H.264 フィクスチャを開けるはずです");
        let mut args = minimal_instance_args();
        args.video_codec_type = Some("h264".into());
        args.video_bit_rate = Some(1000);
        apply_mp4_passthrough_video_params(&mut args, &reader);

        let params = args
            .sora_video_h264_params
            .as_ref()
            .expect("h264_params が補完されるはずです");
        assert_eq!(
            params.profile_level_id.as_deref(),
            Some("640015"),
            "High Profile fixture の profile_level_id が載るはずです"
        );

        // build_video 経由でも connect JSON に含まれることを確認する。
        let video = build_video(&args).expect("Video が返るべきです");
        let json = nojson::Json(&video).to_string();
        assert!(
            json.contains("\"profile_level_id\":\"640015\""),
            "connect 用 Video JSON に profile_level_id が含まれるべき: {json}"
        );
    }

    #[test]
    fn apply_mp4_passthrough_video_params_keeps_explicit_h264_profile_level_id() {
        // CLI で明示した profile_level_id は MP4 実値で上書きしない。
        let reader = Mp4SampleReader::new(testdata("mp4-video-h264.mp4"))
            .expect("H.264 フィクスチャを開けるはずです");
        let mut args = minimal_instance_args();
        args.video_codec_type = Some("h264".into());
        args.sora_video_h264_params = Some(sora_sdk::VideoH264Params {
            profile_level_id: Some("42e01f".to_string()),
            b_frame: Some(true),
        });
        apply_mp4_passthrough_video_params(&mut args, &reader);

        let params = args
            .sora_video_h264_params
            .as_ref()
            .expect("明示指定の h264_params が残るはずです");
        assert_eq!(
            params.profile_level_id.as_deref(),
            Some("42e01f"),
            "明示指定の profile_level_id を保持するはずです"
        );
        assert_eq!(params.b_frame, Some(true), "b_frame も保持するはずです");
    }

    #[test]
    fn apply_mp4_passthrough_video_params_fills_h264_plid_when_only_b_frame_set() {
        // b_frame だけ指定されていても、未指定の profile_level_id は補完する。
        let reader = Mp4SampleReader::new(testdata("mp4-video-h264.mp4"))
            .expect("H.264 フィクスチャを開けるはずです");
        let mut args = minimal_instance_args();
        args.video_codec_type = Some("h264".into());
        args.sora_video_h264_params = Some(sora_sdk::VideoH264Params {
            profile_level_id: None,
            b_frame: Some(false),
        });
        apply_mp4_passthrough_video_params(&mut args, &reader);

        let params = args
            .sora_video_h264_params
            .as_ref()
            .expect("h264_params が残るはずです");
        assert_eq!(
            params.profile_level_id.as_deref(),
            Some("640015"),
            "未指定の profile_level_id だけ補完するはずです"
        );
        assert_eq!(
            params.b_frame,
            Some(false),
            "明示した b_frame は保持するはずです"
        );
    }

    #[test]
    fn apply_mp4_passthrough_video_params_fills_av1_params() {
        // AV1 MP4 で av1_params 未指定なら、av1C 由来の profile / level_idx / tier を埋める。
        let reader = Mp4SampleReader::new(testdata("mp4-video-av1.mp4"))
            .expect("AV1 フィクスチャを開けるはずです");
        let mut args = minimal_instance_args();
        args.video_codec_type = Some("av1".into());
        args.video_bit_rate = Some(1000);
        apply_mp4_passthrough_video_params(&mut args, &reader);

        let params = args
            .sora_video_av1_params
            .as_ref()
            .expect("av1_params が補完されるはずです");
        assert_eq!(
            params.profile,
            Some(0),
            "fixture の profile は 0 のはずです"
        );
        assert_eq!(
            params.level_idx,
            Some(0),
            "fixture の level_idx は 0 のはずです"
        );
        assert_eq!(params.tier, Some(0), "fixture の tier は 0 のはずです");
    }

    #[test]
    fn resolve_video_codec_implementation_maps_cpp_values() {
        // CLI 値が sora_sdk の実装名に解決されることを確認する
        let cases: [(&str, &str, &str); 5] = [
            ("internal", "internal", "WebRTC built-in VideoCodecFactory"),
            (
                "cisco_openh264",
                "cisco_openh264",
                "OpenH264 Software Codec",
            ),
            ("intel_vpl", "vpl", "Intel VPL"),
            ("nvidia_video_codec", "nvcodec", "NVIDIA NVENC/NVDEC"),
            ("amd_amf", "amf", "AMD AMF"),
        ];
        for (cli_value, expected_name, expected_description) in cases {
            let (name, description) = resolve_video_codec_implementation(cli_value);
            assert_eq!(name, expected_name, "CLI 値 '{cli_value}' の実装名が異なる");
            assert_eq!(
                description, expected_description,
                "CLI 値 '{cli_value}' の説明文が異なる"
            );
        }
    }

    #[test]
    fn apply_encoder_spec_sets_internal_implementation() {
        // capabilities に internal がある場合、指定した実装が Encoder 方向へ反映される
        let capabilities: Vec<Box<dyn VideoCodecCapability>> =
            vec![Box::new(InternalVideoCodecCapability::new())];
        let mut preference = VideoCodecPreference::default();
        apply_video_encoder_implementation_specs(
            &mut preference,
            &capabilities,
            &[("vp8-encoder", VideoCodecType::Vp8, Some("internal"))],
        )
        .expect("internal は capabilities にあるので適用できるべき");
        let codec = preference
            .find(CodecDirection::Encoder, VideoCodecType::Vp8)
            .expect("vp8 encoder エントリが追加されるべき");
        assert_eq!(
            codec.implementation().name(),
            "internal",
            "指定した実装が preference に反映されるべき"
        );
        assert!(
            preference
                .find(CodecDirection::Decoder, VideoCodecType::Vp8)
                .is_none(),
            "Decoder 方向には変更を加えないべき"
        );
    }

    #[test]
    fn apply_encoder_spec_rejects_cisco_openh264_without_lib() {
        // OpenH264 ライブラリ未ロード時は cisco_openh264 の capability が無くエラーになる
        let capabilities: Vec<Box<dyn VideoCodecCapability>> =
            vec![Box::new(InternalVideoCodecCapability::new())];
        let mut preference = VideoCodecPreference::default();
        let err = apply_video_encoder_implementation_specs(
            &mut preference,
            &capabilities,
            &[("h264-encoder", VideoCodecType::H264, Some("cisco_openh264"))],
        )
        .expect_err("OpenH264 未ロード時の cisco_openh264 指定は拒否されるべき");
        let msg = format!("{err}");
        assert!(
            msg.contains("--openh264"),
            "エラーメッセージに --openh264 の案内が含まれていない: {msg}"
        );
    }

    #[test]
    fn apply_encoder_spec_rejects_cisco_openh264_for_non_h264_codec() {
        // OpenH264 は H.264 エンコーダーのみ提供するため、他コーデックへの指定は拒否される
        let capabilities: Vec<Box<dyn VideoCodecCapability>> =
            vec![Box::new(InternalVideoCodecCapability::new())];
        let mut preference = VideoCodecPreference::default();
        let err = apply_video_encoder_implementation_specs(
            &mut preference,
            &capabilities,
            &[("vp9-encoder", VideoCodecType::Vp9, Some("cisco_openh264"))],
        )
        .expect_err("VP9 への cisco_openh264 指定は拒否されるべき");
        let msg = format!("{err}");
        assert!(
            msg.contains("H.264"),
            "OpenH264 が H.264 のみに対応する旨が含まれていない: {msg}"
        );
    }

    #[test]
    fn apply_encoder_spec_ignores_none_values() {
        // 未指定 (None) の場合はエラーにもならず、エントリも追加されない
        let capabilities: Vec<Box<dyn VideoCodecCapability>> =
            vec![Box::new(InternalVideoCodecCapability::new())];
        let mut preference = VideoCodecPreference::default();
        apply_video_encoder_implementation_specs(
            &mut preference,
            &capabilities,
            &[("vp8-encoder", VideoCodecType::Vp8, None)],
        )
        .expect("None の指定はエラーになるべきではない");
        assert!(
            preference
                .find(CodecDirection::Encoder, VideoCodecType::Vp8)
                .is_none(),
            "None の指定ではエントリを追加しないべき"
        );
    }

    #[test]
    fn apply_encoder_spec_rejects_unknown_implementation() {
        // capabilities に無い実装名はエラーになる (ハードウェア系は args で拒否済みだが防御として)
        let capabilities: Vec<Box<dyn VideoCodecCapability>> =
            vec![Box::new(NopVideoDecoderCapability)];
        let mut preference = VideoCodecPreference::default();
        let err = apply_video_encoder_implementation_specs(
            &mut preference,
            &capabilities,
            &[("h264-encoder", VideoCodecType::H264, Some("internal"))],
        )
        .expect_err("capabilities に無い実装は拒否されるべき");
        let msg = format!("{err}");
        assert!(
            msg.contains("h264-encoder"),
            "エラーメッセージにオプション名が含まれていない: {msg}"
        );
    }
}
