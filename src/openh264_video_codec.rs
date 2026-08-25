use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use shiguredo_openh264::{EncodeOptions, EncoderConfig, FrameType, Openh264Library};
use shiguredo_webrtc::{
    CodecSpecificInfo, EncodedImage, EncodedImageBuffer, EnvironmentRef, H264PacketizationMode,
    ScalabilityMode, SdpVideoFormat, SdpVideoFormatRef, VideoCodecStatus, VideoCodecType,
    VideoDecoder, VideoEncoder, VideoEncoderEncodedImageCallbackPtr,
    VideoEncoderEncodedImageCallbackRef, VideoEncoderEncoderInfo, VideoEncoderHandler,
    VideoEncoderRateControlParametersRef, VideoFrameRef, VideoFrameType, VideoFrameTypeVectorRef,
    rtc_log_info, rtc_log_warning,
};
use sora_sdk::{CodecDirection, VideoCodecCapability, VideoCodecImplementation};

use crate::error::{ErrorMessage, Result};

/// OpenH264 ライブラリをロードする
pub(crate) fn load_openh264_library(path: &str) -> Result<Openh264Library> {
    if !Path::new(path).exists() {
        return Err(ErrorMessage::new(format!("OpenH264 library not found: {path}")).into());
    }
    let lib = Openh264Library::load(path)
        .map_err(|e| ErrorMessage::new(format!("Failed to load OpenH264 library: {e}")))?;
    rtc_log_info!(
        "OpenH264 library loaded: path={}, version={}",
        lib.path().display(),
        lib.runtime_version()
    );
    Ok(lib)
}

/// OpenH264 エンコーダ
///
/// I420 フレームを受け取り、OpenH264 でエンコードして WebRTC コールバックに渡す。
struct Openh264Encoder {
    lib: Openh264Library,
    encoder: Option<shiguredo_openh264::Encoder>,
    callback: Option<VideoEncoderEncodedImageCallbackPtr>,
    width: i32,
    height: i32,
    bitrate_bps: u32,
    framerate: f64,
}

impl Openh264Encoder {
    fn new(lib: Openh264Library) -> Self {
        Self {
            lib,
            encoder: None,
            callback: None,
            width: 0,
            height: 0,
            bitrate_bps: 0,
            framerate: 30.0,
        }
    }

    /// エンコーダを初期化または再初期化する
    fn init_or_reconfigure(&mut self) -> VideoCodecStatus {
        if self.width <= 0 || self.height <= 0 {
            return VideoCodecStatus::Error;
        }

        let bitrate_bps = if self.bitrate_bps > 0 {
            self.bitrate_bps as usize
        } else {
            500_000
        };

        let fps = if self.framerate > 0.0 {
            self.framerate.round() as usize
        } else {
            30
        };

        let config = EncoderConfig::new(
            self.width as usize,
            self.height as usize,
            bitrate_bps,
            fps,
            1,
        );

        match shiguredo_openh264::Encoder::new(self.lib.clone(), config) {
            Ok(enc) => {
                self.encoder = Some(enc);
                VideoCodecStatus::Ok
            }
            Err(e) => {
                rtc_log_warning!("Failed to initialize OpenH264 encoder: {}", e);
                VideoCodecStatus::Error
            }
        }
    }

    /// I420 バッファからストライド分を除いた Y/U/V プレーンを抽出する
    ///
    /// OpenH264 はストライドが幅と一致する I420 データを要求するため、
    /// WebRTC の I420Buffer のストライドが幅と異なる場合はコピーが必要。
    fn extract_i420_planes(
        &self,
        frame: &VideoFrameRef<'_>,
    ) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let buffer = frame.buffer();
        let i420 = buffer.as_i420()?;
        let width = i420.width() as usize;
        let height = i420.height() as usize;
        let stride_y = i420.stride_y() as usize;
        let stride_u = i420.stride_u() as usize;
        let stride_v = i420.stride_v() as usize;
        let uv_height = height.div_ceil(2);
        let uv_width = width.div_ceil(2);

        let y_data = i420.y_data();
        let u_data = i420.u_data();
        let v_data = i420.v_data();

        if stride_y == width && stride_u == uv_width && stride_v == uv_width {
            // ストライドが幅と一致する場合はそのままコピー
            let y = y_data[..width * height].to_vec();
            let u = u_data[..uv_width * uv_height].to_vec();
            let v = v_data[..uv_width * uv_height].to_vec();
            Some((y, u, v))
        } else {
            // ストライドが異なる場合は行ごとにコピー
            let mut y = Vec::with_capacity(width * height);
            for row in 0..height {
                let start = row * stride_y;
                y.extend_from_slice(&y_data[start..start + width]);
            }
            let mut u = Vec::with_capacity(uv_width * uv_height);
            for row in 0..uv_height {
                let start = row * stride_u;
                u.extend_from_slice(&u_data[start..start + uv_width]);
            }
            let mut v = Vec::with_capacity(uv_width * uv_height);
            for row in 0..uv_height {
                let start = row * stride_v;
                v.extend_from_slice(&v_data[start..start + uv_width]);
            }
            Some((y, u, v))
        }
    }
}

impl VideoEncoderHandler for Openh264Encoder {
    fn init_encode(
        &mut self,
        codec_settings: shiguredo_webrtc::VideoCodecRef<'_>,
        _settings: shiguredo_webrtc::VideoEncoderSettingsRef<'_>,
    ) -> VideoCodecStatus {
        self.width = codec_settings.width();
        self.height = codec_settings.height();
        self.bitrate_bps = codec_settings.start_bitrate_kbps() * 1000;
        self.framerate = codec_settings.max_framerate() as f64;
        self.init_or_reconfigure()
    }

    fn register_encode_complete_callback(
        &mut self,
        callback: Option<VideoEncoderEncodedImageCallbackRef<'_>>,
    ) -> VideoCodecStatus {
        self.callback =
            callback.map(|cb| unsafe { VideoEncoderEncodedImageCallbackPtr::from_ref(cb) });
        VideoCodecStatus::Ok
    }

    fn encode(
        &mut self,
        frame: VideoFrameRef<'_>,
        frame_types: Option<VideoFrameTypeVectorRef<'_>>,
    ) -> VideoCodecStatus {
        if self.callback.is_none() || self.encoder.is_none() {
            return VideoCodecStatus::Uninitialized;
        }

        // 解像度変更の検出
        let frame_width = frame.width();
        let frame_height = frame.height();
        if frame_width != self.width || frame_height != self.height {
            self.width = frame_width;
            self.height = frame_height;
            let status = self.init_or_reconfigure();
            if status != VideoCodecStatus::Ok {
                return status;
            }
        }

        // キーフレーム要求の検出
        let force_idr = frame_types.is_some_and(|ft| {
            !ft.is_empty() && ft.get(0).is_some_and(|t| t == VideoFrameType::Key)
        });

        let (y, u, v) = match self.extract_i420_planes(&frame) {
            Some(planes) => planes,
            None => return VideoCodecStatus::Error,
        };

        let options = EncodeOptions { force_idr };
        let encoder = self.encoder.as_mut().unwrap();
        let encoded = match encoder.encode(&y, &u, &v, &options) {
            Ok(Some(encoded)) => encoded,
            Ok(None) => return VideoCodecStatus::NoOutput,
            Err(e) => {
                rtc_log_warning!("OpenH264 encode failed: {}", e);
                return VideoCodecStatus::Error;
            }
        };

        // SPS/PPS + データを結合して Annex B ストリームを構築する
        let mut bitstream = Vec::new();
        for sps in &encoded.sps_list {
            bitstream.extend_from_slice(&[0, 0, 0, 1]);
            bitstream.extend_from_slice(sps);
        }
        for pps in &encoded.pps_list {
            bitstream.extend_from_slice(&[0, 0, 0, 1]);
            bitstream.extend_from_slice(pps);
        }
        bitstream.extend_from_slice(&encoded.data);

        let is_keyframe = matches!(encoded.frame_type, FrameType::Idr | FrameType::I);

        let buffer = EncodedImageBuffer::from_bytes(&bitstream);
        let mut image = EncodedImage::new();
        image.set_encoded_data(&buffer);
        image.set_rtp_timestamp(frame.rtp_timestamp());
        image.set_encoded_width(self.width as u32);
        image.set_encoded_height(self.height as u32);
        image.set_frame_type(if is_keyframe {
            VideoFrameType::Key
        } else {
            VideoFrameType::Delta
        });

        let mut codec_specific_info = CodecSpecificInfo::new();
        codec_specific_info.set_codec_type(VideoCodecType::H264);
        codec_specific_info.set_h264_packetization_mode(H264PacketizationMode::NonInterleaved);
        codec_specific_info.set_h264_idr_frame(encoded.frame_type == FrameType::Idr);

        let callback = self.callback.as_ref().unwrap();
        let result = unsafe {
            callback.on_encoded_image(image.as_ref(), Some(codec_specific_info.as_ref()))
        };
        if result.error() != shiguredo_webrtc::VideoEncoderEncodedImageCallbackResultError::Ok {
            return VideoCodecStatus::Error;
        }

        VideoCodecStatus::Ok
    }

    fn release(&mut self) -> VideoCodecStatus {
        self.encoder = None;
        VideoCodecStatus::Ok
    }

    fn set_rates(&mut self, parameters: VideoEncoderRateControlParametersRef<'_>) {
        let new_bitrate = parameters.target_bitrate_sum_bps();
        let new_framerate = parameters.framerate_fps();

        if let Some(encoder) = &mut self.encoder {
            if new_bitrate > 0 && new_bitrate != self.bitrate_bps {
                if let Err(e) = encoder.set_bitrate(new_bitrate as usize) {
                    rtc_log_warning!("Failed to set OpenH264 bitrate: {}", e);
                }
                self.bitrate_bps = new_bitrate;
            }

            if new_framerate > 0.0 && (new_framerate - self.framerate).abs() > 0.5 {
                let fps = new_framerate.round() as usize;
                if let Err(e) = encoder.set_frame_rate(fps, 1) {
                    rtc_log_warning!("Failed to set OpenH264 frame rate: {}", e);
                }
                self.framerate = new_framerate;
            }
        }
    }

    fn get_encoder_info(&mut self) -> VideoEncoderEncoderInfo {
        let mut info = VideoEncoderEncoderInfo::new();
        // この名前は WebRTC 統計 (encoderImplementation) に載る情報であり、
        // capability 照合用の実装名 (cisco_openh264) とは役割が異なるため変えない
        info.set_implementation_name("openh264");
        info
    }
}

/// OpenH264 エンコーダの SDP format 広告を構築する
///
/// エンコーダは RFC 6184 §8.1 の NonInterleaved (packetization-mode=1) で出力するため、
/// 広告もそれに揃える。パラメータ構成は sora_sdk の openh264_supported_formats と
/// 同一にして交渉結果と出力の整合を保証する。RFC 6184 の扱いは将来変更される
/// 可能性があるため、変更時は sora_sdk 側の実装と整合を取ること。
fn openh264_encoder_formats() -> Vec<SdpVideoFormat> {
    vec![SdpVideoFormat::new_with_parameters(
        "H264",
        &HashMap::from([
            (String::from("level-asymmetry-allowed"), String::from("1")),
            (String::from("packetization-mode"), String::from("1")),
        ]),
        &[ScalabilityMode::L1T1],
    )]
}

/// OpenH264 コーデック能力
///
/// H.264 エンコーダとして OpenH264 を提供する。
pub(crate) struct Openh264VideoCodecCapability {
    lib: Arc<Openh264Library>,
}

impl Openh264VideoCodecCapability {
    pub(crate) fn new(lib: Openh264Library) -> Self {
        Self { lib: Arc::new(lib) }
    }
}

impl VideoCodecCapability for Openh264VideoCodecCapability {
    fn get_implementation(&self) -> VideoCodecImplementation {
        // 実装名は C++ 版 zakuro の `cisco_openh264` (--h264-encoder の許容値) と揃える。
        // これにより --h264-encoder cisco_openh264 での実装指定が照合できる。
        VideoCodecImplementation::new("cisco_openh264", "OpenH264 Software Codec")
    }

    fn get_supported_formats(&self, direction: CodecDirection) -> Vec<SdpVideoFormat> {
        // H.264 のエンコーダのみサポートする (デコーダは将来対応)
        match direction {
            CodecDirection::Encoder => openh264_encoder_formats(),
            CodecDirection::Decoder => Vec::new(),
        }
    }

    fn create_video_encoder(
        &self,
        _env: EnvironmentRef<'_>,
        _format: SdpVideoFormatRef<'_>,
    ) -> Option<VideoEncoder> {
        Some(VideoEncoder::new_with_handler(Box::new(
            Openh264Encoder::new((*self.lib).clone()),
        )))
    }

    fn create_video_decoder(
        &self,
        _env: EnvironmentRef<'_>,
        _format: SdpVideoFormatRef<'_>,
    ) -> Option<VideoDecoder> {
        // デコーダは将来対応
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openh264_encoder_formats_advertises_packetization_mode_1() {
        // エンコーダ出力 (NonInterleaved) と広告を一致させるため、
        // packetization-mode=1 と level-asymmetry-allowed=1 を広告する
        let formats = openh264_encoder_formats();
        assert_eq!(formats.len(), 1, "H.264 の format は 1 件のみであるべき");
        let format = &formats[0];
        assert_eq!(
            format.as_ref().name().expect("format 名の取得に失敗"),
            "H264",
            "コーデック名は H264 であるべき"
        );
        let params = format
            .as_ref()
            .parameters_mut()
            .iter()
            .collect::<HashMap<String, String>>();
        assert_eq!(
            params.get("packetization-mode").map(String::as_str),
            Some("1"),
            "packetization-mode=1 を広告するべき"
        );
        assert_eq!(
            params.get("level-asymmetry-allowed").map(String::as_str),
            Some("1"),
            "level-asymmetry-allowed=1 を広告するべき"
        );
    }

    #[test]
    fn openh264_encoder_formats_resolves_bare_h264_request() {
        // パラメータ付き広告に変更しても、既定の is_supported (bare H264 要求を
        // fuzzy match で解決) が壊れないことを確認する。解決結果は交渉に使われる
        // 広告そのものなので、packetization-mode=1 が返ることも合わせて確認する
        let formats = openh264_encoder_formats();
        let mut matched = shiguredo_webrtc::fuzzy_match_sdp_video_format(
            &formats,
            SdpVideoFormat::new("H264").as_ref(),
        )
        .expect("bare H264 要求がパラメータ付き広告と解決されるべき");
        let params = matched
            .parameters_mut()
            .iter()
            .collect::<HashMap<String, String>>();
        assert_eq!(
            params.get("packetization-mode").map(String::as_str),
            Some("1"),
            "解決結果も packetization-mode=1 を広告するべき"
        );

        // Sora サーバーが H264 を profile-level-id 付きで offer するケースでも
        // 広告と解決できることを確認する
        let resolved_with_profile_level_id = shiguredo_webrtc::fuzzy_match_sdp_video_format(
            &formats,
            SdpVideoFormat::new_with_parameters(
                "H264",
                &HashMap::from([(String::from("profile-level-id"), String::from("42e01f"))]),
                &[],
            )
            .as_ref(),
        );
        assert!(
            resolved_with_profile_level_id.is_some(),
            "profile-level-id 付き要求も解決されるべき"
        );
    }

    #[test]
    fn openh264_encoder_formats_advertises_scalability_l1t1() {
        // sora_sdk の既定実装と同一の ScalabilityMode を広告する
        let formats = openh264_encoder_formats();
        let modes = formats[0].scalability_modes();
        assert!(
            modes.contains(&ScalabilityMode::L1T1),
            "ScalabilityMode::L1T1 を広告するべき: {modes:?}"
        );
    }
}
