use shiguredo_webrtc::{
    EnvironmentRef, SdpVideoFormat, SdpVideoFormatRef, VideoDecoder, VideoDecoderHandler,
    VideoEncoder,
};
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

    fn get_supported_formats(&self, direction: CodecDirection) -> Vec<SdpVideoFormat> {
        // デコーダのみ全コーデック型をサポートする
        match direction {
            CodecDirection::Decoder => vec![
                SdpVideoFormat::new("VP8"),
                SdpVideoFormat::new("VP9"),
                SdpVideoFormat::new("AV1"),
                SdpVideoFormat::new("H264"),
                SdpVideoFormat::new("H265"),
            ],
            CodecDirection::Encoder => Vec::new(),
        }
    }

    fn create_video_encoder(
        &self,
        _env: EnvironmentRef<'_>,
        _format: SdpVideoFormatRef<'_>,
    ) -> Option<VideoEncoder> {
        None
    }

    fn create_video_decoder(
        &self,
        _env: EnvironmentRef<'_>,
        _format: SdpVideoFormatRef<'_>,
    ) -> Option<VideoDecoder> {
        Some(VideoDecoder::new_with_handler(Box::new(NopDecoder)))
    }
}
