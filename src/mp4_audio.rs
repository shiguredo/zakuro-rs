//! MP4 ファイル内の音声トラックをデコードして PCM を供給するモジュール
//!
//! `--input-mp4` の映像パススルーに加えて、MP4 内の音声トラック (Opus / AAC) を
//! PCM (48kHz モノラル) にデコードし、`FakeAudioCapturer` 経由で WebRTC の
//! builtin Opus エンコーダーに渡す。AAC は feature `fdk-aac` (Linux 限定) が必要。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use shiguredo_mp4::boxes::SampleEntry;
use shiguredo_mp4::demux::{Input, Mp4FileDemuxer};
use shiguredo_webrtc::rtc_log_warning;

use crate::error::{ErrorMessage, Result};
use crate::wav_reader::downmix_to_mono;
// resample は AAC (Linux + feature `fdk-aac`) のデコード経路でのみ使う
#[cfg(all(target_os = "linux", feature = "fdk-aac"))]
use crate::wav_reader::resample;

/// FakeAudioCapturer が要求するサンプルレート (Hz)。
///
/// Opus のデコード出力は常に 48kHz。AAC は 48kHz 以外の場合はこの値へ
/// リサンプリングする。
const SAMPLE_RATE: u32 = 48000;

/// 音声サンプル 1 個の最大サイズ (バイト)
///
/// Opus / AAC の 1 パケットは実用上 16KB 以下に収まる。破損した stsz が
/// ファイルサイズ近い値を指す場合に巨大なバッファを確保するのを防ぐ上限。
const MAX_AUDIO_SAMPLE_SIZE: usize = 1024 * 1024;

/// MP4 音声トラックの調査結果
#[derive(Debug)]
pub(crate) enum Mp4AudioTrackResult {
    /// 音声トラックが存在しない (映像のみで続行する)
    NoAudioTrack,
    /// 音声トラックはあるが対応できない構成 (警告を出力済み、映像のみで続行する)
    Unsupported,
    /// デコードして送信できる音声トラック
    ///
    /// `Mp4AudioSource` はデコーダーとサンプルテーブルを保持して大きく
    /// なるため Box で間接参照する (enum 全体のサイズを節約する)
    Supported(Box<Mp4AudioSource>),
}

/// 対応できない音声トラックの結果を組み立てる (警告ログの出力も行う)
fn unsupported(reason: impl Into<String>) -> Mp4AudioTrackResult {
    let reason = reason.into();
    rtc_log_warning!("MP4 audio track is unsupported: {reason}");
    Mp4AudioTrackResult::Unsupported
}

/// 音声サンプルのメタデータ
#[derive(Debug)]
struct AudioSampleMeta {
    /// サンプルデータのファイル内オフセット
    data_offset: u64,
    /// サンプルデータのサイズ
    data_size: usize,
}

/// 対応コーデックの判別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioCodec {
    /// Opus (dOps ボックス)
    Opus,
    /// AAC (mp4a ボックス + esds)
    Aac,
}

/// 音声トラックのコーデック情報
#[derive(PartialEq, Eq)]
struct AudioTrackInfo {
    /// コーデック種別
    codec: AudioCodec,
    /// チャンネル数 (1 = モノラル、2 = ステレオ)
    channels: u8,
    /// Opus の dOps の PreSkip (48kHz 基準・ループ先頭で破棄するサンプル数)
    opus_pre_skip: Option<u16>,
    /// Opus の dOps の OutputGain (Q7.8 固定小数点の dB)
    opus_output_gain: Option<i16>,
    /// AAC の AudioSpecificConfig (esds の DecoderSpecificInfo)
    aac_asc: Option<Vec<u8>>,
}

impl AudioTrackInfo {
    /// SampleEntry からコーデック情報を抽出する
    ///
    /// 対応コーデック (Opus / AAC) 以外のエントリーは None を返す
    /// (FLAC や未知のボックスは未対応コーデックとして扱う)。
    fn from_sample_entry(entry: &SampleEntry) -> Option<Self> {
        match entry {
            // channel mapping family 0 の Opus はモノラルまたはステレオに限られる
            // (family 0 以外は shiguredo_mp4 のデコードが拒否する)
            SampleEntry::Opus(opus) => Some(Self {
                codec: AudioCodec::Opus,
                channels: opus.dops_box.output_channel_count,
                opus_pre_skip: Some(opus.dops_box.pre_skip),
                opus_output_gain: Some(opus.dops_box.output_gain),
                aac_asc: None,
            }),
            SampleEntry::Mp4a(mp4a) => Some(Self {
                codec: AudioCodec::Aac,
                channels: u8::try_from(mp4a.audio.channelcount).ok()?,
                opus_pre_skip: None,
                opus_output_gain: None,
                aac_asc: mp4a
                    .esds_box
                    .es
                    .dec_config_descr
                    .dec_specific_info
                    .as_ref()
                    .map(|info| info.payload.clone()),
            }),
            _ => None,
        }
    }
}

/// デコード方式とデコーダー
#[derive(Debug)]
enum AudioDecoderKind {
    /// Opus。出力は常に 48kHz。ループ先頭では dOps の PreSkip 分を破棄する
    /// (RFC 7845 の OpusHead PreSkip と同じ扱い)
    Opus {
        decoder: shiguredo_opus::Decoder,
        /// dOps の PreSkip (48kHz 基準のサンプル数)
        pre_skip: usize,
        /// 現在のループで破棄待ちのサンプル数
        skip_remaining: usize,
        /// デコード出力のチャンネル数 (1 または 2)
        channels: u8,
    },
    /// AAC (FDK AAC、Linux + feature `fdk-aac`)。出力サンプルレートは 48kHz とは限らないため
    /// 必要に応じてリサンプリングする。
    #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
    Aac {
        decoder: shiguredo_fdk_aac::Decoder,
        /// ループごとのデコーダー再生成に使う libfdk-aac (Decoder と共有する clone)
        lib: shiguredo_fdk_aac::FdkAacLibrary,
        /// ループごとのデコーダー再生成に使う AudioSpecificConfig
        asc: Vec<u8>,
    },
}

/// MP4 の音声トラックをデコードして PCM を供給するソース
///
/// `FakeAudioSource` の 1 バリアントとして音声スレッドから使われる。
/// MP4 から音声サンプルを順次読み出してデコードし、有界の PCM FIFO に
/// 蓄積する。ファイル全体の PCM はメモリに保持しない。
/// 音声トラックの終端に達したら先頭からループ再生する (映像のループとは独立)。
#[derive(Debug)]
pub(crate) struct Mp4AudioSource {
    /// MP4 ファイルへの読み込みストリーム
    file: BufReader<File>,
    /// 音声サンプルのメタデータ (トラック内の全サンプル)
    samples: Vec<AudioSampleMeta>,
    /// 次に読み出すサンプル位置 (終端に達したら 0 に戻る = ループ再生)
    cursor: usize,
    /// デコード済み PCM (48kHz モノラル) の有界 FIFO
    fifo: VecDeque<i16>,
    /// デコーダー
    decoder: AudioDecoderKind,
}

impl Mp4AudioSource {
    /// `buf` を MP4 音声のサンプルで埋める
    ///
    /// 呼び出し側は通常 10ms 分 (480 サンプル) を渡す想定。FIFO の残量が
    /// 不足していれば次の音声サンプルを読み出してデコードを追加する。
    /// デコードが供給に追いつかず FIFO が枯渇した場合は無音を送出して継続する。
    pub(crate) fn read_samples(&mut self, buf: &mut [i16]) {
        let mut attempts = 0;
        let mut emit_warning = true;
        while self.fifo.len() < buf.len() {
            let before = self.fifo.len();
            self.decode_next_packet(emit_warning);
            // パケットの失敗が続く場合に 10ms ごとに全パケット分の警告が
            // 出続けないよう、最初の失敗だけで警告する
            if self.fifo.len() == before {
                emit_warning = false;
            }
            attempts += 1;
            if attempts > self.samples.len() {
                // 全パケットを巡回しても足りない場合は無音で埋めて継続する
                // (デコード不能な入力で無限ループしないための上限)
                break;
            }
        }
        for slot in buf.iter_mut() {
            *slot = self.fifo.pop_front().unwrap_or(0);
        }
    }

    /// 次のパケットを読み出してデコードし、PCM を FIFO に追加する
    ///
    /// パケットの読み込み・デコードに失敗した場合は警告を出力して
    /// そのパケットをスキップする (音声を止めずに次のパケットへ進む)。
    /// `emit_warning` が false のときは警告を出さずにスキップのみ行う
    /// (破損入力で連続失敗するときに read_samples が最初の失敗だけ
    /// 警告を出すための仕組み)。
    fn decode_next_packet(&mut self, emit_warning: bool) {
        // トラック終端に達したら先頭からループ再生する
        if self.cursor >= self.samples.len() {
            self.cursor = 0;
            self.reset_decoder_for_loop();
        }
        let (data_offset, data_size) = {
            let sample = &self.samples[self.cursor];
            (sample.data_offset, sample.data_size)
        };
        self.cursor += 1;

        let data = match read_bytes_at(&mut self.file, data_offset, data_size) {
            Ok(data) => data,
            Err(err) => {
                if emit_warning {
                    rtc_log_warning!("MP4 audio: failed to read sample data: {err}");
                }
                return;
            }
        };

        match &mut self.decoder {
            AudioDecoderKind::Opus {
                decoder,
                skip_remaining,
                channels,
                ..
            } => {
                let pcm = match decoder.decode(&data) {
                    Ok(pcm) => pcm,
                    Err(err) => {
                        if emit_warning {
                            rtc_log_warning!("MP4 audio: Opus decode failed: {err}");
                        }
                        return;
                    }
                };
                // ループ先頭では PreSkip サンプル分を破棄する (RFC 7845)。
                // PreSkip はチャンネルあたりのサンプル数のため、インターリーブ列では
                // チャンネル数倍のサンプルを破棄する
                let channels = usize::from(*channels);
                let skip_frames = (*skip_remaining).min(pcm.len() / channels);
                *skip_remaining -= skip_frames;
                let skip_interleaved = skip_frames * channels;
                self.fifo
                    .extend(downmix_to_mono(&pcm[skip_interleaved..], channels as u16));
            }
            #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
            AudioDecoderKind::Aac { decoder, .. } => {
                if let Err(err) = decoder.decode(&data) {
                    if emit_warning {
                        rtc_log_warning!("MP4 audio: AAC decode failed: {err}");
                    }
                    return;
                }
                // 1 パケット = 1 フレームのため、入力直後に取り出せる
                let frame = match decoder.next_frame() {
                    Ok(Some(frame)) => frame,
                    Ok(None) => {
                        // 入力不足等でフレームが完結しない場合はスキップする
                        return;
                    }
                    Err(err) => {
                        if emit_warning {
                            rtc_log_warning!("MP4 audio: AAC frame decode failed: {err}");
                        }
                        return;
                    }
                };
                if frame.sample_rate == 0 {
                    if emit_warning {
                        rtc_log_warning!("MP4 audio: AAC decoder returned invalid sample rate 0");
                    }
                    return;
                }
                let mono = downmix_to_mono(&frame.data, u16::from(frame.channels));
                let pcm = if frame.sample_rate == SAMPLE_RATE {
                    mono
                } else {
                    // 48kHz 以外 (例: 44.1kHz) は 48kHz にリサンプリングする
                    resample(&mono, frame.sample_rate, SAMPLE_RATE)
                };
                self.fifo.extend(pcm);
            }
        }
    }

    /// ループ再生の先頭に戻るときにデコーダー状態をリセットする
    ///
    /// Opus は PreSkip がストリーム先頭でのみ有効なため、デコーダー内部状態を
    /// 初期化して PreSkip 分の破棄をやり直す。AAC も同様にデコーダーを作り直し
    /// (FDK の内部入力バッファに残ったデータが次のループへ持ち越されないようにする)、
    /// 各ループをストリーム先頭からの再生として扱う。
    fn reset_decoder_for_loop(&mut self) {
        match &mut self.decoder {
            AudioDecoderKind::Opus {
                decoder,
                pre_skip,
                skip_remaining,
                ..
            } => {
                if let Err(err) = decoder.reset() {
                    rtc_log_warning!("MP4 audio: Opus decoder reset failed: {err}");
                }
                *skip_remaining = *pre_skip;
            }
            #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
            AudioDecoderKind::Aac { decoder, lib, asc } => {
                match shiguredo_fdk_aac::Decoder::new(lib.clone(), asc) {
                    Ok(new_decoder) => *decoder = new_decoder,
                    Err(err) => {
                        rtc_log_warning!("MP4 audio: failed to recreate AAC decoder: {err}");
                    }
                }
            }
        }
    }
}

/// MP4 の音声トラックを調査し、デコード可能なら `Mp4AudioSource` を構築する
///
/// 音声トラックが 2 本以上ある場合はエラーを返す (任意の 1 本を選ばない)。
/// 音声トラックが無い場合と、未対応コーデック・対応外チャンネル構成の場合は
/// 警告を出力したうえで映像のみで続行できる結果を返す。
///
/// AAC 音声トラックは feature `fdk-aac` (Linux 限定) と `--fdk-aac-lib` で
/// 指定した libfdk-aac 共有ライブラリの動的ロードが必要。ロードできない環境で
/// AAC 音声を含む MP4 を指定した場合は起動時エラーを返す。feature 無し / 非 Linux
/// では未対応として映像のみで続行する。
pub(crate) fn inspect_mp4_audio(
    path: &Path,
    fdk_aac_lib_path: Option<&str>,
) -> Result<Mp4AudioTrackResult> {
    let mut file = BufReader::new(
        File::open(path)
            .map_err(|e| ErrorMessage::new(format!("MP4 ファイルのオープンエラー: {e}")))?,
    );
    let file_size = file
        .get_ref()
        .metadata()
        .map_err(|e| ErrorMessage::new(format!("MP4 ファイルのメタデータ取得エラー: {e}")))?
        .len();

    let mut demuxer = Mp4FileDemuxer::new();
    // デマルチプレクサが要求する範囲のデータを順次渡して解析を進める
    while let Some(required) = demuxer.required_input() {
        if required.position > file_size {
            return Err(ErrorMessage::new(format!(
                "MP4 デマルチプレクサがファイルサイズ範囲外の位置を要求しました: position={}, file_size={file_size}",
                required.position
            ))
            .into());
        }
        let remaining = file_size - required.position;
        let size = usize::try_from(
            required
                .size
                .map_or(remaining, |size| (size as u64).min(remaining)),
        )
        .map_err(|_| ErrorMessage::new("MP4 デマルチプレクサの要求サイズが大きすぎます"))?;
        let data = read_bytes_at(&mut file, required.position, size)
            .map_err(|e| ErrorMessage::new(format!("MP4 ファイルの読み込みエラー: {e}")))?;
        demuxer.handle_input(Input {
            position: required.position,
            data: &data,
        });
    }

    let tracks = demuxer
        .tracks()
        .map_err(|e| ErrorMessage::new(format!("MP4 ファイルのデマルチプレクスエラー: {e}")))?;

    // 音声トラックを列挙する (トラック ID だけを取り出して借用を終える)
    let audio_track_ids: Vec<u32> = tracks
        .iter()
        .filter(|t| t.kind == shiguredo_mp4::TrackKind::Audio)
        .map(|t| t.track_id)
        .collect();
    if audio_track_ids.is_empty() {
        return Ok(Mp4AudioTrackResult::NoAudioTrack);
    }
    if audio_track_ids.len() >= 2 {
        return Err(ErrorMessage::new(format!(
            "MP4 に音声トラックが {} 本あります (音声トラックは 1 本のみ対応)",
            audio_track_ids.len()
        ))
        .into());
    }
    let audio_track_id = audio_track_ids[0];

    // 音声サンプルを収集し、サンプルエントリーからコーデック情報を決定する
    let mut samples: Vec<AudioSampleMeta> = Vec::new();
    let mut first_info: Option<AudioTrackInfo> = None;
    while let Some(sample) = demuxer
        .next_sample()
        .map_err(|e| ErrorMessage::new(format!("MP4 ファイルのデマルチプレクスエラー: {e}")))?
    {
        // 音声トラック以外のサンプルはスキップする
        if sample.track.track_id != audio_track_id {
            continue;
        }
        // サンプルエントリーが付与されたサンプルでコーデック情報を確定・検証する。
        // 最初のサンプルは必ず Some になるため、未対応コーデックはここで判明する
        if let Some(entry) = sample.sample_entry {
            match AudioTrackInfo::from_sample_entry(entry) {
                Some(info) => {
                    if let Some(first) = &first_info {
                        // コーデック・チャンネル数・PreSkip・OutputGain・ASC が途中で
                        // 切り替わるサンプルエントリーは受理しない
                        if first != &info {
                            return Err(ErrorMessage::new(
                                "MP4 の音声サンプルエントリーが途中で切り替わっています",
                            )
                            .into());
                        }
                    } else {
                        first_info = Some(info);
                    }
                }
                None => {
                    // 最初のサンプルエントリーが未対応コーデックなら警告して映像のみで続行する
                    if first_info.is_none() {
                        return Ok(unsupported("unsupported audio codec"));
                    }
                    // 途中で未対応コーデックへ切り替わるエントリーは受理しない
                    return Err(ErrorMessage::new(
                        "MP4 の音声サンプルエントリーが途中で未対応コーデックに切り替わっています",
                    )
                    .into());
                }
            }
        }
        samples.push(AudioSampleMeta {
            data_offset: sample.data_offset,
            data_size: sample.data_size,
        });
    }

    // サンプルが 1 つも無い音声トラックは送信できない (映像のみで続行する)
    if samples.is_empty() {
        return Ok(unsupported("audio track has no samples"));
    }
    let track_info = first_info.ok_or_else(|| {
        // サンプルエントリーが付与されない音声トラックはデコードできない
        ErrorMessage::new("MP4 の音声トラックにサンプルエントリーがありません")
    })?;
    if !(1..=2).contains(&track_info.channels) {
        return Ok(unsupported(format!(
            "channel count {} (only mono or stereo is supported)",
            track_info.channels
        )));
    }

    // サンプルテーブルの位置・サイズがファイル範囲内かを検証する
    // (破損した stsz / stco から巨大なバッファ確保や範囲外読み込みを防ぐ)
    for (index, sample) in samples.iter().enumerate() {
        if sample.data_size > MAX_AUDIO_SAMPLE_SIZE {
            return Err(ErrorMessage::new(format!(
                "MP4 の音声サンプルが大きすぎます: sample={index} size={} (上限 {} バイト)",
                sample.data_size, MAX_AUDIO_SAMPLE_SIZE
            ))
            .into());
        }
        let data_size_u64 = sample.data_size as u64;
        if sample
            .data_offset
            .checked_add(data_size_u64)
            .is_none_or(|end| end > file_size)
        {
            return Err(ErrorMessage::new(format!(
                "MP4 の音声サンプルテーブルに不整合があります: sample={index} offset={} size={} file_size={file_size}",
                sample.data_offset, sample.data_size
            ))
            .into());
        }
    }

    // コーデックごとのデコーダーを構築する
    match track_info.codec {
        AudioCodec::Opus => build_opus_source(track_info, file, samples),
        AudioCodec::Aac => build_aac_source(track_info, fdk_aac_lib_path, file, samples),
    }
}

/// Opus デコーダーを構築する
///
/// libopus は静的リンクされているため追加のロードは不要。
/// dOps の PreSkip と OutputGain をデコーダー設定に反映する。
fn build_opus_source(
    track_info: AudioTrackInfo,
    file: BufReader<File>,
    samples: Vec<AudioSampleMeta>,
) -> Result<Mp4AudioTrackResult> {
    // dOps の OutputGain (Q7.8 固定小数点の dB) をデコーダーに渡す。
    // shiguredo_opus の gain (Q8 dB) は dOps の OutputGain (Q7.8) と同じ
    // 位取り (÷256) のためそのまま使える
    let mut config = shiguredo_opus::DecoderConfig::new(SAMPLE_RATE, track_info.channels);
    config.gain = track_info.opus_output_gain.map(i32::from);
    let decoder = shiguredo_opus::Decoder::new(config)
        .map_err(|e| ErrorMessage::new(format!("Opus デコーダーの生成エラー: {e}")))?;
    let pre_skip = track_info.opus_pre_skip.unwrap_or(0) as usize;
    Ok(Mp4AudioTrackResult::Supported(Box::new(Mp4AudioSource {
        file,
        samples,
        cursor: 0,
        fifo: VecDeque::new(),
        decoder: AudioDecoderKind::Opus {
            decoder,
            pre_skip,
            skip_remaining: pre_skip,
            channels: track_info.channels,
        },
    })))
}

/// AAC デコーダーを構築する (Linux + feature `fdk-aac`)
///
/// libfdk-aac 共有ライブラリを動的ロードする。ロードに失敗した場合と
/// `--fdk-aac-lib` が未指定の場合は起動時エラーを返す。
#[cfg(all(target_os = "linux", feature = "fdk-aac"))]
fn build_aac_source(
    track_info: AudioTrackInfo,
    fdk_aac_lib_path: Option<&str>,
    file: BufReader<File>,
    samples: Vec<AudioSampleMeta>,
) -> Result<Mp4AudioTrackResult> {
    // ロードできない環境で AAC 音声を含む MP4 を指定した場合は起動時エラーにする
    let Some(asc) = track_info.aac_asc else {
        return Ok(unsupported("AAC audio has no AudioSpecificConfig in esds"));
    };
    let lib = match fdk_aac_lib_path {
        Some(path) => shiguredo_fdk_aac::FdkAacLibrary::load(path).map_err(|e| {
            ErrorMessage::new(format!("FDK AAC ライブラリ '{path}' のロードエラー: {e}"))
        })?,
        None => {
            return Err(ErrorMessage::new(
                "--input-mp4 の音声が AAC のため --fdk-aac-lib の指定が必要です",
            )
            .into());
        }
    };
    let decoder = match shiguredo_fdk_aac::Decoder::new(lib.clone(), &asc) {
        Ok(decoder) => decoder,
        Err(err) => {
            return Ok(unsupported(format!(
                "failed to initialize FDK AAC decoder: {err}"
            )));
        }
    };
    // lib はループごとのデコーダー再生成に使うため decoder と共有する clone を保持する
    Ok(Mp4AudioTrackResult::Supported(Box::new(Mp4AudioSource {
        file,
        samples,
        cursor: 0,
        fifo: VecDeque::new(),
        decoder: AudioDecoderKind::Aac { decoder, lib, asc },
    })))
}

/// AAC 音声トラックは Linux + feature `fdk-aac` 以外ではデコードできないため未対応として扱う
#[cfg(not(all(target_os = "linux", feature = "fdk-aac")))]
fn build_aac_source(
    _track_info: AudioTrackInfo,
    _fdk_aac_lib_path: Option<&str>,
    _file: BufReader<File>,
    _samples: Vec<AudioSampleMeta>,
) -> Result<Mp4AudioTrackResult> {
    Ok(unsupported(
        "AAC audio decoding requires building with the fdk-aac feature on Linux",
    ))
}

/// ファイルの指定位置から指定サイズのデータを読み込む
fn read_bytes_at(
    file: &mut BufReader<File>,
    position: u64,
    size: usize,
) -> std::io::Result<Vec<u8>> {
    let mut data = vec![0; size];
    file.seek(SeekFrom::Start(position))?;
    file.read_exact(&mut data)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用 MP4 ファイルのパスを返す
    fn testdata(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name)
    }

    /// 調査結果から `Mp4AudioSource` を取り出す
    fn expect_supported(result: Mp4AudioTrackResult) -> Box<Mp4AudioSource> {
        match result {
            Mp4AudioTrackResult::Supported(source) => source,
            Mp4AudioTrackResult::NoAudioTrack => {
                panic!("音声トラックが検出されませんでした")
            }
            Mp4AudioTrackResult::Unsupported => {
                panic!("未対応として扱われました")
            }
        }
    }

    /// 480 サンプルずつ `count` 回読み出してフラットな PCM 列を返す
    fn read_ticks(source: &mut Mp4AudioSource, count: usize) -> Vec<i16> {
        let mut out = Vec::new();
        let mut buf = [0i16; 480];
        for _ in 0..count {
            source.read_samples(&mut buf);
            out.extend_from_slice(&buf);
        }
        out
    }

    /// Opus デコーダーのフィールドを取り出す (Opus 以外は panic)
    ///
    /// テストから `AudioDecoderKind` の variant を直接 match するパターンが
    /// 多数重複するためヘルパーに集約する。`pre_skip` / `skip_remaining` は
    /// ループ境界の挙動検証で使い、戻り値は (channels, pre_skip, skip_remaining)。
    fn opus_fields(source: &Mp4AudioSource) -> (u8, usize, usize) {
        match &source.decoder {
            AudioDecoderKind::Opus {
                channels,
                pre_skip,
                skip_remaining,
                ..
            } => (*channels, *pre_skip, *skip_remaining),
            #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
            AudioDecoderKind::Aac { .. } => {
                panic!("Opus 入力が AAC として扱われました")
            }
        }
    }

    /// 音声トラックが無い MP4 は映像のみで続行できる結果になる
    #[test]
    fn inspect_returns_no_audio_track_for_video_only_mp4() {
        let result = inspect_mp4_audio(&testdata("mp4-video-only.mp4"), None)
            .expect("映像のみの MP4 はエラーにならないはず");
        assert!(
            matches!(result, Mp4AudioTrackResult::NoAudioTrack),
            "音声トラックが無い MP4 は NoAudioTrack になるはず"
        );
    }

    /// ステレオ Opus の MP4 はデコード可能として扱われる
    #[test]
    fn inspect_returns_supported_for_opus_stereo() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-stereo.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let source = expect_supported(result);
        let (channels, _, _) = opus_fields(&source);
        assert_eq!(channels, 2, "ステレオ Opus のチャンネル数は 2 のはず");
    }

    /// モノラル Opus の MP4 はデコード可能として扱われる
    #[test]
    fn inspect_returns_supported_for_opus_mono() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-mono.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let source = expect_supported(result);
        let (channels, _, _) = opus_fields(&source);
        assert_eq!(channels, 1, "モノラル Opus のチャンネル数は 1 のはず");
    }

    /// Opus のデコードで 480 サンプルの供給と信号の再現ができる
    #[test]
    fn opus_read_samples_fills_480_samples_with_signal() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-stereo.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let mut source = expect_supported(result);
        let pcm = read_ticks(&mut source, 100);
        assert_eq!(pcm.len(), 48000, "100 ticks で 48000 サンプルになるはず");
        assert!(
            pcm.iter().any(|&s| s != 0),
            "正弦波のデコード結果に非ゼロサンプルが含まれるはず"
        );
    }

    /// 1 パケット目のデコードで PreSkip 分が破棄される
    ///
    /// PreSkip は ffmpeg のエンコード設定で決まるため、デコーダーが保持する値から
    /// 期待値を組み立てる。破棄しない場合の 1 パケット分 (960 サンプル) から
    /// PreSkip 分を除いたサンプル数が FIFO に入るはず。
    #[test]
    fn opus_decoder_skips_pre_skip_at_stream_start() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-stereo.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let mut source = expect_supported(result);

        let (_, pre_skip, _) = opus_fields(&source);
        assert!(
            pre_skip > 0,
            "ffmpeg の libopus は非ゼロの PreSkip を書き出すはず"
        );

        source.decode_next_packet(true);
        assert_eq!(
            source.fifo.len(),
            960 - pre_skip,
            "PreSkip 分が破棄されたサンプル数が入るはず"
        );
        let (_, _, skip_remaining) = opus_fields(&source);
        assert_eq!(skip_remaining, 0, "PreSkip の破棄後は残り 0 のはず");
    }

    /// トラック終端からのループ再生で PreSkip の破棄がやり直される
    ///
    /// ループ機構の検証: カーソルを終端に置いた状態でデコードすると先頭に戻り、
    /// デコーダーがリセットされて PreSkip 分の破棄が再び適用される。
    #[test]
    fn opus_loop_applies_pre_skip_again() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-stereo.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let mut source = expect_supported(result);

        let (_, pre_skip, _) = opus_fields(&source);

        // カーソルを終端に置いてループ再生を再現する
        source.cursor = source.samples.len();
        source.decode_next_packet(true);
        assert_eq!(
            source.cursor, 1,
            "終端の次は先頭へ戻って 1 パケット進んだ位置になるはず"
        );
        assert_eq!(
            source.fifo.len(),
            960 - pre_skip,
            "ループ先頭で PreSkip が再適用されるはず"
        );
    }

    /// ループ再生で 2 周目の PCM が 1 周目と完全に一致する
    ///
    /// 決定論性の検証: ループ境界でデコーダーがリセットされ、PreSkip の破棄も
    /// やり直されるため、2 周目の先頭パケットのデコード結果は 1 周目と同一に
    /// なるはず (libopus のデコードは決定論的)。
    #[test]
    fn opus_loop_replays_identical_pcm() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-stereo.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let mut source = expect_supported(result);

        // 1 周目の先頭パケットをデコードして結果を採取する
        source.decode_next_packet(true);
        let first_packet_pcm = source.fifo.clone();

        // 残り全パケットをデコードして終端に達する (次のデコードでループが発生する)
        while source.cursor < source.samples.len() {
            source.decode_next_packet(true);
        }
        source.fifo.clear();

        // 2 周目の先頭パケットをデコードする (ループ = デコーダー reset + PreSkip 再適用)
        source.decode_next_packet(true);
        assert_eq!(
            source.fifo, first_packet_pcm,
            "2 周目の先頭パケットは 1 周目と同一の PCM になるはず"
        );
    }

    /// FIFO が枯渇する場合 (要求バッファがトラック 1 周分を超える場合) は
    /// 足りない分を無音で埋めて継続する
    ///
    /// 1 周分を超えるとデコードは終端でループし、先頭パケット 1 個分
    /// (PreSkip 適用後) が追加で入った後に上限 (全パケット 1 周分) で
    /// 打ち切られ、残りは全て無音になるはず。
    #[test]
    fn read_samples_outputs_silence_when_fifo_starves() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-stereo.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let mut source = expect_supported(result);

        let (_, pre_skip, _) = opus_fields(&source);
        // 1 周分の総サンプル数 = パケット数 × 960 - PreSkip (ダウンミックス後)
        let per_loop = 960 * source.samples.len() - pre_skip;
        let nonzero_len = per_loop + (960 - pre_skip);

        // 1 周分 + ループ先頭パケット分 + 10 ticks を要求する (超えた分は無音になるはず)
        let mut buf = vec![0i16; nonzero_len + 4800];
        source.read_samples(&mut buf);

        assert!(
            buf[..nonzero_len].iter().any(|&s| s != 0),
            "1 周分 + ループ先頭パケット分には信号が含まれるはず"
        );
        assert!(
            buf[nonzero_len..].iter().all(|&s| s == 0),
            "デコードが追いつかない分は無音になるはず"
        );
    }

    /// データ読み込みに失敗する破損入力でもクラッシュせず無音で継続できる
    ///
    /// エラーパスの検証: サンプルのオフセットをファイルサイズ範囲外へ書き換えると
    /// 読み込みに失敗するが、警告のうえパケットをスキップし、要求分を
    /// 無音で埋めて継続する (cursor は全パケット巡回後に先頭へ戻る)。
    #[test]
    fn read_samples_survives_corrupt_sample_offset() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-opus-stereo.mp4"), None)
            .expect("Opus 音声の MP4 はエラーにならないはず");
        let mut source = expect_supported(result);

        // 全サンプルのオフセットをファイルサイズ範囲外へ書き換える
        for sample in &mut source.samples {
            sample.data_offset = u64::MAX / 4;
        }

        let mut buf = [0i16; 480];
        source.read_samples(&mut buf);
        assert!(
            buf.iter().all(|&s| s == 0),
            "読み込み不能なら無音になるはず"
        );
        // 全パケットの失敗後に attempts 上限で打ち切られ、
        // カーソルはループで先頭の次の位置に戻っているはず
        assert_eq!(
            source.cursor, 1,
            "全パケット失敗後はカーソルが先頭に戻っているはず"
        );
    }

    /// 映像と音声の両方を持つ MP4 でも音声トラックが検出できる
    ///
    /// 映像トラックのサンプルが混在するインターリーブ列から、
    /// 音声トラックのサンプルだけを正しく収集できることを確認する。
    #[test]
    fn inspect_returns_supported_for_video_with_opus_audio() {
        let result = inspect_mp4_audio(&testdata("mp4-video-with-opus-audio.mp4"), None)
            .expect("映像 + Opus 音声の MP4 はエラーにならないはず");
        // 音声ソースとして構築でき、映像サンプルのスキップも含めて
        // 480 サンプルの供給ができることまで確認する
        let mut source = expect_supported(result);
        let pcm = read_ticks(&mut source, 10);
        assert_eq!(pcm.len(), 4800, "10 ticks で 4800 サンプルになるはず");
        assert!(
            pcm.iter().any(|&s| s != 0),
            "正弦波のデコード結果に非ゼロサンプルが含まれるはず"
        );
    }

    /// 音声トラックが 2 本以上の MP4 は起動時エラーになる
    #[test]
    fn inspect_rejects_multiple_audio_tracks() {
        let err = inspect_mp4_audio(&testdata("mp4-audio-two-tracks.mp4"), None)
            .expect_err("音声トラック 2 本はエラーになるはず");
        assert!(
            format!("{err}").contains("音声トラック"),
            "エラーメッセージに音声トラックの説明が含まれるはず: {err}"
        );
    }

    /// FLAC 音声の MP4 は未対応として扱われ、映像のみで続行できる
    #[test]
    fn inspect_returns_unsupported_for_flac_audio() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-flac.mp4"), None)
            .expect("未対応コーデックではエラーにならないはず");
        assert!(
            matches!(result, Mp4AudioTrackResult::Unsupported),
            "FLAC 音声は未対応として扱われるはず"
        );
    }

    /// 3 チャンネル以上の音声トラックは未対応として扱われる
    #[test]
    fn inspect_returns_unsupported_for_multi_channel_audio() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-6ch.m4a"), None)
            .expect("チャンネル構成非対応ではエラーにならないはず");
        assert!(
            matches!(result, Mp4AudioTrackResult::Unsupported),
            "6ch 音声は未対応として扱われるはず"
        );
    }

    /// AAC 音声の MP4 は Linux + feature `fdk-aac` 以外では未対応として扱われる
    #[cfg(not(all(target_os = "linux", feature = "fdk-aac")))]
    #[test]
    fn aac_is_unsupported_without_fdk_aac_feature() {
        let result = inspect_mp4_audio(&testdata("mp4-audio-aac-stereo.m4a"), None)
            .expect("fdk-aac feature 無しではエラーにならないはず");
        assert!(
            matches!(result, Mp4AudioTrackResult::Unsupported),
            "AAC 音声は fdk-aac feature 無しでは未対応として扱われるはず"
        );
    }

    /// AAC 音声の MP4 は --fdk-aac-lib 未指定なら起動時エラーになる (Linux + feature `fdk-aac`)
    #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
    #[test]
    fn aac_requires_fdk_aac_lib_option() {
        let err = inspect_mp4_audio(&testdata("mp4-audio-aac-stereo.m4a"), None)
            .expect_err("--fdk-aac-lib 未指定の AAC はエラーになるはず");
        assert!(
            format!("{err}").contains("--fdk-aac-lib"),
            "エラーメッセージに --fdk-aac-lib の案内が含まれるはず: {err}"
        );
    }

    /// libfdk-aac がシステムに導入されているかを確認する
    ///
    /// AAC のデコードテストは libfdk-aac の動的ロードが必要。未導入の
    /// ローカル Linux 環境では検証をスキップする (CI には導入済みなので
    /// CI では常に検証される)。
    #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
    fn fdk_aac_lib_available() -> bool {
        shiguredo_fdk_aac::FdkAacLibrary::load("libfdk-aac.so.2").is_ok()
    }

    /// AAC のデコードで 480 サンプルの供給と信号の再現ができる (Linux + feature `fdk-aac`)
    ///
    /// 44.1kHz ステレオの AAC は 1 フレーム (1024 サンプル/チャンネル) が
    /// 48kHz モノラルへダウンミックス・リサンプリングされて FIFO に入る。
    #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
    #[test]
    fn aac_decodes_and_resamples_to_48khz_mono() {
        if !fdk_aac_lib_available() {
            return;
        }
        let result = inspect_mp4_audio(
            &testdata("mp4-audio-aac-stereo.m4a"),
            Some("libfdk-aac.so.2"),
        )
        .expect("libfdk-aac がロードできればエラーにならないはず");
        let mut source = expect_supported(result);

        source.decode_next_packet(true);
        // 1024 / (44100/48000) = 1114 (端数切り捨て)
        assert_eq!(
            source.fifo.len(),
            1114,
            "44.1kHz ステレオ 1 フレームが 48kHz モノラル 1114 サンプルになるはず"
        );

        let pcm = read_ticks(&mut source, 100);
        assert_eq!(pcm.len(), 48000, "100 ticks で 48000 サンプルになるはず");
        assert!(
            pcm.iter().any(|&s| s != 0),
            "正弦波のデコード結果に非ゼロサンプルが含まれるはず"
        );
    }

    /// ループ再生で 2 周目の PCM が 1 周目と完全に一致する (Linux + feature `fdk-aac`)
    ///
    /// ループ境界で FDK デコーダーが作り直され、各ループがストリーム先頭からの
    /// 再生として扱われるため、2 周目の先頭パケットのデコード結果は
    /// 1 周目と同一になるはず (FDK AAC のデコードは決定論的)。
    #[cfg(all(target_os = "linux", feature = "fdk-aac"))]
    #[test]
    fn aac_loop_replays_identical_pcm() {
        if !fdk_aac_lib_available() {
            return;
        }
        let result = inspect_mp4_audio(
            &testdata("mp4-audio-aac-stereo.m4a"),
            Some("libfdk-aac.so.2"),
        )
        .expect("libfdk-aac がロードできればエラーにならないはず");
        let mut source = expect_supported(result);

        // 1 周目の先頭パケットをデコードして結果を採取する
        source.decode_next_packet(true);
        let first_packet_pcm = source.fifo.clone();

        // 残り全パケットをデコードして終端に達する (次のデコードでループが発生する)
        while source.cursor < source.samples.len() {
            source.decode_next_packet(true);
        }
        source.fifo.clear();

        // 2 周目の先頭パケットをデコードする (ループ = デコーダー再生成)
        source.decode_next_packet(true);
        assert_eq!(
            source.fifo, first_packet_pcm,
            "2 周目の先頭パケットは 1 周目と同一の PCM になるはず"
        );
    }
}
