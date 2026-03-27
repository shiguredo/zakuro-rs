use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use shiguredo_mp4::TrackKind;
use shiguredo_mp4::boxes::SampleEntry;
use shiguredo_mp4::demux::{Input, Mp4FileDemuxer};
use shiguredo_webrtc::{
    AdaptedVideoTrackSource, CodecSpecificInfo, EncodedImage, EncodedImageBuffer,
    H264PacketizationMode, I420Buffer, SdpVideoFormat, TimestampAligner, VideoCodecStatus,
    VideoCodecType, VideoDecoderHandler, VideoEncoderEncodedImageCallbackPtr,
    VideoEncoderEncodedImageCallbackRef, VideoEncoderEncoderInfo, VideoEncoderHandler, VideoFrame,
    VideoFrameRef, VideoFrameType, VideoFrameTypeVectorRef, VideoTrackSource, rtc_log_info,
};
use sora_sdk::{CodecDirection, VideoCodecCapability, VideoCodecImplementation};

use crate::error::{ErrorMessage, Result};

/// エンコード済みサンプル
#[derive(Clone)]
pub(crate) struct EncodedSample {
    data: Vec<u8>,
    is_keyframe: bool,
    width: u32,
    height: u32,
}

/// キャプチャスレッドとエンコーダ間のサンプル受け渡しスロット
pub(crate) type SharedSampleSlot = Arc<std::sync::Mutex<Option<EncodedSample>>>;

/// 新しいサンプルスロットを作成する
pub(crate) fn new_sample_slot() -> SharedSampleSlot {
    Arc::new(std::sync::Mutex::new(None))
}

/// MP4 ビデオトラック情報
struct Mp4VideoTrackInfo {
    codec_type: VideoCodecType,
    width: u16,
    height: u16,
    /// H.264 の SPS/PPS、H.265 の VPS/SPS/PPS を Annex B 形式で保持
    parameter_sets: Option<Vec<u8>>,
}

/// サンプルのメタデータ
struct SampleMeta {
    data_offset: u64,
    data_size: usize,
    keyframe: bool,
    duration: u32,
}

/// MP4 ファイルを読み込み、ビデオサンプルを抽出するリーダー
pub(crate) struct Mp4SampleReader {
    file_data: Vec<u8>,
    track_info: Mp4VideoTrackInfo,
    samples: Vec<SampleMeta>,
    /// 各サンプルの再生開始時刻（マイクロ秒）
    cumulative_us: Vec<u64>,
}

impl Mp4SampleReader {
    pub(crate) fn new(path: &str) -> Result<Self> {
        let file_data = std::fs::read(path)
            .map_err(|e| ErrorMessage::new(format!("Failed to read MP4 file '{}': {}", path, e)))?;

        let input = Input {
            position: 0,
            data: &file_data,
        };
        let mut demuxer = Mp4FileDemuxer::new();
        demuxer.handle_input(input);

        let tracks = demuxer
            .tracks()
            .map_err(|e| ErrorMessage::new(format!("Failed to get tracks from MP4 file: {}", e)))?;

        // ビデオトラックを探す
        let video_track = tracks
            .iter()
            .find(|t| t.kind == TrackKind::Video)
            .ok_or_else(|| ErrorMessage::new("MP4 file does not contain a video track"))?;

        let timescale = video_track.timescale.get();

        // 全ビデオサンプルを抽出する
        let mut samples = Vec::new();
        let mut first_sample_entry: Option<SampleEntry> = None;

        while let Some(sample) = demuxer
            .next_sample()
            .map_err(|e| ErrorMessage::new(format!("Failed to read MP4 sample: {}", e)))?
        {
            if sample.track.kind != TrackKind::Video {
                continue;
            }

            if let Some(entry) = sample.sample_entry
                && first_sample_entry.is_none()
            {
                first_sample_entry = Some(entry.clone());
            }

            samples.push(SampleMeta {
                data_offset: sample.data_offset,
                data_size: sample.data_size,
                keyframe: sample.keyframe,
                duration: sample.duration,
            });
        }

        if samples.is_empty() {
            return Err(ErrorMessage::new("MP4 file contains no video samples").into());
        }

        let sample_entry = first_sample_entry
            .ok_or_else(|| ErrorMessage::new("MP4 file has no video sample entry"))?;

        let track_info = Self::extract_track_info(&sample_entry)?;

        // 累積再生時刻テーブルを構築する（マイクロ秒単位）
        let mut cumulative_us = Vec::with_capacity(samples.len() + 1);
        cumulative_us.push(0);
        for sample in &samples {
            let duration_us = sample.duration as u64 * 1_000_000 / timescale as u64;
            let last = *cumulative_us.last().unwrap();
            cumulative_us.push(last + duration_us);
        }

        rtc_log_info!(
            "MP4 reader: {} video samples, codec={:?}, {}x{}, timescale={}",
            samples.len(),
            track_info.codec_type,
            track_info.width,
            track_info.height,
            timescale,
        );

        Ok(Self {
            file_data,
            track_info,
            samples,
            cumulative_us,
        })
    }

    /// SampleEntry からコーデック情報とパラメータセットを抽出する
    fn extract_track_info(entry: &SampleEntry) -> Result<Mp4VideoTrackInfo> {
        match entry {
            SampleEntry::Avc1(avc1) => {
                let parameter_sets = Self::build_h264_parameter_sets(&avc1.avcc_box);
                Ok(Mp4VideoTrackInfo {
                    codec_type: VideoCodecType::H264,
                    width: avc1.visual.width,
                    height: avc1.visual.height,
                    parameter_sets: Some(parameter_sets),
                })
            }
            SampleEntry::Hev1(hev1) => {
                let parameter_sets = Self::build_h265_parameter_sets(&hev1.hvcc_box);
                Ok(Mp4VideoTrackInfo {
                    codec_type: VideoCodecType::H265,
                    width: hev1.visual.width,
                    height: hev1.visual.height,
                    parameter_sets: Some(parameter_sets),
                })
            }
            SampleEntry::Hvc1(hvc1) => {
                let parameter_sets = Self::build_h265_parameter_sets(&hvc1.hvcc_box);
                Ok(Mp4VideoTrackInfo {
                    codec_type: VideoCodecType::H265,
                    width: hvc1.visual.width,
                    height: hvc1.visual.height,
                    parameter_sets: Some(parameter_sets),
                })
            }
            SampleEntry::Vp08(vp08) => Ok(Mp4VideoTrackInfo {
                codec_type: VideoCodecType::Vp8,
                width: vp08.visual.width,
                height: vp08.visual.height,
                parameter_sets: None,
            }),
            SampleEntry::Vp09(vp09) => Ok(Mp4VideoTrackInfo {
                codec_type: VideoCodecType::Vp9,
                width: vp09.visual.width,
                height: vp09.visual.height,
                parameter_sets: None,
            }),
            SampleEntry::Av01(av01) => Ok(Mp4VideoTrackInfo {
                codec_type: VideoCodecType::Av1,
                width: av01.visual.width,
                height: av01.visual.height,
                parameter_sets: None,
            }),
            _ => Err(ErrorMessage::new("Unsupported video codec in MP4 file").into()),
        }
    }

    /// H.264 の SPS/PPS を Annex B 形式で結合する
    fn build_h264_parameter_sets(avcc: &shiguredo_mp4::boxes::AvccBox) -> Vec<u8> {
        let mut out = Vec::new();
        for sps in &avcc.sps_list {
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(sps);
        }
        for pps in &avcc.pps_list {
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(pps);
        }
        out
    }

    /// H.265 の VPS/SPS/PPS を Annex B 形式で結合する
    fn build_h265_parameter_sets(hvcc: &shiguredo_mp4::boxes::HvccBox) -> Vec<u8> {
        let mut out = Vec::new();
        for nalu_array in &hvcc.nalu_arrays {
            for nalu in &nalu_array.nalus {
                out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                out.extend_from_slice(nalu);
            }
        }
        out
    }

    /// 長さプレフィックス形式の NALU を Annex B 形式に変換する
    fn length_prefixed_to_annex_b(data: &[u8], length_size: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut offset = 0;
        while offset + length_size <= data.len() {
            let nalu_len = match length_size {
                1 => data[offset] as usize,
                2 => u16::from_be_bytes([data[offset], data[offset + 1]]) as usize,
                4 => u32::from_be_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ]) as usize,
                _ => break,
            };
            offset += length_size;
            if offset + nalu_len > data.len() {
                break;
            }
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(&data[offset..offset + nalu_len]);
            offset += nalu_len;
        }
        out
    }

    pub(crate) fn codec_type(&self) -> VideoCodecType {
        self.track_info.codec_type
    }

    pub(crate) fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// 指定インデックスのサンプルを取得する
    pub(crate) fn get_sample(&self, index: usize) -> Option<EncodedSample> {
        let meta = self.samples.get(index)?;
        let raw_data =
            &self.file_data[meta.data_offset as usize..meta.data_offset as usize + meta.data_size];

        let data = match self.track_info.codec_type {
            VideoCodecType::H264 | VideoCodecType::H265 => {
                let length_size = 4;
                let mut annex_b = Vec::new();
                // キーフレームにはパラメータセットを先頭に付与する
                if meta.keyframe
                    && let Some(ref ps) = self.track_info.parameter_sets
                {
                    annex_b.extend_from_slice(ps);
                }
                annex_b.extend_from_slice(&Self::length_prefixed_to_annex_b(raw_data, length_size));
                annex_b
            }
            // VP8/VP9/AV1 はそのまま使用
            _ => raw_data.to_vec(),
        };

        Some(EncodedSample {
            data,
            is_keyframe: meta.keyframe,
            width: self.track_info.width as u32,
            height: self.track_info.height as u32,
        })
    }
}

/// MP4 映像キャプチャ
///
/// 専用スレッドで MP4 サンプルを読み出し、ダミーフレームを WebRTC に送信する。
/// エンコード済みデータは SharedSampleSlot 経由でパススルーエンコーダに渡す。
pub(crate) struct Mp4VideoCapturer {
    source: AdaptedVideoTrackSource,
    video_source: VideoTrackSource,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Mp4VideoCapturer {
    pub(crate) fn new() -> Self {
        let source = AdaptedVideoTrackSource::new();
        let video_source = source.cast_to_video_track_source();
        Self {
            source,
            video_source,
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    pub(crate) fn video_source(&self) -> VideoTrackSource {
        self.video_source.clone()
    }

    pub(crate) fn start(
        &mut self,
        reader: Mp4SampleReader,
        sample_slot: SharedSampleSlot,
    ) -> Result<()> {
        if self.handle.is_some() {
            return Ok(());
        }

        let mut source = self.source.clone();
        let stop = self.stop.clone();
        let width = reader.track_info.width as i32;
        let height = reader.track_info.height as i32;

        let handle = thread::Builder::new()
            .name("mp4-video-capturer".to_string())
            .spawn(move || {
                let mut timestamp_aligner = TimestampAligner::new();
                // ダミーフレーム用の最小 I420 バッファ
                let dummy_buffer = I420Buffer::new(width, height);

                loop {
                    let loop_start = Instant::now();

                    for i in 0..reader.sample_count() {
                        if stop.load(Ordering::Acquire) {
                            return;
                        }

                        if let Some(sample) = reader.get_sample(i) {
                            // サンプルをスロットに書き込む
                            {
                                let mut slot = sample_slot.lock().unwrap();
                                *slot = Some(sample);
                            }

                            // ダミー I420 フレームで encode() をトリガーする
                            let timestamp_us = shiguredo_webrtc::time_millis() * 1000;
                            let ts = timestamp_aligner
                                .translate(timestamp_us, shiguredo_webrtc::time_millis() * 1000);
                            let frame = VideoFrame::from_i420(&dummy_buffer, ts, 0);
                            source.on_frame(&frame);

                            // 次のフレームまで絶対時刻ベースで待機する（ドリフト防止）
                            if i + 1 < reader.sample_count() {
                                let target_us = reader.cumulative_us[i + 1];
                                let target_time = loop_start + Duration::from_micros(target_us);
                                let now = Instant::now();
                                if target_time > now {
                                    thread::sleep(target_time - now);
                                }
                            }
                        }
                    }

                    // EOF に達したらループ先頭に戻る
                    rtc_log_info!("MP4 capturer: reached end of file, looping");
                }
            })?;

        self.handle = Some(handle);
        Ok(())
    }
}

impl Drop for Mp4VideoCapturer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// MP4 パススルーエンコーダ
///
/// SharedSampleSlot からエンコード済みデータを取り出し、
/// WebRTC のコールバックに直接渡す（再エンコードしない）。
struct Mp4PassthroughEncoder {
    sample_slot: SharedSampleSlot,
    callback: Option<VideoEncoderEncodedImageCallbackPtr>,
    codec_type: VideoCodecType,
}

impl VideoEncoderHandler for Mp4PassthroughEncoder {
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
        _frame_types: Option<VideoFrameTypeVectorRef<'_>>,
    ) -> VideoCodecStatus {
        let callback = match &self.callback {
            Some(cb) => cb,
            None => return VideoCodecStatus::Uninitialized,
        };

        // スロットからサンプルを取り出す
        let sample = {
            let mut slot = self.sample_slot.lock().unwrap();
            slot.take()
        };

        let sample = match sample {
            Some(s) => s,
            None => return VideoCodecStatus::NoOutput,
        };

        let buffer = EncodedImageBuffer::from_bytes(&sample.data);
        let mut image = EncodedImage::new();
        image.set_encoded_data(&buffer);
        image.set_rtp_timestamp(frame.rtp_timestamp());
        image.set_encoded_width(sample.width);
        image.set_encoded_height(sample.height);
        image.set_frame_type(if sample.is_keyframe {
            VideoFrameType::Key
        } else {
            VideoFrameType::Delta
        });

        let mut codec_specific_info = CodecSpecificInfo::new();
        codec_specific_info.set_codec_type(self.codec_type);

        if self.codec_type == VideoCodecType::H264 {
            codec_specific_info.set_h264_packetization_mode(H264PacketizationMode::NonInterleaved);
            codec_specific_info.set_h264_idr_frame(sample.is_keyframe);
        }

        let result = unsafe {
            callback.on_encoded_image(image.as_ref(), Some(codec_specific_info.as_ref()))
        };
        if result.error() != shiguredo_webrtc::VideoEncoderEncodedImageCallbackResultError::Ok {
            return VideoCodecStatus::Error;
        }

        VideoCodecStatus::Ok
    }

    fn get_encoder_info(&mut self) -> VideoEncoderEncoderInfo {
        let mut info = VideoEncoderEncoderInfo::new();
        info.set_implementation_name("mp4-passthrough");
        // BWE が再エンコードを要求しないようにする
        info.set_has_trusted_rate_controller(true);
        info
    }
}

/// MP4 パススルーコーデック能力
///
/// MP4 から検出したコーデックのエンコーダのみを提供する。
pub(crate) struct Mp4PassthroughVideoCodecCapability {
    codec_type: VideoCodecType,
    sample_slot: SharedSampleSlot,
}

impl Mp4PassthroughVideoCodecCapability {
    pub(crate) fn new(codec_type: VideoCodecType, sample_slot: SharedSampleSlot) -> Self {
        Self {
            codec_type,
            sample_slot,
        }
    }
}

impl VideoCodecCapability for Mp4PassthroughVideoCodecCapability {
    fn get_implementation(&self) -> VideoCodecImplementation {
        VideoCodecImplementation::new("mp4-passthrough", "MP4 Passthrough Encoder")
    }

    fn is_supported(&self, direction: CodecDirection, codec_type: VideoCodecType) -> bool {
        direction == CodecDirection::Encoder && codec_type == self.codec_type
    }

    fn resolve_sdp_format(
        &self,
        direction: CodecDirection,
        codec_type: VideoCodecType,
        _parameters: &HashMap<String, String>,
        _scalability_mode: Option<&str>,
    ) -> Option<SdpVideoFormat> {
        if direction != CodecDirection::Encoder || codec_type != self.codec_type {
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
        Some(Box::new(Mp4PassthroughEncoder {
            sample_slot: self.sample_slot.clone(),
            callback: None,
            codec_type: self.codec_type,
        }))
    }

    fn create_video_decoder(
        &self,
        _format: &SdpVideoFormat,
    ) -> Option<Box<dyn VideoDecoderHandler>> {
        // パススルーではデコーダは不要
        None
    }
}

/// --input-mp4 で指定されたコーデック文字列を VideoCodecType に変換する
pub(crate) fn parse_video_codec_type(s: &str) -> Option<VideoCodecType> {
    match s {
        "vp8" => Some(VideoCodecType::Vp8),
        "vp9" => Some(VideoCodecType::Vp9),
        "av1" => Some(VideoCodecType::Av1),
        "h264" => Some(VideoCodecType::H264),
        "h265" => Some(VideoCodecType::H265),
        _ => None,
    }
}
