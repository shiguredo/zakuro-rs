use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::{ErrorMessage, Result};

/// FakeAudioCapturer が要求するサンプルレート (Hz)
const TARGET_SAMPLE_RATE: u32 = 48000;

/// WAV (RIFF/WAVE PCM 16bit) ファイルリーダー
///
/// 開いた時点で 48kHz モノラル i16 にリサンプリング・ダウンミックス済みのサンプル列を
/// すべてメモリに保持する。`read_samples()` は要求されたサンプル数を返し、
/// ファイル終端に達した場合は先頭からループ再生する。
///
/// 対応フォーマット:
/// - audio format: PCM (1)
/// - bits per sample: 16
/// - channels: 1 (モノラル) または 2 (ステレオ、L+R を平均してモノ化)
/// - sample rate: 任意 (内部で 48kHz に線形補間リサンプル)
pub(crate) struct WavReader {
    /// 48kHz モノラル i16 にリサンプル / ダウンミックス済みのサンプル列
    samples: Vec<i16>,
    /// 次に返すサンプル位置 (samples.len() を超えたら 0 に戻る = ループ再生)
    cursor: usize,
}

impl WavReader {
    /// WAV ファイルを開いて 48kHz モノラル i16 に変換する
    pub(crate) fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let mut file =
            File::open(path).map_err(|e| ErrorMessage::new(format!("WAV file open error: {e}")))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| ErrorMessage::new(format!("WAV file read error: {e}")))?;
        Self::from_bytes(&buf)
    }

    /// バイト列から WAV をパースする (テスト用にも公開)
    pub(crate) fn from_bytes(buf: &[u8]) -> Result<Self> {
        let parsed = parse_wav(buf)?;
        let mono = downmix_to_mono(&parsed.samples, parsed.channels);
        let resampled = resample(&mono, parsed.sample_rate, TARGET_SAMPLE_RATE);
        if resampled.is_empty() {
            return Err(ErrorMessage::new("WAV file contains no samples").into());
        }
        Ok(Self {
            samples: resampled,
            cursor: 0,
        })
    }

    /// `buf` をループ再生のサンプルで埋める
    ///
    /// 呼び出し側は通常 10ms 分 (480 サンプル) を渡す想定。
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

/// パース結果 (内部使用)
struct ParsedWav {
    sample_rate: u32,
    channels: u16,
    samples: Vec<i16>,
}

/// RIFF/WAVE PCM 16bit のヘッダとデータをパースする
///
/// `fmt ` と `data` 以外のチャンク (例: `LIST`, `JUNK`) はスキップする。
/// チャンクサイズが奇数の場合はパディング 1 バイトを読み飛ばす (RIFF 仕様)。
fn parse_wav(buf: &[u8]) -> Result<ParsedWav> {
    if buf.len() < 12 {
        return Err(ErrorMessage::new("WAV file too short").into());
    }
    if &buf[0..4] != b"RIFF" {
        return Err(ErrorMessage::new("WAV file missing RIFF header").into());
    }
    if &buf[8..12] != b"WAVE" {
        return Err(ErrorMessage::new("WAV file missing WAVE marker").into());
    }

    let mut pos = 12;
    let mut audio_format: u16 = 0;
    let mut channels: u16 = 0;
    let mut sample_rate: u32 = 0;
    let mut bits_per_sample: u16 = 0;
    let mut data_range: Option<(usize, usize)> = None;

    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let size = u32::from_le_bytes(
            buf[pos + 4..pos + 8]
                .try_into()
                .expect("chunk size: 4-byte slice"),
        ) as usize;
        let body_start = pos + 8;
        let body_end = body_start
            .checked_add(size)
            .ok_or_else(|| ErrorMessage::new("WAV chunk size overflow"))?;
        if body_end > buf.len() {
            return Err(ErrorMessage::new("WAV chunk size exceeds file").into());
        }
        match id {
            b"fmt " => {
                if size < 16 {
                    return Err(ErrorMessage::new("WAV fmt chunk too short").into());
                }
                audio_format = u16::from_le_bytes(
                    buf[body_start..body_start + 2]
                        .try_into()
                        .expect("audio format: 2-byte slice"),
                );
                channels = u16::from_le_bytes(
                    buf[body_start + 2..body_start + 4]
                        .try_into()
                        .expect("channels: 2-byte slice"),
                );
                sample_rate = u32::from_le_bytes(
                    buf[body_start + 4..body_start + 8]
                        .try_into()
                        .expect("sample rate: 4-byte slice"),
                );
                bits_per_sample = u16::from_le_bytes(
                    buf[body_start + 14..body_start + 16]
                        .try_into()
                        .expect("bits per sample: 2-byte slice"),
                );
            }
            b"data" => {
                data_range = Some((body_start, body_end));
            }
            _ => {}
        }
        // RIFF: チャンクサイズが奇数なら 1 バイトのパディング
        pos = body_end + (size & 1);
    }

    if audio_format != 1 {
        return Err(ErrorMessage::new(format!(
            "WAV unsupported audio format: {audio_format} (PCM only)"
        ))
        .into());
    }
    if bits_per_sample != 16 {
        return Err(ErrorMessage::new(format!(
            "WAV unsupported bits per sample: {bits_per_sample} (16bit only)"
        ))
        .into());
    }
    if channels != 1 && channels != 2 {
        return Err(ErrorMessage::new(format!(
            "WAV unsupported channels: {channels} (mono or stereo only)"
        ))
        .into());
    }
    if sample_rate == 0 {
        return Err(ErrorMessage::new("WAV invalid sample rate").into());
    }

    let (data_start, data_end) =
        data_range.ok_or_else(|| ErrorMessage::new("WAV missing data chunk"))?;
    let data = &buf[data_start..data_end];

    // データを i16 サンプルにデコード (リトルエンディアン)
    // 末尾の余り 1 バイトは破棄 (壊れた WAV 防御)
    let mut samples: Vec<i16> = Vec::new();
    for chunk in data.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    Ok(ParsedWav {
        sample_rate,
        channels,
        samples,
    })
}

/// 多チャンネルを平均化してモノラル化する
///
/// ステレオの場合、サンプル列が L0, R0, L1, R1, ... の順に並んでいる前提。
fn downmix_to_mono(samples: &[i16], channels: u16) -> Vec<i16> {
    if channels == 1 {
        return samples.to_vec();
    }
    let ch = channels as usize;
    let mut out = Vec::new();
    for frame in samples.chunks_exact(ch) {
        let sum: i32 = frame.iter().map(|&s| s as i32).sum();
        out.push((sum / ch as i32) as i16);
    }
    out
}

/// 線形補間で `in_rate` Hz から `out_rate` Hz にリサンプリングする
///
/// 性能より堅牢性優先で単純な線形補間を採用。負荷試験用の音声品質としては十分。
fn resample(input: &[i16], in_rate: u32, out_rate: u32) -> Vec<i16> {
    if input.is_empty() || in_rate == out_rate {
        return input.to_vec();
    }
    let ratio = in_rate as f64 / out_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut output: Vec<i16> = Vec::new();
    let last = input.len() - 1;
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        if idx >= last {
            output.push(input[last]);
        } else {
            let frac = pos - idx as f64;
            let a = input[idx] as f64;
            let b = input[idx + 1] as f64;
            output.push((a + (b - a) * frac) as i16);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用に PCM 16bit WAV バイト列を組み立てるヘルパー
    fn build_wav(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
        let block_align: u16 = channels * bits_per_sample / 8;
        let data_bytes: u32 = (samples.len() * 2) as u32;
        let fmt_size: u32 = 16;
        let riff_size: u32 = 4 + (8 + fmt_size) + (8 + data_bytes);

        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&riff_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&fmt_size.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits_per_sample.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_bytes.to_le_bytes());
        for &s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    #[test]
    fn from_bytes_accepts_48khz_mono() {
        // 48kHz モノラルはリサンプリング不要でそのまま使えるはず
        let samples: Vec<i16> = (0..480).map(|i| (i as i16) * 10).collect();
        let buf = build_wav(48000, 1, &samples);
        let reader = WavReader::from_bytes(&buf).expect("48kHz mono は受理されるはず");
        assert_eq!(reader.samples.len(), 480, "サンプル数はそのまま 480 のはず");
        assert_eq!(reader.samples[0], 0, "先頭サンプルは入力と一致するはず");
        assert_eq!(
            reader.samples[479], 4790,
            "末尾サンプルは入力と一致するはず"
        );
    }

    #[test]
    fn from_bytes_downmixes_stereo_to_mono() {
        // ステレオ (L=1000, R=-1000) は平均化して 0 になるはず
        let samples: Vec<i16> = vec![1000, -1000, 2000, -2000, 3000, -3000];
        let buf = build_wav(48000, 2, &samples);
        let reader = WavReader::from_bytes(&buf).expect("ステレオは受理されるはず");
        assert_eq!(
            reader.samples,
            vec![0, 0, 0],
            "L+R を平均化したモノラルになるはず"
        );
    }

    #[test]
    fn from_bytes_resamples_44100_to_48000() {
        // 44100Hz の 4410 サンプル (= 0.1 秒) は 48000Hz の約 4800 サンプルになるはず
        let samples: Vec<i16> = (0..4410).map(|i| ((i % 100) as i16) * 100).collect();
        let buf = build_wav(44100, 1, &samples);
        let reader = WavReader::from_bytes(&buf).expect("44100Hz は受理されるはず");
        // out_len = floor(4410 / (44100/48000)) = floor(4800) = 4800
        assert_eq!(
            reader.samples.len(),
            4800,
            "48kHz にリサンプリングされてサンプル数が増えるはず"
        );
    }

    #[test]
    fn read_samples_loops_at_end() {
        // 3 サンプルしかない WAV を 7 サンプル要求するとループ再生される
        let samples: Vec<i16> = vec![100, 200, 300];
        let buf = build_wav(48000, 1, &samples);
        let mut reader = WavReader::from_bytes(&buf).expect("WAV は受理されるはず");

        let mut out = [0i16; 7];
        reader.read_samples(&mut out);
        assert_eq!(
            out,
            [100, 200, 300, 100, 200, 300, 100],
            "末尾到達後は先頭から繰り返すはず"
        );
    }

    #[test]
    fn from_bytes_rejects_non_pcm_format() {
        // audio_format=3 (IEEE float) は非対応エラー
        let samples: Vec<i16> = vec![0; 10];
        let mut buf = build_wav(48000, 1, &samples);
        // fmt チャンク内 audio format フィールドを 3 (float) に書き換える
        // ヘッダ構造: RIFF(4) + size(4) + WAVE(4) + "fmt "(4) + size(4) = 20 バイト目から audio_format
        buf[20] = 3;
        buf[21] = 0;
        let err = WavReader::from_bytes(&buf)
            .err()
            .expect("非 PCM フォーマットはエラーになるはず");
        assert!(
            format!("{err}").contains("audio format"),
            "エラーメッセージに audio format が含まれるはず: {err}"
        );
    }

    #[test]
    fn from_bytes_rejects_24bit() {
        // bits_per_sample=24 は非対応エラー
        let samples: Vec<i16> = vec![0; 10];
        let mut buf = build_wav(48000, 1, &samples);
        // bits_per_sample は fmt body の +14, +15 (= ヘッダ全体の +34, +35)
        buf[34] = 24;
        buf[35] = 0;
        let err = WavReader::from_bytes(&buf)
            .err()
            .expect("24bit はエラーになるはず");
        assert!(
            format!("{err}").contains("bits per sample"),
            "エラーメッセージに bits per sample が含まれるはず: {err}"
        );
    }

    #[test]
    fn from_bytes_skips_unknown_chunks() {
        // "fmt " の後ろに未知の "LIST" チャンクを挟んでも data まで到達できるはず
        let sample_rate: u32 = 48000;
        let channels: u16 = 1;
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
        let block_align: u16 = channels * bits_per_sample / 8;
        let samples: Vec<i16> = vec![111, 222, 333];
        let data_bytes: u32 = (samples.len() * 2) as u32;
        let fmt_size: u32 = 16;
        let list_size: u32 = 4;
        let riff_size: u32 = 4 + (8 + fmt_size) + (8 + list_size) + (8 + data_bytes);

        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&riff_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&fmt_size.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits_per_sample.to_le_bytes());
        buf.extend_from_slice(b"LIST");
        buf.extend_from_slice(&list_size.to_le_bytes());
        buf.extend_from_slice(&[0u8; 4]);
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_bytes.to_le_bytes());
        for &s in &samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }

        let reader = WavReader::from_bytes(&buf).expect("未知チャンクをスキップして読み込めるはず");
        assert_eq!(
            reader.samples,
            vec![111, 222, 333],
            "data チャンクのサンプルが正しく取得できるはず"
        );
    }
}
