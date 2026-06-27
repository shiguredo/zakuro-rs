use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use raden::{Circle, Context, Image, Path, PipelineRuntime, PixelFormat, Rect, Rgba32};
use shiguredo_webrtc::{
    AdaptFrameResult, AdaptedVideoTrackSource, TimestampAligner, VideoTrackSource,
};

use crate::error::Result;
use crate::fake_audio_capturer::BeepTrigger;
use crate::y4m_reader::Y4mReader;

pub(crate) struct FakeVideoCapturerConfig {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) fps: i32,
    pub(crate) sandstorm: bool,
    pub(crate) y4m_path: Option<PathBuf>,
    pub(crate) beep_trigger: Option<BeepTrigger>,
}

pub(crate) struct FakeVideoCapturer {
    source: AdaptedVideoTrackSource,
    timestamp_aligner: Option<TimestampAligner>,
    image: Option<ImageHolder>,
    width: i32,
    height: i32,
    fps: i32,
    sandstorm: bool,
    start_time_ms: i64,
    video_source: VideoTrackSource,
    beep_trigger: Option<BeepTrigger>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

enum ImageHolder {
    Raden(Image, Box<PipelineRuntime>),
    Sandstorm(Vec<u32>),
    Y4m(Y4mReader, Vec<u8>),
}

/// ImageHolder が Y4m バリアントかどうかを判定する
fn is_y4m(image: &ImageHolder) -> bool {
    matches!(image, ImageHolder::Y4m(..))
}

impl FakeVideoCapturer {
    pub(crate) fn new(config: FakeVideoCapturerConfig) -> Result<Self> {
        let width = if config.width > 0 { config.width } else { 640 };
        let height = if config.height > 0 {
            config.height
        } else {
            480
        };
        let fps = if config.fps > 0 { config.fps } else { 30 };
        let source = AdaptedVideoTrackSource::new();
        let timestamp_aligner = TimestampAligner::new();
        let video_source = source.cast_to_video_track_source();
        let image = if let Some(ref y4m_path) = config.y4m_path {
            let reader = Y4mReader::open(y4m_path.as_path())?;
            let buf = vec![0u8; reader.frame_size()];
            ImageHolder::Y4m(reader, buf)
        } else if config.sandstorm {
            ImageHolder::Sandstorm(vec![0u32; (width * height) as usize])
        } else {
            let img = Image::new(width as u32, height as u32, PixelFormat::Prgb32);
            let runtime = Box::new(PipelineRuntime::new());
            ImageHolder::Raden(img, runtime)
        };
        Ok(Self {
            image: Some(image),
            width,
            height,
            fps,
            sandstorm: config.sandstorm,
            start_time_ms: shiguredo_webrtc::time_millis(),
            video_source,
            beep_trigger: config.beep_trigger,
            source,
            timestamp_aligner: Some(timestamp_aligner),
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        })
    }

    pub(crate) fn video_source(&self) -> VideoTrackSource {
        self.video_source.clone()
    }

    pub(crate) fn start(&mut self) -> Result<()> {
        if self.handle.is_some() {
            return Ok(());
        }
        let mut source = self.source.clone();
        let mut timestamp_aligner = match self.timestamp_aligner.take() {
            Some(t) => t,
            None => return Ok(()),
        };
        let mut image = match self.image.take() {
            Some(i) => i,
            None => return Ok(()),
        };
        let width = self.width;
        let height = self.height;
        let fps = self.fps.max(1);
        let start_time_ms = self.start_time_ms;
        let sandstorm = self.sandstorm;
        let has_y4m = is_y4m(&image);
        let beep_trigger = self.beep_trigger.take();
        let stop = self.stop.clone();
        let handle = thread::Builder::new()
            .name("fake-video-capturer".to_string())
            .spawn(move || {
                let mut frame_counter: u32 = 0;
                let mut xorshift_state: u32 = 0xDEAD_BEEF;
                while !stop.load(Ordering::Acquire) {
                    if has_y4m {
                        if let ImageHolder::Y4m(ref mut reader, ref mut buf) = image {
                            tick_y4m(
                                &mut source,
                                &mut timestamp_aligner,
                                reader,
                                buf,
                                width,
                                height,
                                start_time_ms,
                            );
                        }
                    } else if sandstorm {
                        if let ImageHolder::Sandstorm(ref mut buf) = image {
                            tick_sandstorm(
                                &mut source,
                                &mut timestamp_aligner,
                                buf,
                                width,
                                height,
                                start_time_ms,
                                &mut xorshift_state,
                            );
                        }
                    } else if let ImageHolder::Raden(ref mut img, ref mut runtime) = image {
                        tick_raden(
                            &mut source,
                            &mut timestamp_aligner,
                            img,
                            runtime,
                            width,
                            height,
                            fps,
                            start_time_ms,
                            frame_counter,
                            &beep_trigger,
                        );
                    }
                    let sleep_ms = (1000 / fps).saturating_sub(2).max(1);
                    std::thread::sleep(std::time::Duration::from_millis(sleep_ms as u64));
                    frame_counter = frame_counter.wrapping_add(1);
                }
            })?;
        self.handle = Some(handle);
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for FakeVideoCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn u32_slice_as_u8_slice(data: &[u32]) -> &[u8] {
    let len = std::mem::size_of_val(data);
    let ptr = data.as_ptr() as *const u8;
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

fn tick_y4m(
    source: &mut AdaptedVideoTrackSource,
    timestamp_aligner: &mut TimestampAligner,
    reader: &mut Y4mReader,
    buf: &mut [u8],
    output_width: i32,
    output_height: i32,
    start_time_ms: i64,
) {
    let elapsed_ms = shiguredo_webrtc::time_millis() - start_time_ms;

    // フレームを取得。同一フレームの場合は None が返る
    let updated = match reader.get_frame(elapsed_ms, buf) {
        Ok(Some(())) => true,
        Ok(None) => false,
        Err(_) => return,
    };

    if !updated {
        return;
    }

    let y4m_width = reader.width();
    let y4m_height = reader.height();

    // Y4M から読んだ I420 データを I420Buffer にコピーする
    let y_size = (y4m_width * y4m_height) as usize;
    let uv_width = ((y4m_width + 1) / 2) as usize;
    let uv_height = ((y4m_height + 1) / 2) as usize;
    let uv_size = uv_width * uv_height;

    let mut i420 = shiguredo_webrtc::I420Buffer::new(y4m_width, y4m_height);

    // Y プレーン: stride が幅と異なる場合があるため行ごとにコピー
    let stride_y = i420.stride_y() as usize;
    let w = y4m_width as usize;
    let h = y4m_height as usize;
    {
        let y_dst = i420.y_data_mut();
        for row in 0..h {
            y_dst[row * stride_y..row * stride_y + w].copy_from_slice(&buf[row * w..row * w + w]);
        }
    }

    // U プレーン
    let stride_u = i420.stride_u() as usize;
    {
        let u_dst = i420.u_data_mut();
        for row in 0..uv_height {
            u_dst[row * stride_u..row * stride_u + uv_width]
                .copy_from_slice(&buf[y_size + row * uv_width..y_size + row * uv_width + uv_width]);
        }
    }

    // V プレーン
    let stride_v = i420.stride_v() as usize;
    {
        let v_dst = i420.v_data_mut();
        for row in 0..uv_height {
            v_dst[row * stride_v..row * stride_v + uv_width].copy_from_slice(
                &buf[y_size + uv_size + row * uv_width
                    ..y_size + uv_size + row * uv_width + uv_width],
            );
        }
    }

    // 出力解像度が Y4M と異なる場合はスケーリング
    let buffer = if y4m_width != output_width || y4m_height != output_height {
        let mut scaled = shiguredo_webrtc::I420Buffer::new(output_width, output_height);
        scaled.scale_from(&i420);
        scaled
    } else {
        i420
    };

    let timestamp_us = elapsed_ms * 1000;
    send_frame(
        source,
        timestamp_aligner,
        &buffer,
        output_width,
        output_height,
        timestamp_us,
    );
}

fn tick_sandstorm(
    source: &mut AdaptedVideoTrackSource,
    timestamp_aligner: &mut TimestampAligner,
    buf: &mut [u32],
    width: i32,
    height: i32,
    start_time_ms: i64,
    xorshift_state: &mut u32,
) {
    let elapsed_ms = shiguredo_webrtc::time_millis() - start_time_ms;

    for pixel in buf.iter_mut() {
        let val = xorshift32(xorshift_state);
        let gray = (val & 0xFF) as u8;
        *pixel = 0xFF00_0000 | (gray as u32) << 16 | (gray as u32) << 8 | (gray as u32);
    }

    let mut buffer = shiguredo_webrtc::I420Buffer::new(width, height);
    let stride_y = buffer.stride_y();
    let stride_u = buffer.stride_u();
    let stride_v = buffer.stride_v();
    {
        let (y, u, v) = buffer.planes_mut();
        if !shiguredo_webrtc::abgr_to_i420(
            u32_slice_as_u8_slice(buf),
            width * 4,
            y,
            stride_y,
            u,
            stride_u,
            v,
            stride_v,
            width,
            height,
        ) {
            return;
        }
    }

    let timestamp_us = elapsed_ms * 1000;
    send_frame(
        source,
        timestamp_aligner,
        &buffer,
        width,
        height,
        timestamp_us,
    );
}

#[allow(clippy::too_many_arguments)]
fn tick_raden(
    source: &mut AdaptedVideoTrackSource,
    timestamp_aligner: &mut TimestampAligner,
    image: &mut Image,
    runtime: &mut PipelineRuntime,
    width: i32,
    height: i32,
    fps: i32,
    start_time_ms: i64,
    frame_counter: u32,
    beep_trigger: &Option<BeepTrigger>,
) {
    let elapsed_ms = shiguredo_webrtc::time_millis() - start_time_ms;

    let mut ctx = Context::new(image, runtime);

    ctx.set_fill_style(Rgba32::rgb(0, 0, 0));
    ctx.fill_all();

    ctx.save();
    draw_digital_clock(&mut ctx, width, height, elapsed_ms);
    ctx.restore();

    ctx.save();
    draw_animations(&mut ctx, width, height, fps, frame_counter, beep_trigger);
    ctx.restore();

    ctx.save();
    draw_boxes(&mut ctx, width, height, frame_counter);
    ctx.restore();

    ctx.end();

    let pixel_data = image.data();

    let mut buffer = shiguredo_webrtc::I420Buffer::new(width, height);
    let stride_y = buffer.stride_y();
    let stride_u = buffer.stride_u();
    let stride_v = buffer.stride_v();
    {
        let (y, u, v) = buffer.planes_mut();
        if !shiguredo_webrtc::abgr_to_i420(
            pixel_data,
            width * 4,
            y,
            stride_y,
            u,
            stride_u,
            v,
            stride_v,
            width,
            height,
        ) {
            return;
        }
    }

    let timestamp_us = elapsed_ms * 1000;
    send_frame(
        source,
        timestamp_aligner,
        &buffer,
        width,
        height,
        timestamp_us,
    );
}

fn send_frame(
    source: &mut AdaptedVideoTrackSource,
    timestamp_aligner: &mut TimestampAligner,
    buffer: &shiguredo_webrtc::I420Buffer,
    width: i32,
    height: i32,
    timestamp_us: i64,
) {
    let AdaptFrameResult { applied, size } = source.adapt_frame(width, height, timestamp_us);
    let translated_ts =
        timestamp_aligner.translate(timestamp_us, shiguredo_webrtc::time_millis() * 1000);
    let frame = if applied && (size.adapted_width != width || size.adapted_height != height) {
        let mut scaled = shiguredo_webrtc::I420Buffer::new(size.adapted_width, size.adapted_height);
        scaled.scale_from(buffer);
        let vfb = scaled.cast_to_video_frame_buffer();
        shiguredo_webrtc::VideoFrame::builder(&vfb)
            .set_timestamp_us(translated_ts)
            .set_rtp_timestamp(0)
            .build()
    } else {
        let vfb = buffer.cast_to_video_frame_buffer();
        shiguredo_webrtc::VideoFrame::builder(&vfb)
            .set_timestamp_us(translated_ts)
            .set_rtp_timestamp(0)
            .build()
    };
    source.on_frame(&frame);
}

fn draw_animations(
    ctx: &mut Context<'_>,
    width: i32,
    height: i32,
    fps: i32,
    frame_counter: u32,
    beep_trigger: &Option<BeepTrigger>,
) {
    let w = width as f64;
    let h = height as f64;

    ctx.translate(w * 0.5, h * 0.5);
    ctx.rotate(-PI / 2.0);

    ctx.set_fill_style(Rgba32::rgb(255, 255, 255));
    ctx.fill_pie(&raden::Arc::new(0.0, 0.0, w * 0.3, w * 0.3, 0.0, 2.0 * PI));

    ctx.set_fill_style(Rgba32::rgb(160, 160, 160));
    let sweep = (frame_counter % fps as u32) as f64 / fps as f64 * 2.0 * PI;
    ctx.fill_pie(&raden::Arc::new(0.0, 0.0, w * 0.3, w * 0.3, 0.0, sweep));

    // パイチャートが一周したときにビープ音をトリガーする
    if frame_counter.is_multiple_of(fps as u32)
        && let Some(trigger) = beep_trigger
    {
        trigger.trigger();
    }
}

fn draw_boxes(ctx: &mut Context<'_>, width: i32, height: i32, frame_counter: u32) {
    let w = width as f64;
    let h = height as f64;

    let box_size = 50.0;
    let num_boxes = 5;

    for i in 0..num_boxes {
        let phase = ((frame_counter + i * 20) % 100) as f64 / 100.0;
        let x = phase * (w - box_size);
        let y = h * 0.5 + (phase * PI * 2.0).sin() * h * 0.2;

        let color = match i % 5 {
            0 => Rgba32::rgb(255, 0, 0),
            1 => Rgba32::rgb(0, 255, 0),
            2 => Rgba32::rgb(0, 0, 255),
            3 => Rgba32::rgb(255, 255, 0),
            4 => Rgba32::rgb(255, 0, 255),
            _ => Rgba32::rgb(255, 255, 255),
        };

        ctx.set_fill_style(color);
        ctx.fill_rect(&Rect::new(x, y, box_size, box_size));
    }
}

fn draw_digital_clock(ctx: &mut Context<'_>, width: i32, height: i32, elapsed_ms: i64) {
    let w = width as f64;
    let h = height as f64;

    let hours = (elapsed_ms / (60 * 60 * 1000)) % 10000;
    let minutes = (elapsed_ms / (60 * 1000)) % 60;
    let seconds = (elapsed_ms / 1000) % 60;
    let milliseconds = elapsed_ms % 1000;

    let clock_x = w * 0.02;
    let clock_y = h * 0.02;
    let digit_width = w * 0.018;
    let digit_height = h * 0.04;
    let spacing = digit_width * 0.3;
    let colon_width = digit_width * 0.3;

    ctx.set_fill_style(Rgba32::rgb(0, 255, 255));

    let mut x = clock_x;

    // 時間（4 桁）
    draw_7segment(
        ctx,
        ((hours / 1000) % 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;
    draw_7segment(
        ctx,
        ((hours / 100) % 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;
    draw_7segment(
        ctx,
        ((hours / 10) % 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;
    draw_7segment(
        ctx,
        (hours % 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;

    draw_colon(ctx, x, clock_y, digit_height);
    x += colon_width + spacing;

    // 分（2 桁）
    draw_7segment(
        ctx,
        (minutes / 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;
    draw_7segment(
        ctx,
        (minutes % 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;

    draw_colon(ctx, x, clock_y, digit_height);
    x += colon_width + spacing;

    // 秒（2 桁）
    draw_7segment(
        ctx,
        (seconds / 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;
    draw_7segment(
        ctx,
        (seconds % 10) as i32,
        x,
        clock_y,
        digit_width,
        digit_height,
    );
    x += digit_width + spacing;

    // ドット
    ctx.fill_circle(&Circle::new(
        x + colon_width * 0.3,
        clock_y + digit_height * 0.8,
        digit_height * 0.05,
    ));
    x += colon_width + spacing;

    // ミリ秒（3 桁）
    let ms_digit_width = digit_width * 0.7;
    let ms_digit_height = digit_height * 0.7;

    ctx.set_fill_style(Rgba32::rgb(200, 200, 200));
    draw_7segment(
        ctx,
        ((milliseconds / 100) % 10) as i32,
        x,
        clock_y + (digit_height - ms_digit_height) / 2.0,
        ms_digit_width,
        ms_digit_height,
    );
    x += ms_digit_width + spacing * 0.8;
    draw_7segment(
        ctx,
        ((milliseconds / 10) % 10) as i32,
        x,
        clock_y + (digit_height - ms_digit_height) / 2.0,
        ms_digit_width,
        ms_digit_height,
    );
    x += ms_digit_width + spacing * 0.8;
    draw_7segment(
        ctx,
        (milliseconds % 10) as i32,
        x,
        clock_y + (digit_height - ms_digit_height) / 2.0,
        ms_digit_width,
        ms_digit_height,
    );
}

fn draw_7segment(ctx: &mut Context<'_>, digit: i32, x: f64, y: f64, width: f64, height: f64) {
    let thickness = width * 0.15;
    let gap = thickness * 0.2;

    let segments: [[bool; 7]; 10] = [
        [true, true, true, true, true, true, false],     // 0
        [false, true, true, false, false, false, false], // 1
        [true, true, false, true, true, false, true],    // 2
        [true, true, true, true, false, false, true],    // 3
        [false, true, true, false, false, true, true],   // 4
        [true, false, true, true, false, true, true],    // 5
        [true, false, true, true, true, true, true],     // 6
        [true, true, true, false, false, false, false],  // 7
        [true, true, true, true, true, true, true],      // 8
        [true, true, true, true, false, true, true],     // 9
    ];

    if !(0..=9).contains(&digit) {
        return;
    }
    let seg = &segments[digit as usize];

    let draw_horizontal = |ctx: &mut Context<'_>, sx: f64, sy: f64| {
        let mut path = Path::new();
        path.move_to(sx + gap, sy);
        path.line_to(sx + width - gap, sy);
        path.line_to(sx + width - gap - thickness * 0.5, sy + thickness * 0.5);
        path.line_to(sx + width - gap, sy + thickness);
        path.line_to(sx + gap, sy + thickness);
        path.line_to(sx + gap + thickness * 0.5, sy + thickness * 0.5);
        path.close();
        ctx.fill_path(&path);
    };

    let draw_vertical = |ctx: &mut Context<'_>, sx: f64, sy: f64, sh: f64| {
        let mut path = Path::new();
        path.move_to(sx, sy + gap);
        path.line_to(sx + thickness * 0.5, sy + gap + thickness * 0.5);
        path.line_to(sx + thickness, sy + gap);
        path.line_to(sx + thickness, sy + sh - gap);
        path.line_to(sx + thickness * 0.5, sy + sh - gap - thickness * 0.5);
        path.line_to(sx, sy + sh - gap);
        path.close();
        ctx.fill_path(&path);
    };

    if seg[0] {
        draw_horizontal(ctx, x, y);
    }
    if seg[1] {
        draw_vertical(ctx, x + width - thickness, y, height * 0.5);
    }
    if seg[2] {
        draw_vertical(ctx, x + width - thickness, y + height * 0.5, height * 0.5);
    }
    if seg[3] {
        draw_horizontal(ctx, x, y + height - thickness);
    }
    if seg[4] {
        draw_vertical(ctx, x, y + height * 0.5, height * 0.5);
    }
    if seg[5] {
        draw_vertical(ctx, x, y, height * 0.5);
    }
    if seg[6] {
        draw_horizontal(ctx, x, y + height * 0.5 - thickness * 0.5);
    }
}

fn draw_colon(ctx: &mut Context<'_>, x: f64, y: f64, height: f64) {
    let dot_size = height * 0.1;
    ctx.fill_circle(&Circle::new(x + dot_size, y + height * 0.3, dot_size));
    ctx.fill_circle(&Circle::new(x + dot_size, y + height * 0.7, dot_size));
}

#[cfg(test)]
mod tests {
    use super::*;
    use raden::{Image, PipelineRuntime, PixelFormat};

    /// is_y4m が各バリアントに対して正しい判定結果を返すことを検証する
    #[test]
    fn test_is_y4m_all_variants() {
        // Y4m バリアント — テスト用 Y4M ファイルを生成して検証する
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("test.y4m");
        let w = 640;
        let h = 480;
        let chroma_w = (w + 1) / 2;
        let chroma_h = (h + 1) / 2;
        let frame_data_size = (w * h + 2 * chroma_w * chroma_h) as usize;
        let header = format!("YUV4MPEG2 W{w} H{h} F30:1 Ip C420\nFRAME\n");
        let mut data = header.into_bytes();
        data.resize(data.len() + frame_data_size, 0);
        std::fs::write(&path, &data).expect("テスト用 Y4M ファイルを書き込めること");

        let reader = Y4mReader::open(&path).expect("Y4M ファイルを開けること");
        let buf = vec![0u8; reader.frame_size()];
        let y4m = ImageHolder::Y4m(reader, buf);
        assert!(is_y4m(&y4m), "Y4m バリアントは true を返すこと");

        // Raden バリアント
        let img = Image::new(640, 480, PixelFormat::Prgb32);
        let runtime = Box::new(PipelineRuntime::new());
        let raden = ImageHolder::Raden(img, runtime);
        assert!(!is_y4m(&raden), "Raden バリアントは false を返すこと");

        // Sandstorm バリアント
        let ss = ImageHolder::Sandstorm(vec![0u32; (640 * 480) as usize]);
        assert!(!is_y4m(&ss), "Sandstorm バリアントは false を返すこと");
    }

    /// 奇数次元の解像度でも frame_size() と W*H*3/2 の結果が異なることを確認する
    /// (バッファ不足の防止のため frame_size() を使用していることの検証)
    #[test]
    fn test_odd_dimension_buffer_size() {
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("test_odd.y4m");
        let w = 641;
        let h = 481;
        let chroma_w = (w + 1) / 2;
        let chroma_h = (h + 1) / 2;
        let frame_data_size = (w * h + 2 * chroma_w * chroma_h) as usize;
        let header = format!("YUV4MPEG2 W{w} H{h} F30:1 Ip C420\nFRAME\n");
        let mut data = header.into_bytes();
        data.resize(data.len() + frame_data_size, 0);
        std::fs::write(&path, &data).expect("テスト用 Y4M ファイルを書き込めること");

        let reader = Y4mReader::open(&path).expect("Y4M ファイルを開けること");
        let frame_size = reader.frame_size();
        let legacy_size = w as usize * h as usize * 3 / 2;

        // frame_size() は ceil(width/2) * ceil(height/2) * 2 + width * height
        // legacy_size は width * height * 3 / 2 (整数除算)
        // 奇数次元では frame_size() ≥ legacy_size + 2 となる
        assert!(
            frame_size > legacy_size,
            "奇数次元では frame_size() が W*H*3/2 の整数除算より大きいこと (frame_size={frame_size}, legacy={legacy_size})"
        );
    }
}
