use shiguredo_webrtc::{
    EnvironmentRef, I420Buffer, SdpVideoFormat, SdpVideoFormatRef, VideoCodecStatus, VideoDecoder,
    VideoDecoderDecodedImageCallbackPtr, VideoDecoderHandler, VideoEncoder, VideoFrame,
};
use sora_sdk::{CodecDirection, VideoCodecCapability, VideoCodecImplementation};

/// 受信映像をデコードせず捨てるデコーダ
///
/// `decode` が `Ok` を返すだけでは、libwebrtc のフレームバッファは符号化フレームを
/// 復号完了とみなして解放しない。完了コールバックを同期的に呼ばないと、
/// 受信 RTP が接続中ずっと溜まり、視聴本数と時間に比例してヒープが膨らむ。
struct NopDecoder {
    callback: Option<VideoDecoderDecodedImageCallbackPtr>,
}

impl VideoDecoderHandler for NopDecoder {
    fn register_decode_complete_callback(
        &mut self,
        callback: Option<VideoDecoderDecodedImageCallbackPtr>,
    ) -> VideoCodecStatus {
        self.callback = callback;
        VideoCodecStatus::Ok
    }

    fn decode(
        &mut self,
        input_image: shiguredo_webrtc::EncodedImageRef<'_>,
        render_time_ms: i64,
    ) -> VideoCodecStatus {
        let Some(callback) = &self.callback else {
            return VideoCodecStatus::Ok;
        };
        // 中身は見ない。タイムスタンプだけ揃えた最小フレームで復号完了を通知し、
        // 符号化データをフレームバッファから捨てさせる。
        let buffer = I420Buffer::new(2, 2);
        let frame_buffer = buffer.cast_to_video_frame_buffer();
        let frame = VideoFrame::builder(&frame_buffer)
            .set_timestamp_us(render_time_ms.saturating_mul(1000))
            .set_rtp_timestamp(input_image.rtp_timestamp())
            .build();
        unsafe { callback.decoded(frame.as_ref()) };
        VideoCodecStatus::Ok
    }

    fn release(&mut self) -> VideoCodecStatus {
        self.callback = None;
        VideoCodecStatus::Ok
    }
}

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
        Some(VideoDecoder::new_with_handler(Box::new(NopDecoder {
            callback: None,
        })))
    }
}
