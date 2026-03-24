use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use shiguredo_video_device::{PixelFormat, VideoCapture, VideoCaptureConfig};
use shiguredo_webrtc::{
    AdaptFrameResult, AdaptedVideoTrackSource, I420Buffer, TimestampAligner, VideoTrackSource,
};

use crate::error::Result;

pub(crate) struct VideoDeviceCapturerConfig {
    pub(crate) device_id: Option<String>,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) fps: i32,
}

pub(crate) struct VideoDeviceCapturer {
    _capture: VideoCapture,
    video_source: VideoTrackSource,
    started: Arc<AtomicBool>,
}

impl VideoDeviceCapturer {
    pub(crate) fn new(config: VideoDeviceCapturerConfig) -> Result<Self> {
        let source = AdaptedVideoTrackSource::new();
        let video_source = source.cast_to_video_track_source();
        let timestamp_aligner = TimestampAligner::new();
        let started = Arc::new(AtomicBool::new(false));

        let output_width = config.width;
        let output_height = config.height;

        // コールバック内で使うリソースを共有する
        let shared = Arc::new(std::sync::Mutex::new(CallbackState {
            source,
            timestamp_aligner,
        }));

        let capture_config = VideoCaptureConfig {
            device_id: config.device_id,
            width: config.width,
            height: config.height,
            fps: config.fps,
            pixel_format: None,
        };

        let capture = VideoCapture::new(capture_config, move |frame| {
            let i420 = match frame.pixel_format {
                PixelFormat::Nv12 => {
                    let Some(uv_data) = frame.uv_data else {
                        return;
                    };
                    shiguredo_webrtc::nv12_to_i420(
                        frame.data,
                        frame.stride,
                        uv_data,
                        frame.stride_uv,
                        frame.width,
                        frame.height,
                    )
                }
                PixelFormat::Yuy2 => shiguredo_webrtc::yuy2_to_i420(
                    frame.data,
                    frame.stride,
                    frame.width,
                    frame.height,
                ),
                PixelFormat::I420 => {
                    let Some(uv_data) = frame.uv_data else {
                        return;
                    };
                    // I420 データを I420Buffer にコピーする
                    let mut buf = I420Buffer::new(frame.width, frame.height);
                    let stride_y = buf.stride_y() as usize;
                    let stride_u = buf.stride_u() as usize;
                    let w = frame.width as usize;
                    let h = frame.height as usize;
                    let src_stride_y = frame.stride as usize;
                    let src_stride_uv = frame.stride_uv as usize;
                    let uv_h = h.div_ceil(2);
                    let uv_w = w.div_ceil(2);

                    // Y プレーン
                    {
                        let y_dst = buf.y_data_mut();
                        for row in 0..h {
                            y_dst[row * stride_y..row * stride_y + w].copy_from_slice(
                                &frame.data[row * src_stride_y..row * src_stride_y + w],
                            );
                        }
                    }
                    // U プレーン
                    {
                        let u_dst = buf.u_data_mut();
                        for row in 0..uv_h {
                            u_dst[row * stride_u..row * stride_u + uv_w].copy_from_slice(
                                &uv_data[row * src_stride_uv..row * src_stride_uv + uv_w],
                            );
                        }
                    }
                    // V プレーン
                    let stride_v = buf.stride_v() as usize;
                    {
                        let v_offset = src_stride_uv * uv_h;
                        let v_dst = buf.v_data_mut();
                        for row in 0..uv_h {
                            v_dst[row * stride_v..row * stride_v + uv_w].copy_from_slice(
                                &uv_data[v_offset + row * src_stride_uv
                                    ..v_offset + row * src_stride_uv + uv_w],
                            );
                        }
                    }
                    Some(buf)
                }
                PixelFormat::Unknown(_) => return,
            };

            let Some(i420) = i420 else {
                return;
            };

            // 出力解像度が異なる場合はスケーリングする
            let buffer = if i420.width() != output_width || i420.height() != output_height {
                let mut scaled = I420Buffer::new(output_width, output_height);
                scaled.scale_from(&i420);
                scaled
            } else {
                i420
            };

            let Ok(mut state) = shared.lock() else {
                return;
            };

            let timestamp_us = frame.timestamp_us;
            let AdaptFrameResult { applied, size } =
                state
                    .source
                    .adapt_frame(output_width, output_height, timestamp_us);
            if !applied {
                return;
            }

            let final_buffer =
                if size.adapted_width != output_width || size.adapted_height != output_height {
                    let mut scaled = I420Buffer::new(size.adapted_width, size.adapted_height);
                    scaled.scale_from(&buffer);
                    scaled
                } else {
                    buffer
                };

            let video_frame = shiguredo_webrtc::VideoFrame::from_i420(
                &final_buffer,
                state
                    .timestamp_aligner
                    .translate(timestamp_us, shiguredo_webrtc::time_millis() * 1000),
                0,
            );
            state.source.on_frame(&video_frame);
        })?;

        Ok(Self {
            _capture: capture,
            video_source,
            started,
        })
    }

    pub(crate) fn start(&mut self) -> Result<()> {
        if self.started.load(Ordering::Acquire) {
            return Ok(());
        }
        self._capture.start()?;
        self.started.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) fn video_source(&self) -> VideoTrackSource {
        self.video_source.clone()
    }
}

struct CallbackState {
    source: AdaptedVideoTrackSource,
    timestamp_aligner: TimestampAligner,
}
