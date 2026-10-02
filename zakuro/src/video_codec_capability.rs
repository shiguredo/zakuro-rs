//! ビデオコーデック能力の一覧表示 (`--show-video-codec-capability`)
//!
//! 各コーデック実装 (capability) ごとの Encoder / Decoder 対応とコーデックパラメータを
//! 標準出力へ表示する。書式は C++ 版 zakuro の `show_video_codec_capability` を基本とするが、
//! 次の 4 点は sora_sdk の型と環境に合わせた意図した差分とする:
//!
//! - `Engine:` 行に説明文を併記する
//! - `Codec Parameters:` は JSON ではなく `key=value` のカンマ区切りで表示する
//!   (`sora_sdk` の `get_supported_formats()` が返すのは SDP パラメータのため)
//!   (パラメータ行はフォーマットごとに表示する)
//! - `Engine Parameters:` 行は表示しない (sora_sdk に相当する情報がない)
//! - 受信ロール指定時は nop エンジンを表示する (C++ 版の表示は内蔵エンジン群のみ)。
//!   また C++ 版は macOS で内蔵コーデックを 1 エンジンにまとめるが、sora_sdk は
//!   internal と internal-apple の 2 エンジンを登録するため、表示もそのまま 2 エンジンになる
//! - `--input-mp4` 指定時は mp4-passthrough エンジンを表示する (Rust 版独自拡張。
//!   C++ 版の表示には MP4 パススルーがない)。ハードウェア系 (vpl / nvcodec / amf) の
//!   エンジンは sora_sdk の機能未対応のため表示されない (C++ 版は提示する場合がある)
//!
//! コーデック対応の判定は `get_supported_formats()` の名前一致で行う。
//! `VideoCodecCapability::is_supported()` はコーデック種別の真偽しか返さず、
//! 対応の根拠となるフォーマット列自体を列挙できないため、表示の判定には使わない
//! (C++ 版 zakuro もフォーマット名の存在で判定する)。
//!
//! 表示は標準出力のみに出力し、表示経路は通常の引数検証 (未知引数・値欠落のエラー化、
//! `--config` の存在確認など) を実施しない (C++ 版は CLI11 が先に全引数を検証する)。
//! ただし `SoraConnectionContextConfig::default()` の構築時に libwebrtc 内部の
//! SDP フォーマット照合ログが標準エラーへ出力される。これは sora_sdk の既定構築に
//! 起因する既知の挙動である。

use shiguredo_webrtc::{SdpVideoFormat, VideoCodecType};
use sora_sdk::{CodecDirection, VideoCodecCapability};

/// コーデック種別の表示順
///
/// C++ 版 zakuro の `show_video_codec_capability` の列挙順に合わせる。
const CODEC_TYPES_DISPLAY_ORDER: [VideoCodecType; 5] = [
    VideoCodecType::Vp8,
    VideoCodecType::Vp9,
    VideoCodecType::H264,
    VideoCodecType::H265,
    VideoCodecType::Av1,
];

/// フォーマットのパラメータを `key=value` のカンマ区切り文字列にする
///
/// パラメータが無い場合は `None` を返す。キー名の昇順にソートして表示順を確定させる
/// (libwebrtc 側の `std::map` は昇順だが、表示は自前で順序を固定する)。
fn format_codec_parameters(format: &mut SdpVideoFormat) -> Option<String> {
    let mut parameters: Vec<String> = format
        .parameters_mut()
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if parameters.is_empty() {
        return None;
    }
    parameters.sort();
    Some(parameters.join(", "))
}

/// capability 群から表示文字列を構築する
///
/// ```text
/// Engine: <実装名> (<説明文>)
///   - <CODEC> Encoder
///     - Codec Parameters: <key>=<value> ...
///   - <CODEC> Decoder
/// ```
fn build_capability_display(capabilities: &[Box<dyn VideoCodecCapability>]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for capability in capabilities {
        let implementation = capability.get_implementation();
        lines.push(format!(
            "Engine: {} ({})",
            implementation.name(),
            implementation.description()
        ));
        for codec_type in CODEC_TYPES_DISPLAY_ORDER {
            let Some(codec_name) = codec_type.as_str() else {
                continue;
            };
            for direction in [CodecDirection::Encoder, CodecDirection::Decoder] {
                let mut format_and_parameters: Vec<(SdpVideoFormat, Option<String>)> = capability
                    .get_supported_formats(direction)
                    .into_iter()
                    .filter(|full_format| full_format.name().ok().as_deref() == Some(codec_name))
                    // パラメータ文字列は 1 度だけ計算して使い回す
                    .map(|mut full_format| {
                        let parameters = format_codec_parameters(&mut full_format);
                        (full_format, parameters)
                    })
                    .collect();
                if format_and_parameters.is_empty() {
                    continue;
                }
                // フォーマット列の並びは環境依存のため、パラメータ文字列で整列して決定性を持たせる
                format_and_parameters.sort_by(|a, b| a.1.cmp(&b.1));
                lines.push(format!("  - {codec_name} {}", direction.as_str()));
                for (_, parameters) in format_and_parameters {
                    if let Some(parameters) = parameters {
                        lines.push(format!("    - Codec Parameters: {parameters}"));
                    }
                }
            }
        }
    }
    lines.join("\n")
}

/// ビデオコーデック能力を表示して終了する
///
/// `--show-video-codec-capability` の pre-parse 処理から呼ばれる。
/// 表示先は標準出力のみで、ログ (標準エラー) とは分離する。
pub(crate) fn show_video_codec_capability(capabilities: Vec<Box<dyn VideoCodecCapability>>) -> ! {
    println!("{}", build_capability_display(&capabilities));
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nop_video_decoder::NopVideoDecoderCapability;
    use sora_sdk::InternalVideoCodecCapability;

    #[test]
    fn nop_capability_lists_all_decoder_directions_only() {
        // NopVideoDecoder は Decoder 方向のみ全コーデック種別をサポートする
        let capabilities: Vec<Box<dyn VideoCodecCapability>> =
            vec![Box::new(NopVideoDecoderCapability)];
        let text = build_capability_display(&capabilities);
        let expected = [
            "Engine: nop (Nop Video Decoder)",
            "  - VP8 Decoder",
            "  - VP9 Decoder",
            "  - H264 Decoder",
            "  - H265 Decoder",
            "  - AV1 Decoder",
        ]
        .join("\n");
        assert_eq!(text, expected);
    }

    #[test]
    fn internal_capability_is_displayed_before_nop_capability() {
        // 表示は capability の登録順を維持する
        let capabilities: Vec<Box<dyn VideoCodecCapability>> = vec![
            Box::new(InternalVideoCodecCapability::new()),
            Box::new(NopVideoDecoderCapability),
        ];
        let text = build_capability_display(&capabilities);
        let nop_position = text
            .lines()
            .position(|line| line.starts_with("Engine: nop"))
            .expect("nop エンジンの行が存在するべき");
        assert!(
            nop_position > 0,
            "internal エンジンが nop エンジンより先に表示されるべき"
        );
        assert!(
            text.starts_with("Engine: internal (WebRTC built-in VideoCodecFactory)"),
            "internal エンジンが先頭に表示されるべき: {text}"
        );
        // internal エンジンには VP9 が登録されている (builtin factory のソフトウェア
        // コーデック: libvpx) ため、表示されることを確認する。VP9 フォーマットには
        // profile-id パラメータが付くため、パラメータ行の組み立てまで通ることも同時に確認する
        let internal_section = &text[..text
            .find("\nEngine: nop")
            .expect("nop 区切り行が存在するべき")];
        assert!(
            internal_section.contains("  - VP9 Encoder"),
            "internal エンジンに VP9 Encoder が表示されるべき: {internal_section}"
        );
        assert!(
            internal_section.contains("    - Codec Parameters: profile-id=0"),
            "VP9 のパラメータ行が表示されるべき: {internal_section}"
        );
    }

    #[test]
    fn format_codec_parameters_joins_sorted_key_value_pairs() {
        // パラメータはキー名の昇順で `key=value` に結合される
        let mut format = SdpVideoFormat::new_with_parameters(
            "H264",
            &std::collections::HashMap::from([
                ("profile-level-id".to_string(), "42e01f".to_string()),
                ("packetization-mode".to_string(), "1".to_string()),
            ]),
            &[],
        );
        assert_eq!(
            format_codec_parameters(&mut format).as_deref(),
            Some("packetization-mode=1, profile-level-id=42e01f")
        );
    }

    #[test]
    fn format_codec_parameters_returns_none_when_empty() {
        let mut format = SdpVideoFormat::new("VP8");
        assert_eq!(format_codec_parameters(&mut format), None);
    }
}
