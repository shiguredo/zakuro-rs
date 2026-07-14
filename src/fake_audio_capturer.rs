use std::f64::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use shiguredo_webrtc::{
    AudioDeviceModule, AudioDeviceModuleHandler, AudioTransportRef, rtc_log_warning,
};

use crate::wav_reader::WavReader;

/// サンプルレート (Hz)。C++ Safari 音源と同じく 48kHz。
const SAMPLE_RATE: u32 = 48000;
/// チャンネル数 (モノラル)
const CHANNELS: usize = 1;

/// BIP / BOP パルス長 (秒)。C++ `BIPBOP_DURATION`。
const BIPBOP_DURATION: f64 = 0.07;
/// BIP / BOP の振幅係数。C++ `BIPBOP_VOLUME`。
const BIPBOP_VOLUME: f32 = 0.5;
/// BIP 周波数 (Hz)。先頭パルス。
const BIP_FREQUENCY: f32 = 1500.0;
/// BOP 周波数 (Hz)。1 秒地点のパルス。
const BOP_FREQUENCY: f32 = 500.0;
/// 全長に加算する HUM 周波数 (Hz)。
const HUM_FREQUENCY: f32 = 150.0;
/// HUM 振幅係数。
const HUM_VOLUME: f32 = 0.1;
/// 全長に加算するノイズ周波数 (Hz)。
const NOISE_FREQUENCY: f32 = 3000.0;
/// ノイズ振幅係数。
const NOISE_VOLUME: f32 = 0.05;

/// C++ Safari 相当の BIP / BOP パルス長 (サンプル数)
///
/// C++ は `(int)std::ceil(BIPBOP_DURATION * SAMPLE_RATE)`。
/// `0.07 * 48000` は double で正確な 3360 にならず、ceil 後は 3361 になる。
fn bipbop_sample_count() -> usize {
    (BIPBOP_DURATION * f64::from(SAMPLE_RATE)).ceil() as usize
}

/// 正弦波寄与を `dest` に加算する
///
/// C++ `add_hum` 相当。`volume` / `frequency` / `sample_rate` は `f32`、
/// `sin` は `f64`。寄与ごとに `i16` へ切り捨ててから加算する
/// (`saturating_add` は使わない。理論上界は i16 内に収まる)。
///
/// 位相インデックスはスライス先頭からの相対位置 (C++ の `start` は常に 0)。
fn add_hum(volume: f32, frequency: f32, sample_rate: f32, dest: &mut [i16]) {
    let hum_period = sample_rate / frequency;
    for (i, sample) in dest.iter_mut().enumerate() {
        let a = (f64::from(volume) * (i as f64 * 2.0 * PI / f64::from(hum_period)).sin() * 32767.0)
            as i16;
        *sample = (*sample as i32 + i32::from(a)) as i16;
    }
}

/// C++ `Type::Safari` 相当の 2 秒 PCM を組み立てる
///
/// 順序: BIP → BOP (`data[SAMPLE_RATE..]`) → NOISE 全長 → HUM 全長。
fn build_safari_audio() -> Vec<i16> {
    let sample_rate = SAMPLE_RATE as usize;
    let bipbop = bipbop_sample_count();
    let mut data = vec![0i16; sample_rate * 2];
    let sr = SAMPLE_RATE as f32;

    add_hum(BIPBOP_VOLUME, BIP_FREQUENCY, sr, &mut data[..bipbop]);
    add_hum(
        BIPBOP_VOLUME,
        BOP_FREQUENCY,
        sr,
        &mut data[sample_rate..sample_rate + bipbop],
    );
    add_hum(NOISE_VOLUME, NOISE_FREQUENCY, sr, &mut data);
    add_hum(HUM_VOLUME, HUM_FREQUENCY, sr, &mut data);

    data
}

/// 手続き生成した連続 PCM をループ再生する音源
///
/// instance (capturer) あたり 1。起動時に `build_safari_audio` で組み立て、
/// 10ms 単位でカーソルを進めながら読み出す。
pub(crate) struct GeneratedAudio {
    samples: Vec<i16>,
    cursor: usize,
}

impl GeneratedAudio {
    /// Safari 相当の 2 秒 PCM で初期化する
    pub(crate) fn new() -> Self {
        Self {
            samples: build_safari_audio(),
            cursor: 0,
        }
    }

    /// `buf` をループ再生のサンプルで埋める
    ///
    /// 呼び出し側は通常 10ms 分 (480 サンプル) を渡す想定。`WavReader` と同型。
    pub(crate) fn read_samples(&mut self, buf: &mut [i16]) {
        let len = self.samples.len();
        for slot in buf.iter_mut() {
            *slot = self.samples[self.cursor];
            self.cursor += 1;
            if self.cursor >= len {
                self.cursor = 0;
            }
        }
    }
}

/// フェイク音声の供給ソース
///
/// `Generated` は C++ Safari 相当の BIP / BOP / HUM / ノイズ 2 秒ループ。
/// `Wav` は WAV ファイルを 48kHz モノラルにリサンプリング済みのサンプル列として
/// ループ再生する。
pub(crate) enum FakeAudioSource {
    Generated(GeneratedAudio),
    Wav(WavReader),
}

/// フェイク音声キャプチャの内部状態 (スレッド間で共有する制御フラグのみ)
#[derive(Clone)]
struct FakeAudioState {
    recording: Arc<AtomicBool>,
    audio_transport: Arc<std::sync::Mutex<Option<AudioTransportRef>>>,
    stop: Arc<AtomicBool>,
}

/// フェイク音声キャプチャ
///
/// カスタム AudioDeviceModule を使って 10ms ごとに PCM データを WebRTC に送信する。
/// 音声ソースは `FakeAudioSource` で指定する。
///
/// - `FakeAudioSource::Generated`: Safari 相当の 2 秒ループを常時送出する。
/// - `FakeAudioSource::Wav`: WAV ファイルから読み込んだサンプル列をループ再生する。
pub(crate) struct FakeAudioCapturer {
    adm: AudioDeviceModule,
    state: FakeAudioState,
    /// `start()` で音声スレッドに move する。
    source: Option<FakeAudioSource>,
    handle: Option<thread::JoinHandle<()>>,
}

struct FakeAudioHandler {
    recording: Arc<AtomicBool>,
    audio_transport: Arc<std::sync::Mutex<Option<AudioTransportRef>>>,
}

impl AudioDeviceModuleHandler for FakeAudioHandler {
    fn register_audio_callback(&self, transport: Option<AudioTransportRef>) -> i32 {
        let Ok(mut stored) = self.audio_transport.lock() else {
            rtc_log_warning!("audio_transport mutex poisoned in register_audio_callback");
            return -1;
        };
        *stored = transport;
        0
    }

    fn init(&self) -> i32 {
        0
    }

    fn terminate(&self) -> i32 {
        0
    }

    fn initialized(&self) -> bool {
        true
    }

    fn recording_devices(&self) -> i16 {
        1
    }

    fn recording_device_name(&self, index: u16) -> Option<(String, String)> {
        if index == 0 {
            Some(("Fake Recording".to_string(), "fake-recording".to_string()))
        } else {
            None
        }
    }

    fn recording_is_available(&self, available: &mut bool) -> i32 {
        *available = true;
        0
    }

    fn init_recording(&self) -> i32 {
        0
    }

    fn recording_is_initialized(&self) -> bool {
        true
    }

    fn start_recording(&self) -> i32 {
        self.recording.store(true, Ordering::SeqCst);
        0
    }

    fn stop_recording(&self) -> i32 {
        self.recording.store(false, Ordering::SeqCst);
        0
    }

    fn recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }
}

impl FakeAudioCapturer {
    pub(crate) fn new(source: FakeAudioSource) -> Self {
        let recording = Arc::new(AtomicBool::new(false));
        let audio_transport = Arc::new(std::sync::Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));

        let adm = AudioDeviceModule::new_with_handler(Box::new(FakeAudioHandler {
            recording: Arc::clone(&recording),
            audio_transport: Arc::clone(&audio_transport),
        }));

        let state = FakeAudioState {
            recording,
            audio_transport,
            stop,
        };

        Self {
            adm,
            state,
            source: Some(source),
            handle: None,
        }
    }

    pub(crate) fn audio_device_module(&self) -> AudioDeviceModule {
        self.adm.clone()
    }

    pub(crate) fn start(&mut self) {
        if self.handle.is_some() {
            return;
        }

        let state = self.state.clone();
        // source は所有権をスレッドに移す (Generated / Wav とも内部 cursor を所有し Clone 不可)
        let source = self
            .source
            .take()
            .expect("FakeAudioCapturer::start called twice");
        let handle = thread::Builder::new()
            .name("fake-audio-capturer".to_string())
            .spawn(move || {
                audio_thread(state, source);
            })
            .expect("failed to spawn fake audio thread");

        self.handle = Some(handle);
    }
}

impl Drop for FakeAudioCapturer {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 10ms ごとに PCM データを生成して WebRTC に送信するスレッド
fn audio_thread(state: FakeAudioState, mut source: FakeAudioSource) {
    // 10ms 分のサンプル数
    let samples_per_10ms = (SAMPLE_RATE / 100) as usize;
    let mut buffer = vec![0i16; samples_per_10ms * CHANNELS];

    let interval = std::time::Duration::from_millis(10);
    let mut next_time = std::time::Instant::now();

    while !state.stop.load(Ordering::Acquire) {
        match &mut source {
            FakeAudioSource::Generated(generated) => {
                generated.read_samples(&mut buffer);
            }
            FakeAudioSource::Wav(reader) => {
                // WAV からサンプルを取り出してループ再生する
                reader.read_samples(&mut buffer);
            }
        }

        // WebRTC に送信
        if state.recording.load(Ordering::SeqCst) {
            let transport = {
                let Ok(guard) = state.audio_transport.lock() else {
                    rtc_log_warning!("audio_transport mutex poisoned in audio_thread");
                    continue;
                };
                *guard
            };
            if let Some(transport) = transport {
                let mut new_mic_level = 0;
                let _ = unsafe {
                    transport.recorded_data_is_available(
                        buffer.as_ptr() as *const u8,
                        samples_per_10ms,
                        2 * CHANNELS, // bytes per sample
                        CHANNELS,
                        SAMPLE_RATE,
                        0,
                        0,
                        0,
                        false,
                        &mut new_mic_level,
                        None,
                    )
                };
            }
        }

        // 10ms 間隔を維持
        next_time += interval;
        let now = std::time::Instant::now();
        if next_time > now {
            thread::sleep(next_time - now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// poison 済み Mutex に対して register_audio_callback が -1 を返すことを検証する
    #[test]
    fn test_register_audio_callback_poisoned_mutex() {
        let audio_transport = Arc::new(std::sync::Mutex::new(None::<AudioTransportRef>));
        // Mutex を poison させる
        let poisoned = audio_transport.clone();
        let handle = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("意図的に mutex を poison する");
        });
        let _ = handle.join();

        let handler = FakeAudioHandler {
            recording: Arc::new(AtomicBool::new(false)),
            audio_transport,
        };
        let result = handler.register_audio_callback(None);
        assert_eq!(
            result, -1,
            "poison 済み Mutex では register_audio_callback が -1 を返すこと"
        );
    }

    /// 正常系: register_audio_callback が 0 を返し transport が設定されることを検証する
    #[test]
    fn test_register_audio_callback_normal() {
        let audio_transport = Arc::new(std::sync::Mutex::new(None::<AudioTransportRef>));
        let handler = FakeAudioHandler {
            recording: Arc::new(AtomicBool::new(false)),
            audio_transport,
        };
        let result = handler.register_audio_callback(None);
        assert_eq!(
            result, 0,
            "正常系では register_audio_callback が 0 を返すこと"
        );
    }

    /// Safari バッファ長と BIP/BOP パルス長が C++ と同じになることを検証する
    #[test]
    fn safari_buffer_length_and_bipbop_count() {
        let samples = build_safari_audio();
        assert_eq!(samples.len(), 48000 * 2, "バッファ長は 48kHz × 2 秒のはず");
        // C++ `(int)std::ceil(0.07 * 48000)` は floating 誤差で 3361
        assert_eq!(
            bipbop_sample_count(),
            3361,
            "BIP/BOP パルス長は C++ ceil 結果と一致するはず"
        );
    }

    /// 代表点の金値が BIP/BOP/NOISE/HUM の加算規則と一致することを検証する
    ///
    /// 期待値は C++ と同じ位相規則で算出した固定定数。
    /// `samples[0]` / `samples[SAMPLE_RATE]` は sin(0)=0 のため使わない。
    /// `samples[bipbop]` も NOISE/HUM が零点になりうるため使わない。
    #[test]
    fn safari_golden_samples_at_representative_indices() {
        let samples = build_safari_audio();
        let sample_rate = SAMPLE_RATE as usize;
        let bipbop = bipbop_sample_count();

        // samples[1] = BIP(i=1) + NOISE(i=1) + HUM(i=1)
        assert_eq!(samples[1], 3886, "index 1 の金値が一致するはず");
        // samples[SAMPLE_RATE+1] = BOP(k=1) + NOISE(i) + HUM(i)
        assert_eq!(
            samples[sample_rate + 1],
            1761,
            "index SAMPLE_RATE+1 の金値が一致するはず"
        );
        // BIP/BOP パルス外 = NOISE + HUM のみ
        assert_eq!(
            samples[bipbop + 1],
            1030,
            "index bipbop+1 の金値が一致するはず"
        );
    }

    /// 末尾付近からの読み出しが先頭へ折り返し、要求長どおり埋まることを検証する
    #[test]
    fn generated_read_samples_loops_at_end() {
        let mut generated = GeneratedAudio::new();
        let len = generated.samples.len();
        // カーソルを末尾 3 サンプル手前に置く
        generated.cursor = len - 3;
        let expected = [
            generated.samples[len - 3],
            generated.samples[len - 2],
            generated.samples[len - 1],
            generated.samples[0],
            generated.samples[1],
            generated.samples[2],
            generated.samples[3],
        ];

        let mut out = [0i16; 7];
        generated.read_samples(&mut out);
        assert_eq!(
            out, expected,
            "末尾到達後は先頭から繰り返し、出力長が要求どおりのはず"
        );
        assert_eq!(generated.cursor, 4, "カーソルは折り返し後の位置のはず");
    }
}
