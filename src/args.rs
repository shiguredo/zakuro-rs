use nojson::{JsonValueKind, RawJson, RawJsonValue};
use shiguredo_webrtc::{VideoCodecType, rtc_log_info, rtc_log_warning};
use sora_sdk::Role;

use crate::error::{ErrorMessage, Result};
use crate::scenario::ScenarioType;

/// プロセス全体で共有する設定 (HTTP サーバー / Ctrl+C ハンドラ / OpenH264 ライブラリ / mTLS / DuckDB 等)
#[derive(Debug, Clone)]
pub(crate) struct CommonArgs {
    pub(crate) instance_hatch_rate: f64,
    pub(crate) http_host: Option<String>,
    pub(crate) http_port: Option<u16>,
    pub(crate) openh264: Option<String>,
    pub(crate) insecure: bool,
    pub(crate) client_cert: Option<String>,
    pub(crate) client_key: Option<String>,
    /// DuckDB ファイルの出力ディレクトリ (デフォルトはカレントディレクトリ)
    pub(crate) duckdb_output_dir: String,
    /// DuckDB への統計書き込み間隔 (秒)
    pub(crate) duckdb_interval: f64,
    /// DuckDB 出力を無効化する (`--no-duckdb-output`)
    pub(crate) no_duckdb_output: bool,
}

/// インスタンスごとの設定 (vc 群・Sora 接続 / 映像音声キャプチャ / シナリオ等)
#[derive(Debug, Clone)]
pub(crate) struct InstanceArgs {
    pub(crate) signaling_urls: Vec<String>,
    pub(crate) channel_id: String,
    pub(crate) role: Role,
    pub(crate) client_id: Option<String>,
    pub(crate) bundle_id: Option<String>,
    pub(crate) metadata: Option<String>,
    pub(crate) signaling_notify_metadata: Option<String>,
    pub(crate) vcs: u32,
    pub(crate) vcs_hatch_rate: f64,
    pub(crate) duration: Option<f64>,
    pub(crate) repeat_interval: Option<f64>,
    pub(crate) max_retry: u32,
    pub(crate) retry_interval: f64,
    pub(crate) no_video_device: bool,
    pub(crate) no_audio_device: bool,
    pub(crate) video_input_device: Option<String>,
    pub(crate) resolution: (i32, i32),
    pub(crate) framerate: u32,
    pub(crate) sandstorm: bool,
    pub(crate) input_y4m: Option<String>,
    pub(crate) input_mp4: Option<String>,
    pub(crate) input_wav: Option<String>,
    pub(crate) video_codec_type: Option<String>,
    pub(crate) video_bit_rate: Option<u32>,
    pub(crate) audio: bool,
    pub(crate) audio_codec_type: Option<String>,
    pub(crate) audio_bit_rate: Option<u32>,
    pub(crate) data_channels: Option<String>,
    pub(crate) data_channel_signaling: Option<bool>,
    pub(crate) ignore_disconnect_websocket: Option<bool>,
    pub(crate) disconnect_wait_timeout: Option<f64>,
    pub(crate) simulcast: Option<bool>,
    pub(crate) simulcast_request_rid: Option<String>,
    pub(crate) spotlight: Option<bool>,
    pub(crate) spotlight_focus_rid: Option<String>,
    pub(crate) spotlight_unfocus_rid: Option<String>,
    pub(crate) scenario: Option<ScenarioType>,
}

/// JSONC ファイルから抽出した argv 群
///
/// `common_argv` は `CommonArgs` パース用、`instance_argvs[i]` は i 番目の
/// `InstanceArgs` パース用に渡す (どちらも先頭に `program_name` を付与してから
/// `noargs::RawArgs::new` に流す)。`instance_argvs[i]` には JSONC 最上位の
/// テンプレートが先に焼き込まれており、`instances[i]` 由来の引数が末尾に連結されている。
#[derive(Debug)]
pub(crate) struct JsoncConfig {
    pub(crate) common_argv: Vec<String>,
    pub(crate) instance_argvs: Vec<Vec<String>>,
}

/// `CommonArgs` に分類すべきキーかどうか
///
/// `CommonArgs` にフィールドを追加するときは本関数も更新すること。
fn is_common_key(key: &str) -> bool {
    matches!(
        key,
        "instance-hatch-rate"
            | "http-host"
            | "http-port"
            | "openh264"
            | "insecure"
            | "client-cert"
            | "client-key"
            | "duckdb-output-dir"
            | "duckdb-interval"
            | "no-duckdb-output"
    )
}

/// 値を伴わない bool フラグ単体のキー全体 (CommonArgs / InstanceArgs を問わない)
///
/// CLI argv 分割ロジックで「次トークンを値として取るか取らないか」の判別に用いる。
/// 振り分け先 (common / instance) の判定は別途 `is_common_key()` で行うこと。
/// bool フラグを追加するときは本関数も更新すること。
fn is_flag(key: &str) -> bool {
    matches!(
        key,
        "insecure"  // CommonArgs
            | "no-duckdb-output"  // CommonArgs
            | "no-video-device"  // InstanceArgs
            | "no-audio-device"  // InstanceArgs
            | "sandstorm" // InstanceArgs
    )
}

/// 解像度文字列をパースする
fn parse_resolution(s: &str) -> Result<(i32, i32)> {
    match s {
        "QVGA" => Ok((320, 240)),
        "VGA" => Ok((640, 480)),
        "HD" => Ok((1280, 720)),
        "FHD" => Ok((1920, 1080)),
        "4K" => Ok((3840, 2160)),
        _ => {
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() != 2 {
                return Err(ErrorMessage::new(format!(
                    "resolution は QVGA/VGA/HD/FHD/4K または WxH で指定してください: {s}"
                ))
                .into());
            }
            let width: i32 = parts[0].parse().map_err(|_| {
                ErrorMessage::new(format!("resolution の幅が不正です: {}", parts[0]))
            })?;
            let height: i32 = parts[1].parse().map_err(|_| {
                ErrorMessage::new(format!("resolution の高さが不正です: {}", parts[1]))
            })?;
            if width <= 0 || height <= 0 {
                return Err(
                    ErrorMessage::new("resolution の幅と高さは正の整数で指定してください").into(),
                );
            }
            Ok((width, height))
        }
    }
}

/// `--sora-video-codec-type` の文字列を VideoCodecType に変換する
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

/// 単一の JSON 値を `--{key} {value}` の形に展開する
///
/// JSONC 最上位 / `instances[i]` 内のどちらでも同じ規則を使うため切り出し。
/// `instances[i]` 内では `sora` ネストは `--sora-{subkey}` にフラット展開する
/// (本関数は呼び出し側で `sora` を処理した後の単一値に対して呼ぶ)。
fn push_kv(key: &str, value: RawJsonValue<'_, '_>, argv: &mut Vec<String>) -> Result<()> {
    match value.kind() {
        JsonValueKind::Boolean => {
            let b: bool = value
                .try_into()
                .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
            if b {
                argv.push(format!("--{key}"));
            }
        }
        JsonValueKind::String => {
            let s: String = value
                .try_into()
                .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
            argv.push(format!("--{key}"));
            argv.push(s);
        }
        JsonValueKind::Integer | JsonValueKind::Float => {
            argv.push(format!("--{key}"));
            argv.push(value.as_raw_str().to_string());
        }
        _ => {
            // Array / Object はそのまま JSON 文字列として渡す
            argv.push(format!("--{key}"));
            argv.push(value.as_raw_str().to_string());
        }
    }
    Ok(())
}

/// `sora` キー配下のオブジェクトを `--sora-{subkey}` 群に展開する
///
/// `sora.signaling-url` が配列の場合のみカンマ結合して 1 引数に詰める
/// (zakuro-rs の `--sora-signaling-url` は `split(',')` 仕様のため)。
/// それ以外の Array / Object は `push_kv()` の規則で 1 引数化する。
fn flatten_sora_object(
    sora_value: RawJsonValue<'_, '_>,
    argv: &mut Vec<String>,
    label: &str,
) -> Result<()> {
    if sora_value.kind() != JsonValueKind::Object {
        return Err(ErrorMessage::new(format!("'sora' must be a JSON object (in {label})")).into());
    }
    let members = sora_value
        .to_object()
        .map_err(|e| ErrorMessage::new(format!("sora object parse error: {e}")))?;
    for (key_value, value) in members {
        let key: String = key_value
            .try_into()
            .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;

        if key == "signaling-url" && value.kind() == JsonValueKind::Array {
            // 配列を Comma 結合して 1 引数で渡す
            let mut joined = String::new();
            let mut first = true;
            for elem in value
                .to_array()
                .map_err(|e| ErrorMessage::new(format!("sora.signaling-url parse error: {e}")))?
            {
                let url: String = match elem.kind() {
                    JsonValueKind::String => elem
                        .try_into()
                        .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?,
                    _ => {
                        return Err(ErrorMessage::new(
                            "sora.signaling-url の要素は文字列で指定してください",
                        )
                        .into());
                    }
                };
                if !first {
                    joined.push(',');
                }
                joined.push_str(&url);
                first = false;
            }
            argv.push("--sora-signaling-url".to_string());
            argv.push(joined);
        } else {
            push_kv(&format!("sora-{key}"), value, argv)?;
        }
    }
    Ok(())
}

/// 1.2 節「初期実装の対象外」キーを判定する
///
/// 該当キーは警告ログを出して無視する (最上位・`instances[i]` 内の両方で同じ規則)。
fn is_unsupported_key(key: &str) -> bool {
    matches!(key, "instance-num" | "name")
}

/// JSONC 設定ファイルを読み込み、`JsoncConfig` を構築する
pub(crate) fn load_jsonc_config(path: &str) -> Result<JsoncConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ErrorMessage::new(format!("config file read error: {e}")))?;
    parse_jsonc_config(&content)
}

/// JSONC 文字列をパースして `JsoncConfig` を構築する
///
/// テストから直接呼び出せるように env / I/O 依存を持たない。
fn parse_jsonc_config(content: &str) -> Result<JsoncConfig> {
    let (json, _) = RawJson::parse_jsonc(content)
        .map_err(|e| ErrorMessage::new(format!("config file parse error: {e}")))?;
    let root = json.value();
    if root.kind() != JsonValueKind::Object {
        return Err(ErrorMessage::new("config file must be a JSON object").into());
    }

    let mut common_argv: Vec<String> = Vec::new();
    let mut instance_template_argv: Vec<String> = Vec::new();
    let mut instances_value: Option<RawJsonValue<'_, '_>> = None;

    let members = root
        .to_object()
        .map_err(|e| ErrorMessage::new(format!("config file parse error: {e}")))?;
    for (key_value, value) in members {
        let key: String = key_value
            .try_into()
            .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;

        // 再帰ロード防止
        if key == "config" {
            return Err(
                ErrorMessage::new("'config' cannot be specified inside config file").into(),
            );
        }

        // ${...} 形式の環境変数置換は現バージョンでは未対応 (検出して警告する)
        if value.kind() == JsonValueKind::String && value.as_raw_str().contains("${") {
            rtc_log_warning!(
                "Unsupported top-level config key '{}', ignoring (env var substitution not supported)",
                key,
            );
            continue;
        }

        if key == "instances" {
            instances_value = Some(value);
        } else if is_common_key(&key) {
            push_kv(&key, value, &mut common_argv)?;
        } else if is_unsupported_key(&key) {
            rtc_log_warning!("Unsupported top-level config key '{}', ignoring", key);
        } else if key == "sora" {
            flatten_sora_object(value, &mut instance_template_argv, "top level")?;
        } else {
            push_kv(&key, value, &mut instance_template_argv)?;
        }
    }

    // `instances` キーが無ければテンプレートのみで 1 インスタンス分扱う (後方互換)
    let instance_argvs = match instances_value {
        Some(value) => expand_instances(value, &instance_template_argv)?,
        None => vec![instance_template_argv],
    };

    Ok(JsoncConfig {
        common_argv,
        instance_argvs,
    })
}

/// `instances` 配列を展開する
fn expand_instances(value: RawJsonValue<'_, '_>, template: &[String]) -> Result<Vec<Vec<String>>> {
    if value.kind() != JsonValueKind::Array {
        return Err(ErrorMessage::new("'instances' must be a JSON array").into());
    }

    let elements: Vec<RawJsonValue<'_, '_>> = value
        .to_array()
        .map_err(|e| ErrorMessage::new(format!("instances parse error: {e}")))?
        .collect();
    if elements.is_empty() {
        return Err(ErrorMessage::new("instances は 1 から 64 の範囲で指定してください").into());
    }
    if elements.len() > 64 {
        return Err(ErrorMessage::new("instances は 1 から 64 の範囲で指定してください").into());
    }

    let mut result: Vec<Vec<String>> = Vec::new();
    for (i, instance) in elements.into_iter().enumerate() {
        if instance.kind() != JsonValueKind::Object {
            return Err(ErrorMessage::new(format!(
                "instances[{i}] は JSON オブジェクトで指定してください"
            ))
            .into());
        }

        let mut argv: Vec<String> = template.to_vec();
        let members = instance
            .to_object()
            .map_err(|e| ErrorMessage::new(format!("instances[{i}] parse error: {e}")))?;
        for (key_value, value) in members {
            let key: String = key_value
                .try_into()
                .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;

            if key == "config" {
                return Err(ErrorMessage::new(format!(
                    "'config' cannot be specified inside instances[{i}]"
                ))
                .into());
            }
            if is_common_key(&key) {
                return Err(ErrorMessage::new(format!(
                    "common option '{key}' cannot be specified inside instances[{i}]"
                ))
                .into());
            }
            // `instances[i]` 内に `sora-*` のフラットキーを書かれると `sora` Object 形式と
            // 衝突するためサイレント上書きにせずエラーにする
            if key.starts_with("sora-") {
                return Err(ErrorMessage::new(format!(
                    "'{key}' must be nested under 'sora' object inside instances[{i}]"
                ))
                .into());
            }
            // ${...} 形式の環境変数置換は現バージョンでは未対応 (検出して警告する)
            if value.kind() == JsonValueKind::String && value.as_raw_str().contains("${") {
                rtc_log_warning!(
                    "Unsupported config key '{}' in instances[{}], ignoring (env var substitution not supported)",
                    key,
                    i,
                );
                continue;
            }
            if is_unsupported_key(&key) {
                rtc_log_warning!(
                    "Unsupported config key '{}' in instances[{}], ignoring",
                    key,
                    i,
                );
                continue;
            }
            if key == "sora" {
                flatten_sora_object(value, &mut argv, &format!("instances[{i}]"))?;
            } else {
                push_kv(&key, value, &mut argv)?;
            }
        }
        result.push(argv);
    }

    Ok(result)
}

/// `std::env::args()` を `[CommonArgs 用, InstanceArgs 用]` に分割する
///
/// 分割は `is_common_key()` で振り分け、「次トークンを値として取るか取らないか」は
/// `is_flag()` で判定する (振り分け先と独立)。
///
/// `--config` / `--help` / `--version` は呼び出し元で除外済みである前提。
fn split_cli_argv(cli_argv: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut common: Vec<String> = Vec::new();
    let mut instance: Vec<String> = Vec::new();
    let mut i = 0;
    while i < cli_argv.len() {
        let token = &cli_argv[i];
        if let Some(eq_pos) = token.find('=')
            && token.starts_with("--")
        {
            // --key=value 形式
            let key = &token[2..eq_pos];
            if is_common_key(key) {
                common.push(token.clone());
            } else {
                instance.push(token.clone());
            }
            i += 1;
            continue;
        }
        if let Some(key) = token.strip_prefix("--") {
            if is_flag(key) {
                if is_common_key(key) {
                    common.push(token.clone());
                } else {
                    instance.push(token.clone());
                }
                i += 1;
                continue;
            }
            // 値付きオプションは次トークンも同じ側に push
            if is_common_key(key) {
                common.push(token.clone());
                if i + 1 < cli_argv.len() {
                    common.push(cli_argv[i + 1].clone());
                }
            } else {
                instance.push(token.clone());
                if i + 1 < cli_argv.len() {
                    instance.push(cli_argv[i + 1].clone());
                }
            }
            i += 2;
            continue;
        }
        // 想定外の素のトークン (positional) はそのまま InstanceArgs 側に流す
        instance.push(token.clone());
        i += 1;
    }
    (common, instance)
}

/// 同名オプション・フラグが複数登場する argv を「後勝ち重複除去」する
///
/// noargs::OptSpec::take() は **先勝ち** で最初の 1 つを消費し、残りは未消費のまま
/// `finish()` で「unexpected argument」エラーになる。テンプレート (JSONC 最上位)、
/// `instances[i]`、CLI の優先順位を後勝ちで実現するため、argv 連結後に本関数で
/// 前処理する。
///
/// 値付きオプション (`--key value`) は 2 トークンを 1 ユニット、bool フラグ
/// (`--insecure` 等) と `--key=value` は 1 トークンを 1 ユニットとして扱う。
/// 後ろから走査して、同じキーのユニットが既に採用されていれば後続をスキップする。
fn dedupe_argv_last_wins(argv: Vec<String>) -> Vec<String> {
    // 1 ユニット = キー + 値 (値付きオプション) または キー単体 (フラグ・`--key=value`)
    let mut units: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let token = &argv[i];
        if token.starts_with("--") && token.contains('=') {
            units.push(vec![token.clone()]);
            i += 1;
        } else if let Some(key) = token.strip_prefix("--") {
            if is_flag(key) || i + 1 >= argv.len() {
                units.push(vec![token.clone()]);
                i += 1;
            } else {
                units.push(vec![token.clone(), argv[i + 1].clone()]);
                i += 2;
            }
        } else {
            // 想定外の素のトークンはそのまま 1 ユニット (positional)
            units.push(vec![token.clone()]);
            i += 1;
        }
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result_rev: Vec<Vec<String>> = Vec::new();
    for unit in units.into_iter().rev() {
        let first = &unit[0];
        let key = if let Some(eq_pos) = first.find('=') {
            first[..eq_pos].to_string()
        } else {
            first.clone()
        };
        // positional は dedupe しない (key が "--" で始まらない場合)
        if key.starts_with("--") {
            if seen.insert(key) {
                result_rev.push(unit);
            }
        } else {
            result_rev.push(unit);
        }
    }
    result_rev.reverse();
    result_rev.into_iter().flatten().collect()
}

/// `CommonArgs` 用の noargs::RawArgs を構築してパースする
fn parse_common_args(program_name: &str, argv: Vec<String>) -> Result<(CommonArgs, String)> {
    // help_mode (= --help / -h 指定時) ではファイル存在チェックなどの副作用検査を skip する
    // (example 値の `libopenh264-2.6.0-mac-arm64.dylib` などはユーザー環境に存在しないため)
    let help_mode = argv.iter().any(|s| s == "--help" || s == "-h");
    let mut merged: Vec<String> = vec![program_name.to_string()];
    merged.extend(dedupe_argv_last_wins(argv));
    let mut args = noargs::RawArgs::new(merged.into_iter());
    args.metadata_mut().app_name = env!("CARGO_PKG_NAME");
    args.metadata_mut().app_description = "Sora WebRTC SFU 負荷試験ツール (CommonArgs)";

    // --help / -h 検出時に finish() がヘルプテキストを返すよう help_mode を立てる
    noargs::HELP_FLAG.take_help(&mut args);

    let instance_hatch_rate: f64 = noargs::opt("instance-hatch-rate")
        .doc("インスタンスの起動レート (秒あたりの起動数, デフォルト: 1.0)。複数インスタンスは JSONC `instances` 配列でのみ指定可能")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(1.0);

    let http_host: Option<String> = noargs::opt("http-host")
        .doc("HTTP サーバーのホストアドレス")
        .example("0.0.0.0")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let http_port: Option<u16> = noargs::opt("http-port")
        .doc("HTTP サーバーのポート番号")
        .example("8080")
        .take(&mut args)
        .present_and_then(|o| {
            o.value()
                .parse::<u16>()
                .map_err(|_| "http-port は 0-65535 の整数で指定してください")
        })?;

    let openh264: Option<String> = noargs::opt("openh264")
        .doc("OpenH264 共有ライブラリのパス")
        .example("libopenh264-2.6.0-mac-arm64.dylib")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !help_mode && !std::path::Path::new(&path).exists() {
                return Err("openh264: library file not found");
            }
            Ok(path)
        })?;

    let insecure = noargs::flag("insecure")
        .doc("TLS 証明書の検証をスキップする")
        .take(&mut args)
        .is_present();

    let client_cert: Option<String> = noargs::opt("client-cert")
        .doc("mTLS クライアント証明書ファイルのパス (PEM)")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !help_mode && !std::path::Path::new(&path).exists() {
                return Err("client-cert: file not found");
            }
            Ok(path)
        })?;

    let client_key: Option<String> = noargs::opt("client-key")
        .doc("mTLS クライアント秘密鍵ファイルのパス (PEM)")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !help_mode && !std::path::Path::new(&path).exists() {
                return Err("client-key: file not found");
            }
            Ok(path)
        })?;

    // --no-duckdb-output は単独フラグ
    let no_duckdb_output = noargs::flag("no-duckdb-output")
        .doc("DuckDB への統計情報出力を無効化する")
        .take(&mut args)
        .is_present();

    // --duckdb-output-dir は値付きオプション
    // `--no-duckdb-output` 指定時はディレクトリ存在検証をスキップする (優先されるため)
    // 明示指定されたかは `dir_presented` フラグで記録し、あとで --no-duckdb-output 併用を警告する
    let mut dir_presented = false;
    let duckdb_output_dir: String = noargs::opt("duckdb-output-dir")
        .doc("DuckDB ファイルの出力ディレクトリ (デフォルト: カレントディレクトリ)")
        .example(".")
        .take(&mut args)
        .present_and_then(|o| {
            dir_presented = true;
            let dir = o.value().to_string();
            if !help_mode && !no_duckdb_output && !std::path::Path::new(&dir).is_dir() {
                return Err("duckdb-output-dir: directory not found");
            }
            Ok(dir)
        })?
        .unwrap_or_else(|| ".".to_string());

    // --duckdb-interval は 0.1 以上 86400 以下の inclusive 範囲
    let mut interval_presented = false;
    let duckdb_interval: f64 = noargs::opt("duckdb-interval")
        .doc("DuckDB への統計書き込み間隔 (秒, デフォルト: 1.0)")
        .take(&mut args)
        .present_and_then(|o| {
            interval_presented = true;
            let v: f64 = o
                .value()
                .parse()
                .map_err(|_| "duckdb-interval は 0.1 から 86400 の範囲で指定してください")?;
            if !(0.1..=86400.0).contains(&v) {
                return Err("duckdb-interval は 0.1 から 86400 の範囲で指定してください");
            }
            Ok(v)
        })?
        .unwrap_or(1.0);

    // --no-duckdb-output と他の --duckdb-* 引数の併用検知
    // (引数の登場順を問わず --no-duckdb-output が指定されていれば警告 1 回)
    if no_duckdb_output && (dir_presented || interval_presented) {
        rtc_log_warning!("--no-duckdb-output specified, ignoring other --duckdb-* options");
    }

    let help = args.finish()?.unwrap_or_default();

    // help_mode のときは早期 return: バリデーションは skip し、戻り値の CommonArgs はダミー値とする
    // (呼び出し側はヘルプテキストのみを参照する想定)
    if !help.is_empty() {
        return Ok((
            CommonArgs {
                instance_hatch_rate,
                http_host,
                http_port,
                openh264,
                insecure,
                client_cert,
                client_key,
                duckdb_output_dir,
                duckdb_interval,
                no_duckdb_output,
            },
            help,
        ));
    }

    if instance_hatch_rate <= 0.0 {
        return Err(ErrorMessage::new("instance-hatch-rate は正の数で指定してください").into());
    }
    if http_host.is_some() != http_port.is_some() {
        return Err(
            ErrorMessage::new("--http-host と --http-port は両方指定する必要があります").into(),
        );
    }
    if client_cert.is_some() != client_key.is_some() {
        return Err(ErrorMessage::new(
            "--client-cert と --client-key は両方指定する必要があります",
        )
        .into());
    }

    Ok((
        CommonArgs {
            instance_hatch_rate,
            http_host,
            http_port,
            openh264,
            insecure,
            client_cert,
            client_key,
            duckdb_output_dir,
            duckdb_interval,
            no_duckdb_output,
        },
        help,
    ))
}

/// `InstanceArgs` 用の noargs::RawArgs を構築してパースする
fn parse_instance_args(program_name: &str, argv: Vec<String>) -> Result<(InstanceArgs, String)> {
    // help_mode (= --help / -h 指定時) ではファイル存在チェックなどの副作用検査を skip する
    let help_mode = argv.iter().any(|s| s == "--help" || s == "-h");
    let mut merged: Vec<String> = vec![program_name.to_string()];
    merged.extend(dedupe_argv_last_wins(argv));
    let mut args = noargs::RawArgs::new(merged.into_iter());
    args.metadata_mut().app_name = env!("CARGO_PKG_NAME");
    args.metadata_mut().app_description = "Sora WebRTC SFU 負荷試験ツール (InstanceArgs)";

    // --help / -h 検出時に finish() がヘルプテキストを返すよう help_mode を立てる
    noargs::HELP_FLAG.take_help(&mut args);

    let signaling_urls: Vec<String> = noargs::opt("sora-signaling-url")
        .doc("Sora の WebSocket シグナリング URL (カンマ区切りで複数指定可)")
        .example("wss://sora.example.com/signaling")
        .take(&mut args)
        .then(|o| Ok::<_, &str>(o.value().split(',').map(|s| s.trim().to_string()).collect()))?;

    let channel_id: String = noargs::opt("sora-channel-id")
        .doc("Sora のチャネル ID")
        .example("zakuro-test")
        .take(&mut args)
        .then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let role: String = noargs::opt("sora-role")
        .doc("Sora のロール (sendonly, recvonly, sendrecv)")
        .example("sendonly")
        .take(&mut args)
        .then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let client_id: Option<String> = noargs::opt("sora-client-id")
        .doc("Sora のクライアント ID")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let bundle_id: Option<String> = noargs::opt("sora-bundle-id")
        .doc("Sora のバンドル ID")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let metadata: Option<String> = noargs::opt("sora-metadata")
        .doc("Sora の connect メッセージに含めるメタデータ (JSON)")
        .example(r#"{"key":"value"}"#)
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let signaling_notify_metadata: Option<String> = noargs::opt("sora-signaling-notify-metadata")
        .doc("Sora のシグナリング通知メタデータ (JSON)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let vcs: u32 = noargs::opt("vcs")
        .doc("仮想クライアント数 (1-1000, デフォルト: 1)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(1);

    let vcs_hatch_rate: f64 = noargs::opt("vcs-hatch-rate")
        .doc("仮想クライアントの起動レート (秒あたりの起動数, デフォルト: 1.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(1.0);

    let duration: Option<f64> = noargs::opt("duration")
        .doc("仮想クライアントの接続維持秒数 (省略時は無制限)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?;

    let repeat_interval: Option<f64> = noargs::opt("repeat-interval")
        .doc("duration 経過後の再接続間隔 (秒)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?;

    let max_retry: u32 = noargs::opt("max-retry")
        .doc("接続失敗時の最大リトライ回数 (デフォルト: 0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(0);

    let retry_interval: f64 = noargs::opt("retry-interval")
        .doc("リトライ間隔 (秒, デフォルト: 60.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(60.0);

    let no_video_device = noargs::flag("no-video-device")
        .doc("映像デバイスを使用しない")
        .take(&mut args)
        .is_present();

    let no_audio_device = noargs::flag("no-audio-device")
        .doc("音声デバイスを使用しない")
        .take(&mut args)
        .is_present();

    let video_input_device: Option<String> = noargs::opt("video-input-device")
        .doc("映像入力デバイス名または ID")
        .example("FaceTime HD Camera")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let resolution: (i32, i32) = noargs::opt("resolution")
        .doc("映像解像度 (QVGA/VGA/HD/FHD/4K または WxH, デフォルト: VGA)")
        .take(&mut args)
        .present_and_then(|o| parse_resolution(o.value()))?
        .unwrap_or((640, 480));

    let framerate: u32 = noargs::opt("framerate")
        .doc("映像フレームレート (1-60, デフォルト: 30)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(30);

    let sandstorm = noargs::flag("sandstorm")
        .doc("砂嵐映像を生成する")
        .take(&mut args)
        .is_present();

    let input_y4m: Option<String> = noargs::opt("input-y4m")
        .doc("Y4M ファイルを映像入力として再生する")
        .example("video.y4m")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !help_mode && !std::path::Path::new(&path).exists() {
                return Err("input-y4m: file not found");
            }
            Ok(path)
        })?;

    let input_mp4: Option<String> = noargs::opt("input-mp4")
        .doc("MP4 ファイルからエンコード済み映像をパススルー送信する")
        .example("video.mp4")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !help_mode && !std::path::Path::new(&path).exists() {
                return Err("input-mp4: file not found");
            }
            Ok(path)
        })?;

    let input_wav: Option<String> = noargs::opt("input-wav")
        .doc("WAV ファイル (PCM 16bit) を音声入力としてループ再生する")
        .example("audio.wav")
        .take(&mut args)
        .present_and_then(|o| {
            let path = o.value().to_string();
            if !help_mode && !std::path::Path::new(&path).exists() {
                return Err("input-wav: file not found");
            }
            Ok(path)
        })?;

    let video_codec_type: Option<String> = noargs::opt("sora-video-codec-type")
        .doc("映像コーデック (vp8/vp9/av1/h264/h265)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "vp8" | "vp9" | "av1" | "h264" | "h265" => Ok(o.value().to_string()),
            _ => Err("sora-video-codec-type は vp8/vp9/av1/h264/h265 で指定してください"),
        })?;

    let video_bit_rate: Option<u32> = noargs::opt("sora-video-bit-rate")
        .doc("映像ビットレート (kbps)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?;

    let audio: bool = noargs::opt("sora-audio")
        .doc("音声の有効/無効 (true/false, デフォルト: true)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-audio は true または false で指定してください"),
        })?
        .unwrap_or(true);

    let audio_codec_type: Option<String> = noargs::opt("sora-audio-codec-type")
        .doc("音声コーデック (opus)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "opus" => Ok(o.value().to_string()),
            _ => Err("sora-audio-codec-type は opus で指定してください"),
        })?;

    let audio_bit_rate: Option<u32> = noargs::opt("sora-audio-bit-rate")
        .doc("音声ビットレート (kbps)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?;

    let data_channels: Option<String> = noargs::opt("sora-data-channels")
        .doc("DataChannel メッセージング設定 (JSON 文字列)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let data_channel_signaling: Option<bool> = noargs::opt("sora-data-channel-signaling")
        .doc("DataChannel 経由でシグナリングを行う (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-data-channel-signaling は true または false で指定してください"),
        })?;

    let ignore_disconnect_websocket: Option<bool> = noargs::opt("sora-ignore-disconnect-websocket")
        .doc("DataChannel 使用時に WebSocket 切断を無視する (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-ignore-disconnect-websocket は true または false で指定してください"),
        })?;

    let disconnect_wait_timeout: Option<f64> = noargs::opt("sora-disconnect-wait-timeout")
        .doc("切断待ちタイムアウト (秒, デフォルト: 5.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?;

    let simulcast: Option<bool> = noargs::opt("sora-simulcast")
        .doc("サイマルキャストの有効/無効 (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-simulcast は true または false で指定してください"),
        })?;

    let simulcast_request_rid: Option<String> = noargs::opt("sora-simulcast-request-rid")
        .doc("サイマルキャストで受信する rid (r0/r1/r2)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let spotlight: Option<bool> = noargs::opt("sora-spotlight")
        .doc("スポットライトの有効/無効 (true/false)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("sora-spotlight は true または false で指定してください"),
        })?;

    let spotlight_focus_rid: Option<String> = noargs::opt("sora-spotlight-focus-rid")
        .doc("スポットライトでフォーカス時の rid (r0/r1/r2)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let spotlight_unfocus_rid: Option<String> = noargs::opt("sora-spotlight-unfocus-rid")
        .doc("スポットライトでアンフォーカス時の rid (r0/r1/r2)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let scenario: Option<ScenarioType> = noargs::opt("scenario")
        .doc("シナリオ種別 (reconnect)")
        .take(&mut args)
        .present_and_then(|o| {
            ScenarioType::parse(o.value()).ok_or("scenario は reconnect で指定してください")
        })?;

    let help = args.finish()?.unwrap_or_default();

    let role = Role::parse(&role)?;

    // help_mode のときは早期 return: バリデーションは skip し、戻り値の InstanceArgs はダミー値とする
    // (呼び出し側はヘルプテキストのみを参照する想定)
    if !help.is_empty() {
        return Ok((
            InstanceArgs {
                signaling_urls,
                channel_id,
                role,
                client_id,
                bundle_id,
                metadata,
                signaling_notify_metadata,
                vcs,
                vcs_hatch_rate,
                duration,
                repeat_interval,
                max_retry,
                retry_interval,
                no_video_device,
                no_audio_device,
                video_input_device,
                resolution,
                framerate,
                sandstorm,
                input_y4m,
                input_mp4,
                input_wav,
                video_codec_type,
                video_bit_rate,
                audio,
                audio_codec_type,
                audio_bit_rate,
                data_channels,
                data_channel_signaling,
                ignore_disconnect_websocket,
                disconnect_wait_timeout,
                simulcast,
                simulcast_request_rid,
                spotlight,
                spotlight_focus_rid,
                spotlight_unfocus_rid,
                scenario,
            },
            help,
        ));
    }

    // バリデーション (per-instance)
    if vcs == 0 || vcs > 1000 {
        return Err(ErrorMessage::new("vcs は 1 から 1000 の範囲で指定してください").into());
    }
    if vcs_hatch_rate <= 0.0 {
        return Err(ErrorMessage::new("vcs-hatch-rate は正の数で指定してください").into());
    }
    if framerate == 0 || framerate > 60 {
        return Err(ErrorMessage::new("framerate は 1 から 60 の範囲で指定してください").into());
    }
    if sandstorm && input_y4m.is_some() {
        return Err(ErrorMessage::new("--sandstorm と --input-y4m は同時に指定できません").into());
    }
    if video_input_device.is_some() && input_y4m.is_some() {
        return Err(ErrorMessage::new(
            "--video-input-device と --input-y4m は同時に指定できません",
        )
        .into());
    }
    if video_input_device.is_some() && sandstorm {
        return Err(ErrorMessage::new(
            "--video-input-device と --sandstorm は同時に指定できません",
        )
        .into());
    }
    if input_mp4.is_some() && video_input_device.is_some() {
        return Err(ErrorMessage::new(
            "--input-mp4 と --video-input-device は同時に指定できません",
        )
        .into());
    }
    if input_mp4.is_some() && input_y4m.is_some() {
        return Err(ErrorMessage::new("--input-mp4 と --input-y4m は同時に指定できません").into());
    }
    if input_mp4.is_some() && sandstorm {
        return Err(ErrorMessage::new("--input-mp4 と --sandstorm は同時に指定できません").into());
    }
    if input_wav.is_some() && no_audio_device {
        return Err(
            ErrorMessage::new("--input-wav と --no-audio-device は同時に指定できません").into(),
        );
    }
    if input_wav.is_some() && !audio {
        return Err(
            ErrorMessage::new("--input-wav 使用時は --sora-audio=false を指定できません").into(),
        );
    }
    if input_mp4.is_some() && video_codec_type.is_none() {
        return Err(ErrorMessage::new(
            "--input-mp4 使用時は --sora-video-codec-type の指定が必須です",
        )
        .into());
    }
    if input_mp4.is_some() && video_bit_rate.is_none() {
        return Err(ErrorMessage::new(
            "--input-mp4 使用時は --sora-video-bit-rate の指定が必須です",
        )
        .into());
    }

    Ok((
        InstanceArgs {
            signaling_urls,
            channel_id,
            role,
            client_id,
            bundle_id,
            metadata,
            signaling_notify_metadata,
            vcs,
            vcs_hatch_rate,
            duration,
            repeat_interval,
            max_retry,
            retry_interval,
            no_video_device,
            no_audio_device,
            video_input_device,
            resolution,
            framerate,
            sandstorm,
            input_y4m,
            input_mp4,
            input_wav,
            video_codec_type,
            video_bit_rate,
            audio,
            audio_codec_type,
            audio_bit_rate,
            data_channels,
            data_channel_signaling,
            ignore_disconnect_websocket,
            disconnect_wait_timeout,
            simulcast,
            simulcast_request_rid,
            spotlight,
            spotlight_focus_rid,
            spotlight_unfocus_rid,
            scenario,
        },
        help,
    ))
}

/// 純粋関数版のメインパース (単体テスト用、env IO を持たない)
fn parse_args_from_argv(
    program_name: &str,
    common_argv: Vec<String>,
    common_cli_argv: Vec<String>,
    instance_argvs: Vec<Vec<String>>,
    instance_cli_argv: Vec<String>,
) -> Result<(CommonArgs, Vec<InstanceArgs>)> {
    // CommonArgs: テンプレート (JSONC 最上位) + CLI 由来 を後勝ち連結
    let mut common_merged: Vec<String> = Vec::new();
    common_merged.extend(common_argv);
    common_merged.extend(common_cli_argv);
    let (common, _help_common) = parse_common_args(program_name, common_merged)?;

    // InstanceArgs[i]: テンプレート + instances[i] (parse_jsonc_config で焼き込み済み) + CLI 由来 を後勝ち連結
    let mut instances: Vec<InstanceArgs> = Vec::new();
    for argv in instance_argvs.into_iter() {
        let mut merged: Vec<String> = argv;
        merged.extend(instance_cli_argv.clone());
        let (instance, _help_instance) = parse_instance_args(program_name, merged)?;
        instances.push(instance);
    }

    Ok((common, instances))
}

/// プロセス入口の引数パース
///
/// 1. env から `--config` / `--help` / `--version` を pre-parse
/// 2. JSONC をロードして `JsoncConfig` を構築
/// 3. 残り CLI 引数を CommonArgs / InstanceArgs 用に分割
/// 4. `parse_args_from_argv()` に流す
pub(crate) fn parse_args() -> Result<(CommonArgs, Vec<InstanceArgs>, Option<String>)> {
    let env_argv: Vec<String> = std::env::args().collect();
    let program_name = env_argv.first().cloned().unwrap_or_default();

    // pre-parse: --version / --help を 1 度走査して検出する
    let mut want_version = false;
    let mut want_help = false;
    for token in env_argv.iter().skip(1) {
        if token == "--version" {
            want_version = true;
        } else if token == "--help" || token == "-h" {
            want_help = true;
        }
    }
    if want_version {
        rtc_log_info!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    // pre-parse: --config を取り出して env から除外する (`split_cli_argv()` に渡さない)
    let mut config_path: Option<String> = None;
    let mut cli_after_config: Vec<String> = Vec::new();
    let mut iter = env_argv.iter().skip(1).peekable();
    while let Some(token) = iter.next() {
        if token == "--config" {
            if let Some(value) = iter.next() {
                config_path = Some(value.clone());
            } else {
                return Err(ErrorMessage::new("--config requires a value").into());
            }
            continue;
        }
        if let Some(value) = token.strip_prefix("--config=") {
            config_path = Some(value.to_string());
            continue;
        }
        // --help / -h / --version は help/version 処理で消費したので argv から除外する
        if token == "--help" || token == "-h" || token == "--version" {
            continue;
        }
        cli_after_config.push(token.clone());
    }

    // JSONC をロード
    let JsoncConfig {
        common_argv,
        instance_argvs,
    } = if let Some(ref path) = config_path {
        load_jsonc_config(path)?
    } else {
        JsoncConfig {
            common_argv: Vec::new(),
            instance_argvs: vec![Vec::new()],
        }
    };

    // --help モード時は CommonArgs / InstanceArgs それぞれをヘルプ生成用 argv で構築する。
    // parse_*_args 内で noargs::HELP_FLAG.take_help() が help_mode を立て、finish() が
    // ヘルプテキストを返す。戻り値の CommonArgs / InstanceArgs は使わず help テキストのみ表示する。
    if want_help {
        let (_common, common_help) = parse_common_args(&program_name, vec!["--help".to_string()])?;
        let (_instance, instance_help) =
            parse_instance_args(&program_name, vec!["--help".to_string()])?;
        print!("{common_help}");
        print!("{instance_help}");
        std::process::exit(0);
    }

    // CLI 引数を CommonArgs / InstanceArgs 用に分割
    let (common_cli_argv, instance_cli_argv) = split_cli_argv(cli_after_config);

    let (common, instances) = parse_args_from_argv(
        &program_name,
        common_argv,
        common_cli_argv,
        instance_argvs,
        instance_cli_argv,
    )?;

    Ok((common, instances, config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小限の有効な instance を組み立てるためのテンプレート argv 末尾用
    fn minimal_sora_argv() -> Vec<String> {
        vec![
            "--sora-signaling-url".into(),
            "wss://example.com/".into(),
            "--sora-channel-id".into(),
            "ch".into(),
            "--sora-role".into(),
            "sendonly".into(),
        ]
    }

    #[test]
    fn parse_jsonc_config_without_instances_uses_template_only() {
        // instances キーが無い JSONC は 1 インスタンス分のテンプレートとして扱う (後方互換)
        let content = r#"{
            "vcs": 5,
            "sora": {
                "signaling-url": "wss://example.com/",
                "channel-id": "ch",
                "role": "sendonly"
            }
        }"#;
        let cfg = parse_jsonc_config(content).expect("有効な JSONC のパースに失敗してはならない");
        assert!(cfg.common_argv.is_empty(), "common_argv は空であるべき");
        assert_eq!(
            cfg.instance_argvs.len(),
            1,
            "instances 無しは 1 件扱いになるべき"
        );
        assert!(
            cfg.instance_argvs[0]
                .iter()
                .any(|s| s == "--sora-signaling-url"),
            "sora.signaling-url が --sora-signaling-url に展開されていない"
        );
    }

    #[test]
    fn parse_jsonc_config_with_two_instances_carries_template_to_each() {
        // テンプレートで vcs=5、instances[1] で vcs=10 を上書き
        let content = r#"{
            "instance-hatch-rate": 2.0,
            "vcs": 5,
            "instances": [
                { "sora": { "signaling-url": "wss://a/", "channel-id": "a", "role": "sendonly" } },
                { "vcs": 10, "sora": { "signaling-url": "wss://b/", "channel-id": "b", "role": "recvonly" } }
            ]
        }"#;
        let cfg = parse_jsonc_config(content).expect("有効な JSONC のパースに失敗してはならない");
        assert_eq!(
            cfg.common_argv,
            vec!["--instance-hatch-rate".to_string(), "2.0".to_string()],
            "common_argv に instance-hatch-rate が反映されていない"
        );
        assert_eq!(cfg.instance_argvs.len(), 2, "instance 数が 2 でない");
        // 1 つ目はテンプレートの vcs=5 を継承し、独自の上書きは無い
        assert!(
            cfg.instance_argvs[0]
                .windows(2)
                .any(|w| w[0] == "--vcs" && w[1] == "5"),
            "instances[0] のテンプレート vcs=5 が継承されていない"
        );
        // 2 つ目はテンプレート vcs=5 の後に vcs=10 が追加され、noargs の後勝ちで 10 になる
        let last_vcs = cfg.instance_argvs[1]
            .windows(2)
            .filter(|w| w[0] == "--vcs")
            .map(|w| w[1].clone())
            .next_back()
            .expect("instances[1] に vcs が無い");
        assert_eq!(last_vcs, "10", "instances[1] の vcs 上書きが効いていない");
    }

    #[test]
    fn parse_jsonc_config_with_signaling_url_array_joins_with_comma() {
        // sora.signaling-url が配列の場合はカンマ結合
        let content = r#"{
            "instances": [
                { "sora": { "signaling-url": ["wss://a/", "wss://b/"], "channel-id": "c", "role": "sendonly" } }
            ]
        }"#;
        let cfg = parse_jsonc_config(content).expect("配列の signaling-url のパースに失敗");
        let url = cfg.instance_argvs[0]
            .windows(2)
            .find(|w| w[0] == "--sora-signaling-url")
            .map(|w| w[1].clone())
            .expect("--sora-signaling-url が無い");
        assert_eq!(url, "wss://a/,wss://b/", "配列がカンマ結合されていない");
    }

    #[test]
    fn parse_jsonc_config_rejects_common_key_inside_instance() {
        // instances[i] 内に CommonArgs キーがあるとエラー
        let content = r#"{
            "instances": [
                { "http-host": "0.0.0.0", "sora": { "signaling-url": "wss://a/", "channel-id": "c", "role": "sendonly" } }
            ]
        }"#;
        let err = parse_jsonc_config(content)
            .expect_err("CommonArgs キーが instances 内にあるのを許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("http-host"),
            "エラーメッセージに 'http-host' が含まれていない: {msg}"
        );
    }

    #[test]
    fn parse_jsonc_config_rejects_flat_sora_key_inside_instance() {
        // instances[i] 直下に `sora-` プレフィックスのフラットキーがあるとエラー
        let content = r#"{
            "instances": [
                { "sora-channel-id": "c", "sora": { "signaling-url": "wss://a/", "channel-id": "c", "role": "sendonly" } }
            ]
        }"#;
        let err = parse_jsonc_config(content)
            .expect_err("instances 内のフラット sora キーを許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("sora-channel-id"),
            "エラーメッセージに 'sora-channel-id' が含まれていない: {msg}"
        );
    }

    #[test]
    fn parse_jsonc_config_rejects_empty_instances() {
        // instances が空配列ならエラー
        let content = r#"{
            "instances": []
        }"#;
        let err = parse_jsonc_config(content).expect_err("空の instances を許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("1 から 64"),
            "エラーメッセージに範囲指示が含まれていない: {msg}"
        );
    }

    #[test]
    fn parse_jsonc_config_rejects_non_object_sora() {
        // sora キーが Object 以外ならエラー
        let content = r#"{
            "instances": [
                { "sora": "wss://example.com/" }
            ]
        }"#;
        let err = parse_jsonc_config(content).expect_err("Object 以外の sora を許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("'sora' must be a JSON object"),
            "エラーメッセージが期待形式でない: {msg}"
        );
    }

    #[test]
    fn parse_jsonc_config_rejects_config_key_recursion() {
        // config キーが JSONC 内にあるとエラー (再帰ロード防止)
        let content = r#"{
            "config": "other.jsonc",
            "instances": [
                { "sora": { "signaling-url": "wss://a/", "channel-id": "c", "role": "sendonly" } }
            ]
        }"#;
        let err = parse_jsonc_config(content).expect_err("config キーを許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("config"),
            "エラーメッセージに config の語が無い: {msg}"
        );
    }

    #[test]
    fn parse_jsonc_config_boolean_expands_to_flag_or_omit() {
        // boolean true はフラグ単体に、false はキー無視に展開される
        let content = r#"{
            "insecure": true,
            "instances": [
                { "no-video-device": true, "no-audio-device": false, "sora": { "signaling-url": "wss://a/", "channel-id": "c", "role": "sendonly" } }
            ]
        }"#;
        let cfg = parse_jsonc_config(content).expect("有効な JSONC のパースに失敗");
        assert!(
            cfg.common_argv.iter().any(|s| s == "--insecure"),
            "insecure=true がフラグ単体に展開されていない"
        );
        assert!(
            cfg.instance_argvs[0]
                .iter()
                .any(|s| s == "--no-video-device"),
            "no-video-device=true がフラグ単体に展開されていない"
        );
        assert!(
            !cfg.instance_argvs[0]
                .iter()
                .any(|s| s == "--no-audio-device"),
            "no-audio-device=false が誤って展開されている"
        );
    }

    #[test]
    fn parse_args_from_argv_three_layer_override_works() {
        // テンプレート (instance_argvs[0] 先頭) で vcs=10
        // CLI 由来 (instance_cli_argv 末尾) で vcs=20
        // 期待: 連結順序 [program, ...instance_argvs[0], ...instance_cli_argv] で noargs 後勝ち → vcs=20
        let common_argv: Vec<String> = vec!["--instance-hatch-rate".into(), "1.0".into()];
        let common_cli_argv: Vec<String> = Vec::new();
        let mut tpl: Vec<String> = minimal_sora_argv();
        tpl.extend(["--vcs".into(), "10".into()]);
        let instance_argvs = vec![tpl];
        let instance_cli_argv: Vec<String> = vec!["--vcs".into(), "20".into()];

        let (_common, instances) = parse_args_from_argv(
            "zakuro",
            common_argv,
            common_cli_argv,
            instance_argvs,
            instance_cli_argv,
        )
        .expect("有効な argv のパースに失敗してはならない");
        assert_eq!(instances.len(), 1, "instance 数が 1 でない");
        assert_eq!(
            instances[0].vcs, 20,
            "CLI 由来の vcs=20 がテンプレートの vcs=10 を上書きできていない"
        );
    }

    #[test]
    fn parse_args_from_argv_rejects_non_positive_instance_hatch_rate() {
        // instance-hatch-rate が 0 以下ならエラー
        let common_argv: Vec<String> = vec!["--instance-hatch-rate".into(), "0".into()];
        let instance_argvs = vec![minimal_sora_argv()];
        let err = parse_args_from_argv(
            "zakuro",
            common_argv,
            Vec::new(),
            instance_argvs,
            Vec::new(),
        )
        .expect_err("instance-hatch-rate=0 を許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("instance-hatch-rate"),
            "エラーメッセージに instance-hatch-rate が含まれていない: {msg}"
        );
    }

    #[test]
    fn parse_args_from_argv_rejects_invalid_vcs() {
        // 各 instance の vcs が 0 または 1001 以上ならエラー
        let mut tpl = minimal_sora_argv();
        tpl.extend(["--vcs".into(), "0".into()]);
        let err = parse_args_from_argv("zakuro", Vec::new(), Vec::new(), vec![tpl], Vec::new())
            .expect_err("vcs=0 を許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("vcs"),
            "エラーメッセージに vcs が含まれていない: {msg}"
        );
    }

    #[test]
    fn parse_args_from_argv_rejects_non_positive_vcs_hatch_rate() {
        // 各 instance の vcs_hatch_rate が 0 以下ならエラー
        let mut tpl = minimal_sora_argv();
        tpl.extend(["--vcs-hatch-rate".into(), "0".into()]);
        let err = parse_args_from_argv("zakuro", Vec::new(), Vec::new(), vec![tpl], Vec::new())
            .expect_err("vcs-hatch-rate=0 を許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("vcs-hatch-rate"),
            "エラーメッセージに vcs-hatch-rate が含まれていない: {msg}"
        );
    }

    // ---- DuckDB 系引数のテスト ----

    /// DuckDB 引数を含む argv を CommonArgs 用に組み立てる
    /// (instance 側は最小限の sora argv を別途用意する)
    fn common_duckdb_argv(dir: &str, interval: &str, no_output: bool) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if !dir.is_empty() {
            v.push("--duckdb-output-dir".into());
            v.push(dir.into());
        }
        if !interval.is_empty() {
            v.push("--duckdb-interval".into());
            v.push(interval.into());
        }
        if no_output {
            v.push("--no-duckdb-output".into());
        }
        v
    }

    #[test]
    fn duckdb_output_dir_defaults_to_current_directory() {
        // --duckdb-output-dir 未指定時はカレントディレクトリ "." になる
        let (common, _instances) = parse_args_from_argv(
            "zakuro",
            Vec::new(),
            Vec::new(),
            vec![minimal_sora_argv()],
            Vec::new(),
        )
        .expect("有効な argv のパースに失敗してはならない");
        assert_eq!(
            common.duckdb_output_dir, ".",
            "未指定時はカレントディレクトリになるべき"
        );
    }

    #[test]
    fn duckdb_interval_defaults_to_one_second() {
        // --duckdb-interval 未指定時は 1.0 になる
        let (common, _instances) = parse_args_from_argv(
            "zakuro",
            Vec::new(),
            Vec::new(),
            vec![minimal_sora_argv()],
            Vec::new(),
        )
        .expect("有効な argv のパースに失敗してはならない");
        assert_eq!(common.duckdb_interval, 1.0, "未指定時は 1.0 になるべき");
    }

    #[test]
    fn duckdb_no_output_flag_parses() {
        // --no-duckdb-output 単独フラグのパース
        let (common, _instances) = parse_args_from_argv(
            "zakuro",
            common_duckdb_argv("", "", true),
            Vec::new(),
            vec![minimal_sora_argv()],
            Vec::new(),
        )
        .expect("有効な argv のパースに失敗してはならない");
        assert!(
            common.no_duckdb_output,
            "no_duckdb_output が true になるべき"
        );
    }

    #[test]
    fn duckdb_interval_boundary_values_accepted() {
        // 0.1 と 86400 は inclusive 範囲なので受け入れる
        let dir = tempfile::TempDir::new()
            .expect("一時ディレクトリの作成に失敗")
            .keep();
        let dir_str = dir.to_string_lossy().to_string();
        for &val in &["0.1", "86400"] {
            let (common, _instances) = parse_args_from_argv(
                "zakuro",
                common_duckdb_argv(&dir_str, val, false),
                Vec::new(),
                vec![minimal_sora_argv()],
                Vec::new(),
            )
            .expect(&format!("duckdb-interval={val} は受け入れられるべき"));
            assert_eq!(
                common.duckdb_interval.to_string(),
                val,
                "duckdb-interval={val} が正しくパースされていない"
            );
        }
    }

    #[test]
    fn duckdb_interval_out_of_range_rejected() {
        // 0.099 と 86400.1 は範囲外なのでエラー
        let dir = tempfile::TempDir::new()
            .expect("一時ディレクトリの作成に失敗")
            .keep();
        let dir_str = dir.to_string_lossy().to_string();
        for &val in &["0.099", "86400.1"] {
            let err = parse_args_from_argv(
                "zakuro",
                common_duckdb_argv(&dir_str, val, false),
                Vec::new(),
                vec![minimal_sora_argv()],
                Vec::new(),
            )
            .expect_err(&format!("duckdb-interval={val} は拒否されるべき"));
            let msg = format!("{err}");
            assert!(
                msg.contains("duckdb-interval"),
                "エラーメッセージに duckdb-interval が含まれていない: {msg}"
            );
        }
    }

    #[test]
    fn duckdb_output_dir_missing_rejected() {
        // 存在しないディレクトリはエラー
        let dir = tempfile::TempDir::new()
            .expect("一時ディレクトリの作成に失敗")
            .keep();
        let missing = dir.join("does-not-exist");
        let missing_str = missing.to_string_lossy().to_string();
        let err = parse_args_from_argv(
            "zakuro",
            common_duckdb_argv(&missing_str, "", false),
            Vec::new(),
            vec![minimal_sora_argv()],
            Vec::new(),
        )
        .expect_err("存在しないディレクトリを許容してはならない");
        let msg = format!("{err}");
        assert!(
            msg.contains("duckdb-output-dir"),
            "エラーメッセージに duckdb-output-dir が含まれていない: {msg}"
        );
    }

    #[test]
    fn duckdb_no_output_skips_directory_validation() {
        // --no-duckdb-output 指定時はディレクトリ存在検証をスキップする
        let dir = tempfile::TempDir::new()
            .expect("一時ディレクトリの作成に失敗")
            .keep();
        let missing = dir.join("does-not-exist");
        let missing_str = missing.to_string_lossy().to_string();
        // 存在しないディレクトリ + --no-duckdb-output はエラーにならない
        let (common, _instances) = parse_args_from_argv(
            "zakuro",
            common_duckdb_argv(&missing_str, "", true),
            Vec::new(),
            vec![minimal_sora_argv()],
            Vec::new(),
        )
        .expect("--no-duckdb-output 時はディレクトリ検証をスキップするべき");
        assert!(
            common.no_duckdb_output,
            "no_duckdb_output が true になるべき"
        );
    }

    #[test]
    fn is_common_key_includes_duckdb_keys() {
        // is_common_key に DuckDB 系 3 キーが含まれている
        assert!(is_common_key("duckdb-output-dir"));
        assert!(is_common_key("duckdb-interval"));
        assert!(is_common_key("no-duckdb-output"));
    }

    #[test]
    fn is_flag_includes_no_duckdb_output() {
        // is_flag に --no-duckdb-output が含まれている
        assert!(is_flag("no-duckdb-output"));
    }

    #[test]
    fn split_cli_argv_routes_duckdb_keys_to_common() {
        // --duckdb-output-dir (値付き) と --no-duckdb-output (フラグ) が common 側に振り分けられる
        let dir = tempfile::TempDir::new()
            .expect("一時ディレクトリの作成に失敗")
            .keep();
        let dir_str = dir.to_string_lossy().to_string();
        let cli: Vec<String> = vec![
            "--duckdb-output-dir".into(),
            dir_str.clone(),
            "--no-duckdb-output".into(),
            "--vcs".into(),
            "5".into(),
        ];
        let (common, instance) = split_cli_argv(cli);
        assert!(
            common
                .windows(2)
                .any(|w| w[0] == "--duckdb-output-dir" && w[1] == dir_str),
            "common 側に --duckdb-output-dir が振り分けられていない"
        );
        assert!(
            common.iter().any(|s| s == "--no-duckdb-output"),
            "common 側に --no-duckdb-output が振り分けられていない"
        );
        assert!(
            instance.windows(2).any(|w| w[0] == "--vcs" && w[1] == "5"),
            "instance 側に --vcs が振り分けられていない"
        );
    }

    #[test]
    fn dedupe_argv_last_wins_duckdb_output_dir() {
        // --duckdb-output-dir が複数登場した場合は後勝ち
        let dir1 = tempfile::TempDir::new()
            .expect("一時ディレクトリの作成に失敗")
            .keep();
        let dir2 = tempfile::TempDir::new()
            .expect("一時ディレクトリの作成に失敗")
            .keep();
        let dir1_str = dir1.to_string_lossy().to_string();
        let dir2_str = dir2.to_string_lossy().to_string();
        let argv: Vec<String> = vec![
            "--duckdb-output-dir".into(),
            dir1_str.clone(),
            "--duckdb-output-dir".into(),
            dir2_str.clone(),
        ];
        let deduped = dedupe_argv_last_wins(argv);
        // 後勝ちで dir2 だけ残る
        let dirs: Vec<&String> = deduped
            .windows(2)
            .filter(|w| w[0] == "--duckdb-output-dir")
            .map(|w| &w[1])
            .collect();
        assert_eq!(dirs.len(), 1, "重複除去後に 1 件だけ残るべき");
        assert_eq!(dirs[0], &dir2_str, "後勝ちで dir2 が残るべき");
    }
}
