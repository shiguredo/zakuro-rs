use std::f64::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use shiguredo_webrtc::{AudioDeviceModule, AudioDeviceModuleHandler, AudioTransportRef};

/// ビープ音の周波数 (Hz)
const BEEP_FREQUENCY: f64 = 1000.0;
/// ビープ音の長さ (ミリ秒)
const BEEP_DURATION_MS: u32 = 100;
/// ビープ音の振幅 (最大 32767 の約半分)
const BEEP_AMPLITUDE: f64 = 16000.0;
/// サンプルレート (Hz)
const SAMPLE_RATE: u32 = 48000;
/// チャンネル数
const CHANNELS: usize = 1;

/// フェイク音声のビープトリガー
///
/// 映像スレッドからパイチャート一周時に `trigger()` を呼び出す。
/// 音声スレッドが `take()` でトリガーを消費してビープ音を生成する。
#[derive(Clone)]
pub(crate) struct BeepTrigger {
    flag: Arc<AtomicBool>,
}

impl BeepTrigger {
    pub(crate) fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// ビープ音をトリガーする (映像スレッドから呼ぶ)
    pub(crate) fn trigger(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// トリガーを消費する (音声スレッドから呼ぶ)
    fn take(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }
}

/// フェイク音声キャプチャの内部状態
#[derive(Clone)]
pub(crate) struct FakeAudioState {
    recording: Arc<AtomicBool>,
    audio_transport: Arc<std::sync::Mutex<Option<AudioTransportRef>>>,
    beep_trigger: BeepTrigger,
    stop: Arc<AtomicBool>,
}

/// フェイク音声キャプチャ
///
/// カスタム AudioDeviceModule を使って 10ms ごとに PCM データを WebRTC に送信する。
/// 通常は無音を送信し、`BeepTrigger::trigger()` が呼ばれると
/// 1000Hz のビープ音を 100ms 間生成する。
pub(crate) struct FakeAudioCapturer {
    adm: AudioDeviceModule,
    state: FakeAudioState,
    handle: Option<thread::JoinHandle<()>>,
}

struct FakeAudioHandler {
    recording: Arc<AtomicBool>,
    audio_transport: Arc<std::sync::Mutex<Option<AudioTransportRef>>>,
}

impl AudioDeviceModuleHandler for FakeAudioHandler {
    fn register_audio_callback(&self, transport: Option<AudioTransportRef>) -> i32 {
        let mut stored = self.audio_transport.lock().unwrap();
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
    pub(crate) fn new(beep_trigger: BeepTrigger) -> Self {
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
            beep_trigger,
            stop,
        };

        Self {
            adm,
            state,
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
        let handle = thread::Builder::new()
            .name("fake-audio-capturer".to_string())
            .spawn(move || {
                audio_thread(state);
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
fn audio_thread(state: FakeAudioState) {
    // 10ms 分のサンプル数
    let samples_per_10ms = (SAMPLE_RATE / 100) as usize;
    let mut buffer = vec![0i16; samples_per_10ms * CHANNELS];

    let mut beep_samples_remaining: i32 = 0;
    let mut beep_phase: f64 = 0.0;
    let phase_increment = 2.0 * PI * BEEP_FREQUENCY / SAMPLE_RATE as f64;

    let interval = std::time::Duration::from_millis(10);
    let mut next_time = std::time::Instant::now();

    while !state.stop.load(Ordering::Acquire) {
        // ビープトリガーをチェック
        if state.beep_trigger.take() {
            beep_samples_remaining = (BEEP_DURATION_MS * SAMPLE_RATE / 1000) as i32;
            beep_phase = 0.0;
        }

        // ビープ音またはサイレンスを生成
        if beep_samples_remaining > 0 {
            for sample in buffer.iter_mut() {
                *sample = (BEEP_AMPLITUDE * beep_phase.sin()) as i16;
                beep_phase += phase_increment;
                if beep_phase >= 2.0 * PI {
                    beep_phase -= 2.0 * PI;
                }
            }
            beep_samples_remaining -= samples_per_10ms as i32;
            if beep_samples_remaining < 0 {
                beep_samples_remaining = 0;
            }
        } else {
            buffer.fill(0);
        }

        // WebRTC に送信
        if state.recording.load(Ordering::SeqCst) {
            let transport = {
                let stored = state.audio_transport.lock().unwrap();
                *stored
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
