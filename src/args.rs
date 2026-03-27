use shiguredo_webrtc::rtc_log_info;
use sora_sdk::Role;

use crate::error::{ErrorMessage, Result};

pub(crate) struct Args {
    pub(crate) signaling_urls: Vec<String>,
    pub(crate) channel_id: String,
    pub(crate) role: Role,
    pub(crate) vcs: u32,
    pub(crate) vcs_hatch_rate: f64,
    pub(crate) duration: Option<f64>,
    pub(crate) repeat_interval: Option<f64>,
    pub(crate) max_retry: u32,
    pub(crate) retry_interval: f64,
    pub(crate) no_video_device: bool,
    pub(crate) no_audio_device: bool,
    pub(crate) video_input_device: Option<String>,
    pub(crate) resolution: (i32, i32),
    pub(crate) framerate: u32,
    pub(crate) sandstorm: bool,
    pub(crate) fake_video_capture: Option<String>,
    pub(crate) input_mp4: Option<String>,
    pub(crate) openh264: Option<String>,
    pub(crate) video_codec_type: Option<String>,
    pub(crate) video_bit_rate: Option<u32>,
    pub(crate) audio: bool,
    pub(crate) audio_codec_type: Option<String>,
    pub(crate) audio_bit_rate: Option<u32>,
    pub(crate) data_channels: Option<String>,
    pub(crate) data_channel_signaling: Option<bool>,
    pub(crate) ignore_disconnect_websocket: Option<bool>,
    pub(crate) simulcast: Option<bool>,
    pub(crate) simulcast_request_rid: Option<String>,
    pub(crate) spotlight: Option<bool>,
    pub(crate) spotlight_focus_rid: Option<String>,
    pub(crate) spotlight_unfocus_rid: Option<String>,
    pub(crate) http_host: Option<String>,
    pub(crate) http_port: Option<u16>,
}

fn parse_resolution(s: &str) -> Result<(i32, i32)> {
    match s {
        "QVGA" => Ok((320, 240)),
        "VGA" => Ok((640, 480)),
        "HD" => Ok((1280, 720)),
        "FHD" => Ok((1920, 1080)),
        "4K" => Ok((3840, 2160)),
        _ => {
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() != 2 {
                return Err(ErrorMessage::new(format!(
                    "resolution は QVGA/VGA/HD/FHD/4K または WxH で指定してください: {s}"
                ))
                .into());
            }
            let width: i32 = parts[0].parse().map_err(|_| {
                ErrorMessage::new(format!("resolution の幅が不正です: {}", parts[0]))
            })?;
            let height: i32 = parts[1].parse().map_err(|_| {
                ErrorMessage::new(format!("resolution の高さが不正です: {}", parts[1]))
            })?;
            if width <= 0 || height <= 0 {
                return Err(
                    ErrorMessage::new("resolution の幅と高さは正の整数で指定してください").into(),
                );
            }
            Ok((width, height))
        }
    }
}

pub(crate) fn parse_args() -> Result<Args> {
    let mut args = noargs::raw_args();
    args.metadata_mut().app_name = env!("CARGO_PKG_NAME");
    args.metadata_mut().app_description = "Sora WebRTC SFU 負荷試験ツール";

    if noargs::VERSION_FLAG.take(&mut args).is_present() {
        rtc_log_info!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    noargs::HELP_FLAG.take_help(&mut args);

    let signaling_urls: Vec<String> = noargs::opt("sora-signaling-url")
        .doc("Sora の WebSocket シグナリング URL (カンマ区切りで複数指定可)")
        .example("wss://sora.example.com/signaling")
        .take(&mut args)
        .then(|o| Ok::<_, &str>(o.value().split(',').map(|s| s.trim().to_string()).collect()))?;

    let channel_id: String = noargs::opt("sora-channel-id")
        .doc("Sora のチャネル ID")
        .example("zakuro-test")
        .take(&mut args)
        .then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let role: String = noargs::opt("sora-role")
        .doc("Sora のロール (sendonly, recvonly, sendrecv)")
        .example("sendonly")
        .take(&mut args)
        .then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let vcs: u32 = noargs::opt("vcs")
        .doc("仮想クライアント数 (1-1000, デフォルト: 1)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(1);

    let vcs_hatch_rate: f64 = noargs::opt("vcs-hatch-rate")
        .doc("仮想クライアントの起動レート (秒あたりの起動数, デフォルト: 1.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(1.0);

    let duration: Option<f64> = noargs::opt("duration")
        .doc("仮想クライアントの接続維持秒数 (省略時は無制限)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?;

    let repeat_interval: Option<f64> = noargs::opt("repeat-interval")
        .doc("duration 経過後の再接続間隔 (秒)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?;

    let max_retry: u32 = noargs::opt("max-retry")
        .doc("接続失敗時の最大リトライ回数 (デフォルト: 0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(0);

    let retry_interval: f64 = noargs::opt("retry-interval")
        .doc("リトライ間隔 (秒, デフォルト: 60.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(60.0);

    let no_video_device = noargs::flag("no-video-device")
        .doc("映像デバイスを使用しない")
        .take(&mut args)
        .is_present();

    let no_audio_device = noargs::flag("no-audio-device")
        .doc("音声デバイスを使用しない")
        .take(&mut args)
        .is_present();

    let video_input_device: Option<String> = noargs::opt("video-input-device")
        .doc("映像入力デバイス名または ID")
        .example("FaceTime HD Camera")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let resolution: (i32, i32) = noargs::opt("resolution")
        .doc("映像解像度 (QVGA/VGA/HD/FHD/4K または WxH, デフォルト: VGA)")
        .take(&mut args)
        .present_and_then(|o| parse_resolution(o.value()))?
        .unwrap_or((640, 480));

    let framerate: u32 = noargs::opt("framerate")
        .doc("映像フレームレート (1-60, デフォルト: 30)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(30);

    let sandstorm = noargs::flag("sandstorm")
        .doc("砂嵐映像を生成する")
        .take(&mut args)
        .is_present();

    let fake_video_capture: Option<String> = noargs::opt("fake-video-capture")
        .doc("Y4M 動画ファイルからフェイク映像を生成する")
        .example("video.y4m")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !std::path::Path::new(&path).exists() {
                return Err("fake-video-capture: file not found");
            }
            Ok(path)
        })?;

    let input_mp4: Option<String> = noargs::opt("input-mp4")
        .doc("MP4 ファイルからエンコード済み映像をパススルー送信する")
        .example("video.mp4")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !std::path::Path::new(&path).exists() {
                return Err("input-mp4: file not found");
            }
            Ok(path)
        })?;

    let openh264: Option<String> = noargs::opt("openh264")
        .doc("OpenH264 共有ライブラリのパス")
        .example("libopenh264-2.6.0-mac-arm64.dylib")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !std::path::Path::new(&path).exists() {
                return Err("openh264: library file not found");
            }
            Ok(path)
        })?;

    let video_codec_type: Option<String> = noargs::opt("sora-video-codec-type")
        .doc("映像コーデック (vp8/vp9/av1/h264/h265)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "vp8" | "vp9" | "av1" | "h264" | "h265" => Ok(o.value().to_string()),
            _ => Err("sora-video-codec-type は vp8/vp9/av1/h264/h265 で指定してください"),
        })?;

    let video_bit_rate: Option<u32> = noargs::opt("sora-video-bit-rate")
        .doc("映像ビットレート (kbps)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?;

    let audio: bool = noargs::opt("sora-audio")
        .doc("音声の有効/無効 (true/false, デフォルト: true)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-audio は true または false で指定してください"),
        })?
        .unwrap_or(true);

    let audio_codec_type: Option<String> = noargs::opt("sora-audio-codec-type")
        .doc("音声コーデック (opus)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "opus" => Ok(o.value().to_string()),
            _ => Err("sora-audio-codec-type は opus で指定してください"),
        })?;

    let audio_bit_rate: Option<u32> = noargs::opt("sora-audio-bit-rate")
        .doc("音声ビットレート (kbps)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?;

    let data_channels: Option<String> = noargs::opt("sora-data-channels")
        .doc("DataChannel メッセージング設定 (JSON 文字列)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let data_channel_signaling: Option<bool> = noargs::opt("sora-data-channel-signaling")
        .doc("DataChannel 経由でシグナリングを行う (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-data-channel-signaling は true または false で指定してください"),
        })?;

    let ignore_disconnect_websocket: Option<bool> = noargs::opt("sora-ignore-disconnect-websocket")
        .doc("DataChannel 使用時に WebSocket 切断を無視する (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-ignore-disconnect-websocket は true または false で指定してください"),
        })?;

    let simulcast: Option<bool> = noargs::opt("sora-simulcast")
        .doc("サイマルキャストの有効/無効 (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-simulcast は true または false で指定してください"),
        })?;

    let simulcast_request_rid: Option<String> = noargs::opt("sora-simulcast-request-rid")
        .doc("サイマルキャストで受信する rid (r0/r1/r2)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let spotlight: Option<bool> = noargs::opt("sora-spotlight")
        .doc("スポットライトの有効/無効 (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-spotlight は true または false で指定してください"),
        })?;

    let spotlight_focus_rid: Option<String> = noargs::opt("sora-spotlight-focus-rid")
        .doc("スポットライトでフォーカス時の rid (r0/r1/r2)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let spotlight_unfocus_rid: Option<String> = noargs::opt("sora-spotlight-unfocus-rid")
        .doc("スポットライトでアンフォーカス時の rid (r0/r1/r2)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let http_host: Option<String> = noargs::opt("http-host")
        .doc("HTTP サーバーのホストアドレス")
        .example("0.0.0.0")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let http_port: Option<u16> = noargs::opt("http-port")
        .doc("HTTP サーバーのポート番号")
        .example("8080")
        .take(&mut args)
        .present_and_then(|o| {
            o.value()
                .parse::<u16>()
                .map_err(|_| "http-port は 0-65535 の整数で指定してください")
        })?;

    if let Some(help) = args.finish()? {
        print!("{}", help);
        std::process::exit(0);
    }

    let role = Role::parse(&role)?;

    // バリデーション
    if vcs == 0 || vcs > 1000 {
        return Err(ErrorMessage::new("vcs は 1 から 1000 の範囲で指定してください").into());
    }
    if vcs_hatch_rate <= 0.0 {
        return Err(ErrorMessage::new("vcs-hatch-rate は正の数で指定してください").into());
    }
    if framerate == 0 || framerate > 60 {
        return Err(ErrorMessage::new("framerate は 1 から 60 の範囲で指定してください").into());
    }
    if sandstorm && fake_video_capture.is_some() {
        return Err(ErrorMessage::new(
            "--sandstorm と --fake-video-capture は同時に指定できません",
        )
        .into());
    }
    if video_input_device.is_some() && fake_video_capture.is_some() {
        return Err(ErrorMessage::new(
            "--video-input-device と --fake-video-capture は同時に指定できません",
        )
        .into());
    }
    if video_input_device.is_some() && sandstorm {
        return Err(ErrorMessage::new(
            "--video-input-device と --sandstorm は同時に指定できません",
        )
        .into());
    }
    if input_mp4.is_some() && video_input_device.is_some() {
        return Err(ErrorMessage::new(
            "--input-mp4 と --video-input-device は同時に指定できません",
        )
        .into());
    }
    if input_mp4.is_some() && fake_video_capture.is_some() {
        return Err(ErrorMessage::new(
            "--input-mp4 と --fake-video-capture は同時に指定できません",
        )
        .into());
    }
    if input_mp4.is_some() && sandstorm {
        return Err(ErrorMessage::new("--input-mp4 と --sandstorm は同時に指定できません").into());
    }
    if http_host.is_some() != http_port.is_some() {
        return Err(
            ErrorMessage::new("--http-host と --http-port は両方指定する必要があります").into(),
        );
    }

    if input_mp4.is_some() && video_codec_type.is_none() {
        return Err(ErrorMessage::new(
            "--input-mp4 使用時は --sora-video-codec-type の指定が必須です",
        )
        .into());
    }
    if input_mp4.is_some() && video_bit_rate.is_none() {
        return Err(ErrorMessage::new(
            "--input-mp4 使用時は --sora-video-bit-rate の指定が必須です",
        )
        .into());
    }

    Ok(Args {
        signaling_urls,
        channel_id,
        role,
        vcs,
        vcs_hatch_rate,
        duration,
        repeat_interval,
        max_retry,
        retry_interval,
        no_video_device,
        no_audio_device,
        video_input_device,
        resolution,
        framerate,
        sandstorm,
        fake_video_capture,
        input_mp4,
        openh264,
        video_codec_type,
        video_bit_rate,
        audio,
        audio_codec_type,
        audio_bit_rate,
        data_channels,
        data_channel_signaling,
        ignore_disconnect_websocket,
        simulcast,
        simulcast_request_rid,
        spotlight,
        spotlight_focus_rid,
        spotlight_unfocus_rid,
        http_host,
        http_port,
    })
}
