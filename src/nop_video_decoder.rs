use std::collections::HashMap;

use shiguredo_webrtc::{SdpVideoFormat, VideoCodecType, VideoDecoderHandler, VideoEncoderHandler};
use sora_sdk::{CodecDirection, VideoCodecCapability, VideoCodecImplementation};

/// 受信映像廃棄デコーダ
///
/// デコード処理をスキップして受信フレームを即座に廃棄する。
/// VideoDecoderHandler のデフォルト実装がそのまま Nop 動作になる。
struct NopDecoder;

impl VideoDecoderHandler for NopDecoder {}

/// NopVideoDecoder コーデック能力
///
/// 全コーデック型に対して NopDecoder を提供する。
pub(crate) struct NopVideoDecoderCapability;

impl VideoCodecCapability for NopVideoDecoderCapability {
    fn get_implementation(&self) -> VideoCodecImplementation {
        VideoCodecImplementation::new("nop", "Nop Video Decoder")
    }

    fn is_supported(&self, direction: CodecDirection, _codec_type: VideoCodecType) -> bool {
        direction == CodecDirection::Decoder
    }

    fn resolve_sdp_format(
        &self,
        direction: CodecDirection,
        codec_type: VideoCodecType,
        _parameters: &HashMap<String, String>,
        _scalability_mode: Option<&str>,
    ) -> Option<SdpVideoFormat> {
        if direction != CodecDirection::Decoder {
            return None;
        }
        let name = match codec_type {
            VideoCodecType::Vp8 => "VP8",
            VideoCodecType::Vp9 => "VP9",
            VideoCodecType::Av1 => "AV1",
            VideoCodecType::H264 => "H264",
            VideoCodecType::H265 => "H265",
            _ => return None,
        };
        Some(SdpVideoFormat::new(name))
    }

    fn create_video_encoder(
        &self,
        _format: &SdpVideoFormat,
    ) -> Option<Box<dyn VideoEncoderHandler>> {
        None
    }

    fn create_video_decoder(
        &self,
        _format: &SdpVideoFormat,
    ) -> Option<Box<dyn VideoDecoderHandler>> {
        Some(Box::new(NopDecoder))
    }
}
