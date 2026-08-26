//! 数字音声のリーダー
//!
//! C++ 版 zakuro の `VoiceNumberReader` に相当する。0-99 の番号に対応する
//! 数字音声を、埋め込んだ WAV 断片の連結で合成して 48kHz モノラル i16 の
//! サンプル列として返す。断片は 16kHz モノラルのため `WavReader` の
//! リサンプル処理で 48kHz に変換する。

use std::sync::OnceLock;

use crate::wav_reader::WavReader;

/// 0-29 の単体用断片 (num000_01.wav 〜 num029_01.wav、番号順)
const SINGLE_FRAGMENTS: [&[u8]; 30] = [
    include_bytes!("../resource/voice_number/num000_01.wav"),
    include_bytes!("../resource/voice_number/num001_01.wav"),
    include_bytes!("../resource/voice_number/num002_01.wav"),
    include_bytes!("../resource/voice_number/num003_01.wav"),
    include_bytes!("../resource/voice_number/num004_01.wav"),
    include_bytes!("../resource/voice_number/num005_01.wav"),
    include_bytes!("../resource/voice_number/num006_01.wav"),
    include_bytes!("../resource/voice_number/num007_01.wav"),
    include_bytes!("../resource/voice_number/num008_01.wav"),
    include_bytes!("../resource/voice_number/num009_01.wav"),
    include_bytes!("../resource/voice_number/num010_01.wav"),
    include_bytes!("../resource/voice_number/num011_01.wav"),
    include_bytes!("../resource/voice_number/num012_01.wav"),
    include_bytes!("../resource/voice_number/num013_01.wav"),
    include_bytes!("../resource/voice_number/num014_01.wav"),
    include_bytes!("../resource/voice_number/num015_01.wav"),
    include_bytes!("../resource/voice_number/num016_01.wav"),
    include_bytes!("../resource/voice_number/num017_01.wav"),
    include_bytes!("../resource/voice_number/num018_01.wav"),
    include_bytes!("../resource/voice_number/num019_01.wav"),
    include_bytes!("../resource/voice_number/num020_01.wav"),
    include_bytes!("../resource/voice_number/num021_01.wav"),
    include_bytes!("../resource/voice_number/num022_01.wav"),
    include_bytes!("../resource/voice_number/num023_01.wav"),
    include_bytes!("../resource/voice_number/num024_01.wav"),
    include_bytes!("../resource/voice_number/num025_01.wav"),
    include_bytes!("../resource/voice_number/num026_01.wav"),
    include_bytes!("../resource/voice_number/num027_01.wav"),
    include_bytes!("../resource/voice_number/num028_01.wav"),
    include_bytes!("../resource/voice_number/num029_01.wav"),
];

/// 十の位の単体用断片 (30/40/.../90 用、n/10 で引く)
///
/// 0-2 は使用しない (0-29 は単体断片で扱う) ため空で埋める。
const TENS_SINGLE_FRAGMENTS: [&[u8]; 10] = [
    &[],
    &[],
    &[],
    include_bytes!("../resource/voice_number/num030_01.wav"),
    include_bytes!("../resource/voice_number/num040_01.wav"),
    include_bytes!("../resource/voice_number/num050_01.wav"),
    include_bytes!("../resource/voice_number/num060_01.wav"),
    include_bytes!("../resource/voice_number/num070_01.wav"),
    include_bytes!("../resource/voice_number/num080_01.wav"),
    include_bytes!("../resource/voice_number/num090_01.wav"),
];

/// 十の位の連結用断片 (31-99 用、n/10 で引く)
///
/// 0-2 は使用しない (0-29 は単体断片で扱う) ため空で埋める。
const TENS_CONCAT_FRAGMENTS: [&[u8]; 10] = [
    &[],
    &[],
    &[],
    include_bytes!("../resource/voice_number/num030_02.wav"),
    include_bytes!("../resource/voice_number/num040_02.wav"),
    include_bytes!("../resource/voice_number/num050_02.wav"),
    include_bytes!("../resource/voice_number/num060_02.wav"),
    include_bytes!("../resource/voice_number/num070_02.wav"),
    include_bytes!("../resource/voice_number/num080_02.wav"),
    include_bytes!("../resource/voice_number/num090_02.wav"),
];

/// 48kHz モノラル i16 にリサンプル済みの断片の集合
struct Fragments {
    /// 0-29 の単体用断片 (番号順)
    single: [Vec<i16>; 30],
    /// 十の位の単体用断片 (30/40/.../90 用、n/10 で引く)
    tens_single: [Vec<i16>; 10],
    /// 十の位の連結用断片 (31-99 用、n/10 で引く)
    tens_concat: [Vec<i16>; 10],
}

/// 断片のキャッシュ (初回呼び出し時に一度だけパースする)
static FRAGMENTS: OnceLock<Fragments> = OnceLock::new();

/// 番号に対応する数字音声のサンプル列 (48kHz モノラル i16) を返す
///
/// 合成規則は C++ 版 `VoiceNumberReader::Read` と同じ:
/// - 0-29 は単体断片そのまま
/// - 30/40/.../90 は十の位の単体断片のみ
/// - 31-99 は十の位の連結用断片 + 一の位の単体断片を連結する
/// - 100 以上は空を返す
pub(crate) fn read(number: u32) -> Vec<i16> {
    if number >= 100 {
        return Vec::new();
    }
    let frags = fragments();
    let n = number as usize;
    match n {
        0..=29 => frags.single[n].clone(),
        // 30/40/.../90 は十の位のみ (31-99 の一の位 0 は存在しない)
        n if n % 10 == 0 => frags.tens_single[n / 10].clone(),
        // 31-99 は十の位 (連結用) + 一の位の連結
        _ => {
            let mut out = frags.tens_concat[n / 10].clone();
            out.extend_from_slice(&frags.single[n % 10]);
            out
        }
    }
}

/// 埋め込み断片を 48kHz モノラル i16 に変換する
///
/// 未使用スロット (十の位の 0-2) は空のため空ベクタを返す。
fn parse_fragment(bytes: &[u8]) -> Vec<i16> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let reader =
        WavReader::from_bytes(bytes).expect("embedded voice number WAV fragment must be parseable");
    reader.samples().to_vec()
}

fn fragments() -> &'static Fragments {
    FRAGMENTS.get_or_init(|| Fragments {
        single: std::array::from_fn(|i| parse_fragment(SINGLE_FRAGMENTS[i])),
        tens_single: std::array::from_fn(|i| parse_fragment(TENS_SINGLE_FRAGMENTS[i])),
        tens_concat: std::array::from_fn(|i| parse_fragment(TENS_CONCAT_FRAGMENTS[i])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0-29 は単体断片そのままであることを検証する
    #[test]
    fn read_single_digits_use_single_fragment() {
        let frags = fragments();
        assert_eq!(read(0), frags.single[0], "0 は単体断片そのまま");
        assert_eq!(read(1), frags.single[1], "1 は単体断片そのまま");
        assert_eq!(read(29), frags.single[29], "29 は単体断片そのまま");
    }

    /// 30/40/.../90 は十の位の単体断片のみであることを検証する
    #[test]
    fn read_tens_only_use_tens_single_fragment() {
        let frags = fragments();
        assert_eq!(read(30), frags.tens_single[3], "30 は十の位のみ");
        assert_eq!(read(40), frags.tens_single[4], "40 は十の位のみ");
        assert_eq!(read(90), frags.tens_single[9], "90 は十の位のみ");
    }

    /// 31-99 は十の位の連結用断片 + 一の位の連結であることを検証する
    #[test]
    fn read_tens_and_ones_concatenate_fragments() {
        let frags = fragments();
        let mut expected31 = frags.tens_concat[3].clone();
        expected31.extend_from_slice(&frags.single[1]);
        assert_eq!(read(31), expected31, "31 は十の位 (連結用) + 一の位の連結");

        let mut expected99 = frags.tens_concat[9].clone();
        expected99.extend_from_slice(&frags.single[9]);
        assert_eq!(read(99), expected99, "99 は十の位 (連結用) + 一の位の連結");
    }

    /// 100 以上は空を返すことを検証する (C++ 版の Read と同じ)
    #[test]
    fn read_over_100_returns_empty() {
        assert!(read(100).is_empty(), "100 は空を返すこと");
        assert!(read(1000).is_empty(), "1000 は空を返すこと");
    }

    /// 0-99 の全数で合成規則を満たすことを検証する
    ///
    /// 断片の番号 ↔ ファイル名の対応が入れ替わっても検出できるよう、全数スイープで
    /// 0-29 は単体 / 30,40,...,90 は十の位のみ / 31-99 は連結を検証する。
    #[test]
    fn read_all_numbers_follow_synthesis_rules() {
        let frags = fragments();
        for number in 0..100u32 {
            let samples = read(number);
            assert!(!samples.is_empty(), "{number} の音声は空でないこと");
            let n = number as usize;
            let expected = if n <= 29 {
                frags.single[n].clone()
            } else if n.is_multiple_of(10) {
                frags.tens_single[n / 10].clone()
            } else {
                let mut out = frags.tens_concat[n / 10].clone();
                out.extend_from_slice(&frags.single[n % 10]);
                out
            };
            assert_eq!(samples, expected, "{number} の合成規則が正しいこと");
        }
    }

    /// 断片が 48kHz にリサンプルされていることを検証する
    ///
    /// 埋め込み断片は 16kHz モノラルのため、48kHz では 3 倍のサンプル数になる。
    /// このテストは断片の変更 (再生成など) を検出する金値でもある。
    #[test]
    fn read_resamples_fragments_to_48khz() {
        let frags = fragments();
        let reader =
            WavReader::from_bytes(SINGLE_FRAGMENTS[0]).expect("埋め込み断片はパースできること");
        assert_eq!(
            frags.single[0].len(),
            reader.samples().len(),
            "単体断片と同経路でリサンプルされること"
        );
        // num000_01.wav は 16kHz 7454 サンプル → 48kHz 22362 サンプル
        assert_eq!(frags.single[0].len(), 7454 * 3, "48kHz で 3 倍になること");
    }
}
