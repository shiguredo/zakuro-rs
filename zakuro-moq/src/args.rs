//! CLI 引数と JSONC 設定ファイルの解釈

use noargs::RawArgs;
use nojson::{JsonValueKind, RawJson, RawJsonValue};

use crate::error::{ErrorMessage, Result};
use crate::logging::LogLevel;

/// `--tracks` で object レートを省略したときの既定値 (objects/sec)
const DEFAULT_OBJECT_RATE: f64 = 30.0;

/// `--tracks` で object サイズを省略したときの既定値 (バイト)
const DEFAULT_OBJECT_SIZE: usize = 1000;

/// `--tracks` を省略したときのトラック名
const DEFAULT_TRACK_NAME: &str = "video";

/// object レートの許容範囲 (objects/sec)
///
/// `Duration::from_secs_f64(1.0 / rate)` を `tokio::time::interval` に渡すため、極端な値は
/// 期間 0 (panic) や表現不能な Duration (panic) を生む。負荷試験で意味のある範囲に制限する。
const OBJECT_RATE_RANGE: std::ops::RangeInclusive<f64> = 0.001..=1_000_000.0;

/// object サイズの上限 (バイト)
const MAX_OBJECT_SIZE: usize = 1024 * 1024;

/// Full Track Name の最大バイト長
///
/// draft-ietf-moq-transport-22 §8.7 (Track Namespace Structure) が Track Namespace と
/// Full Track Name の合計を 4096 バイト以下と定めている。
const MAX_TRACK_NAME_LENGTH: usize = 4096;

/// 仮想クライアント識別子の suffix (`-<instance>-<vc>`) のバイト数
const TRACK_NAME_SUFFIX_BUDGET: usize = 24;

/// 起動レート (毎秒) の許容範囲
///
/// 周期は `Duration::from_secs_f64(1.0 / rate)` で求めるため、極端に小さい値は `Duration`
/// で表現できない秒数になり panic する。負荷試験で意味のある範囲に制限する。
const HATCH_RATE_RANGE: std::ops::RangeInclusive<f64> = 1e-6..=1e6;

/// 時間指定 (秒) の上限
///
/// `--duration` / `--repeat-interval` / `--retry-interval` は `Duration::from_secs_f64` に
/// 渡すため、`inf` や表現不能な大きさは panic する。実質無制限とみなせる 1e9 秒 (約 31 年)
/// を上限にする。
const MAX_DURATION_SECONDS: f64 = 1e9;

/// トラック 1 本の設定
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrackSpec {
    /// トラック名の接頭辞 (実行時に `<名前>-<instance>-<vc>` とする)
    pub(crate) name: String,
    /// 1 秒あたりに送る object 数
    pub(crate) object_rate: f64,
    /// 1 object の payload サイズ (バイト)
    pub(crate) object_size: usize,
}

/// プロセス全体で共有する設定
#[derive(Debug, Clone)]
pub(crate) struct CommonArgs {
    /// インスタンスの起動レート (毎秒)
    pub(crate) instance_hatch_rate: f64,
    /// HTTP API のバインドホスト
    pub(crate) http_host: Option<String>,
    /// HTTP API のバインドポート
    pub(crate) http_port: Option<u16>,
    /// TLS 証明書の検証をスキップする
    pub(crate) insecure: bool,
    /// ログレベル
    pub(crate) log_level: LogLevel,
}

/// インスタンス 1 つ分の設定
#[derive(Debug, Clone)]
pub(crate) struct InstanceArgs {
    /// MOQ relay の URL (`moqt://host:port`)
    pub(crate) url: String,
    /// Track Namespace
    pub(crate) namespace: String,
    /// publish するトラック (1 仮想クライアントが全トラックを publish する)
    pub(crate) tracks: Vec<TrackSpec>,
    /// subscribe するトラック名 (Full Track Name は `<名前>-<instance>-<vc>`)
    pub(crate) subscribe_tracks: Vec<String>,
    /// 受信 payload が zakuro-moq の publisher のパターンと一致するかを検査する
    pub(crate) verify_payload: bool,
    /// relay の CA 証明書 (PEM)
    pub(crate) ca_cert: Option<String>,
    /// 仮想クライアント数
    pub(crate) vcs: u32,
    /// 仮想クライアントの起動レート (毎秒)
    pub(crate) vcs_hatch_rate: f64,
    /// 接続維持秒数 (未指定なら無制限)
    pub(crate) duration: Option<f64>,
    /// duration 経過後の再接続間隔 (秒)
    pub(crate) repeat_interval: Option<f64>,
    /// 接続失敗時の最大リトライ回数
    pub(crate) max_retry: u32,
    /// リトライ間隔 (秒)
    pub(crate) retry_interval: f64,
}

/// JSONC から取り出した argv 群
struct JsoncConfig {
    common_argv: Vec<String>,
    instance_argvs: Vec<Vec<String>>,
}

/// `CommonArgs` に属するキーかどうか
fn is_common_key(key: &str) -> bool {
    matches!(
        key,
        "instance-hatch-rate" | "http-host" | "http-port" | "insecure" | "log-level"
    )
}

/// 値を伴わない bool フラグのキー (CLI argv 分割で次のトークンを値として取らない)
fn is_flag(key: &str) -> bool {
    matches!(key, "insecure" | "verify-payload")
}

/// 引数を解釈する
///
/// 戻り値は `(共通設定, インスタンス設定, 警告メッセージ)`。
/// 警告はログ初期化後に出力するため、呼び出し側へ返す。
pub(crate) fn parse_args() -> Result<(CommonArgs, Vec<InstanceArgs>, Vec<String>)> {
    let env_argv: Vec<String> = std::env::args().collect();
    let program_name = env_argv.first().cloned().unwrap_or_default();

    // --version / --help は他の検証より先に処理する
    for token in env_argv.iter().skip(1) {
        if token == "--version" {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
    }
    if env_argv.iter().skip(1).any(|t| t == "--help" || t == "-h") {
        // 共通とインスタンスのヘルプを続けて表示する
        let help_argv = vec!["--help".to_string()];
        let (_common, common_help) =
            parse_common_args(&program_name, help_argv.clone(), &mut Vec::new())?;
        let (_instance, instance_help) =
            parse_instance_args(&program_name, help_argv, &mut Vec::new())?;
        print!("{common_help}");
        print!("{instance_help}");
        std::process::exit(0);
    }

    // --config を取り出して argv から除外する
    let mut config_path: Option<String> = None;
    let mut cli_after_config: Vec<String> = Vec::new();
    let mut iter = env_argv.iter().skip(1).peekable();
    while let Some(token) = iter.next() {
        if token == "--config" {
            match iter.next() {
                Some(value) => config_path = Some(value.clone()),
                None => return Err(ErrorMessage::new("--config requires a value").into()),
            }
            continue;
        }
        if let Some(value) = token.strip_prefix("--config=") {
            config_path = Some(value.to_string());
            continue;
        }
        if token == "--help" || token == "-h" || token == "--version" {
            continue;
        }
        cli_after_config.push(token.clone());
    }

    // JSONC をロード (未指定ならテンプレート無しの 1 インスタンス)
    let JsoncConfig {
        common_argv,
        instance_argvs,
    } = match config_path.as_deref() {
        Some(path) => {
            let content = std::fs::read_to_string(path).map_err(|e| {
                ErrorMessage::new(format!("設定ファイルの読み込みに失敗しました: {e}"))
            })?;
            parse_jsonc_config(&content)?
        }
        None => JsoncConfig {
            common_argv: Vec::new(),
            instance_argvs: vec![Vec::new()],
        },
    };

    // CLI 引数を共通 / インスタンスに振り分ける
    let (common_cli_argv, instance_cli_argv) = split_cli_argv(cli_after_config)?;

    let mut warnings = Vec::new();
    let (common, _common_help) = parse_common_args(
        &program_name,
        merge_argv(common_argv, common_cli_argv),
        &mut warnings,
    )?;

    let mut instances = Vec::new();
    for argv in instance_argvs {
        let (instance, _help) = parse_instance_args(
            &program_name,
            merge_argv(argv, instance_cli_argv.clone()),
            &mut warnings,
        )?;
        instances.push(instance);
    }

    Ok((common, instances, warnings))
}

/// テンプレート argv と CLI argv を後勝ちで連結する
///
/// JSONC の `instances[i]` は最上位のテンプレートを継承し、CLI はさらにそれを上書きする。
/// noargs は同じオプションが複数あると 2 つ目以降を「想定外の引数」として拒否するため、
/// 連結後に後勝ちで重複を排除する。
fn merge_argv(template: Vec<String>, cli: Vec<String>) -> Vec<String> {
    let mut merged = template;
    merged.extend(cli);
    dedupe_argv_last_wins(merged)
}

/// 同じオプションが複数ある場合に最後の指定だけを残す
///
/// 1 ユニット = キー + 値 (値付きオプション) または キー単体 (フラグ・`--key=value`)。
/// 位置引数 (キーが `--` で始まらないもの) は重複排除しない。
fn dedupe_argv_last_wins(argv: Vec<String>) -> Vec<String> {
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
            units.push(vec![token.clone()]);
            i += 1;
        }
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result_rev: Vec<Vec<String>> = Vec::new();
    for unit in units.into_iter().rev() {
        let first = &unit[0];
        let key = match first.find('=') {
            Some(eq_pos) => first[..eq_pos].to_string(),
            None => first.clone(),
        };
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

/// 共通設定を解釈する
fn parse_common_args(
    program_name: &str,
    argv: Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(CommonArgs, String)> {
    let mut merged = vec![program_name.to_string()];
    merged.extend(argv);
    let mut args = RawArgs::new(merged.into_iter());
    args.metadata_mut().app_name = env!("CARGO_PKG_NAME");
    args.metadata_mut().app_description = "MOQ (Media over QUIC Transport) 負荷試験ツール";
    noargs::HELP_FLAG.take_help(&mut args);

    let instance_hatch_rate: f64 = noargs::opt("instance-hatch-rate")
        .doc("Instance start rate (instances per second, default: 1.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(1.0);

    let http_host: Option<String> = noargs::opt("http-host")
        .doc("HTTP API bind host")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let http_port: Option<u16> = noargs::opt("http-port")
        .doc("HTTP API bind port")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u16>())?;

    let insecure = noargs::flag("insecure")
        .doc("Skip TLS certificate verification")
        .take(&mut args)
        .is_present();

    let log_level_str: Option<String> = noargs::opt("log-level")
        .doc("Log level (verbose, info, warning, error, none, default: info)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let help = args.finish()?.unwrap_or_default();

    if !HATCH_RATE_RANGE.contains(&instance_hatch_rate) {
        return Err(ErrorMessage::new(format!(
            "--instance-hatch-rate は {} から {} の範囲で指定してください",
            HATCH_RATE_RANGE.start(),
            HATCH_RATE_RANGE.end()
        ))
        .into());
    }
    if http_host.is_some() != http_port.is_some() {
        return Err(ErrorMessage::new("--http-host と --http-port は両方指定してください").into());
    }

    let log_level = match log_level_str.as_deref() {
        Some(value) => LogLevel::parse(value)
            .map_err(|e| ErrorMessage::new(format!("--log-level が不正です: {e}")))?,
        None => LogLevel::Info,
    };
    let _ = warnings;

    Ok((
        CommonArgs {
            instance_hatch_rate,
            http_host,
            http_port,
            insecure,
            log_level,
        },
        help,
    ))
}

/// インスタンス設定を解釈する
fn parse_instance_args(
    program_name: &str,
    argv: Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(InstanceArgs, String)> {
    let mut merged = vec![program_name.to_string()];
    merged.extend(argv);
    let mut args = RawArgs::new(merged.into_iter());
    args.metadata_mut().app_name = env!("CARGO_PKG_NAME");
    args.metadata_mut().app_description = "MOQ (Media over QUIC Transport) 負荷試験ツール";
    noargs::HELP_FLAG.take_help(&mut args);

    let url: String = noargs::opt("url")
        .doc("MOQ relay URL (moqt://host:port)")
        .example("moqt://relay.example.com:4433")
        .take(&mut args)
        .then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let namespace: String = noargs::opt("namespace")
        .doc("Track Namespace (default: zakuro)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?
        .unwrap_or_else(|| "zakuro".to_string());

    let tracks_arg: Option<String> = noargs::opt("tracks")
        .doc("Tracks to publish (name[:rate[:size]], comma separated, default: video)")
        .example("video:30:1000,audio:50:200")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let subscribe_tracks_arg: Option<String> = noargs::opt("subscribe-tracks")
        .doc("Track names to subscribe (comma separated)")
        .example("video,audio")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let verify_payload = noargs::flag("verify-payload")
        .doc("Verify that received payloads match the zakuro-moq publisher pattern")
        .take(&mut args)
        .is_present();

    let ca_cert: Option<String> = noargs::opt("ca-cert")
        .doc("CA certificate (PEM) of the relay (default: WebPKI root store)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let vcs: u32 = noargs::opt("vcs")
        .doc("Virtual client count (1-1000, default: 1)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(1);

    let vcs_hatch_rate: f64 = noargs::opt("vcs-hatch-rate")
        .doc("Virtual client start rate (clients per second, default: 1.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(1.0);

    let duration: Option<f64> = noargs::opt("duration")
        .doc("Virtual client connection duration (seconds, unlimited if omitted)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?;

    let repeat_interval: Option<f64> = noargs::opt("repeat-interval")
        .doc("Reconnection interval after duration (seconds)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?;

    let max_retry: u32 = noargs::opt("max-retry")
        .doc("Maximum retry count on connection failure (default: 0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<u32>())?
        .unwrap_or(0);

    let retry_interval: f64 = noargs::opt("retry-interval")
        .doc("Retry interval (seconds, default: 60.0)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<f64>())?
        .unwrap_or(60.0);

    let help = args.finish()?.unwrap_or_default();
    let _ = warnings;

    let instance = validate_instance_args(
        &url,
        &namespace,
        tracks_arg.as_deref(),
        subscribe_tracks_arg.as_deref(),
        verify_payload,
        ca_cert.clone(),
        vcs,
        vcs_hatch_rate,
        duration,
        repeat_interval,
        max_retry,
        retry_interval,
    )?;
    Ok((instance, help))
}

/// 検証を通ったインスタンス設定を組み立てる
#[expect(clippy::too_many_arguments)]
fn validate_instance_args(
    url: &str,
    namespace: &str,
    tracks_arg: Option<&str>,
    subscribe_tracks_arg: Option<&str>,
    verify_payload: bool,
    ca_cert: Option<String>,
    vcs: u32,
    vcs_hatch_rate: f64,
    duration: Option<f64>,
    repeat_interval: Option<f64>,
    max_retry: u32,
    retry_interval: f64,
) -> Result<InstanceArgs> {
    crate::moq_client::MoqEndpoint::parse(url)?;

    if namespace.is_empty() {
        return Err(ErrorMessage::new("--namespace に空文字列は指定できません").into());
    }
    // `.` と `.session` は publish できない予約名前空間
    // (draft-ietf-moq-transport-22 §6.5 (Session-Level Tracks and Namespaces))
    if namespace == "." || namespace == ".session" {
        return Err(ErrorMessage::new(format!(
            "--namespace に予約名前空間 '{namespace}' は指定できません"
        ))
        .into());
    }
    if namespace.len() > MAX_TRACK_NAME_LENGTH {
        return Err(ErrorMessage::new(format!(
            "--namespace は {MAX_TRACK_NAME_LENGTH} バイト以下にしてください"
        ))
        .into());
    }

    // publish するトラック。`--subscribe-tracks` を指定したときは既定値の `video` を
    // 使わない (購読専用で起動したのに publish を始めるのを避ける)
    let tracks = match (tracks_arg, subscribe_tracks_arg) {
        (Some(value), _) => parse_tracks(value)?,
        (None, None) => parse_tracks(DEFAULT_TRACK_NAME)?,
        (None, Some(_)) => Vec::new(),
    };
    let subscribe_tracks = parse_subscribe_tracks(subscribe_tracks_arg)?;

    if vcs == 0 || vcs > 1000 {
        return Err(ErrorMessage::new("--vcs は 1 から 1000 の範囲で指定してください").into());
    }
    if !HATCH_RATE_RANGE.contains(&vcs_hatch_rate) {
        return Err(ErrorMessage::new(format!(
            "--vcs-hatch-rate は {} から {} の範囲で指定してください",
            HATCH_RATE_RANGE.start(),
            HATCH_RATE_RANGE.end()
        ))
        .into());
    }
    if !retry_interval.is_finite() || !(0.0..=MAX_DURATION_SECONDS).contains(&retry_interval) {
        return Err(ErrorMessage::new(format!(
            "--retry-interval は 0 から {MAX_DURATION_SECONDS} の範囲で指定してください"
        ))
        .into());
    }
    // `Duration::from_secs_f64` は inf や表現不能な大きさで panic するため、時間指定は
    // 起動時に検証する
    for (name, value) in [
        ("--duration", duration),
        ("--repeat-interval", repeat_interval),
    ] {
        if let Some(value) = value
            && (!value.is_finite() || !(0.0..=MAX_DURATION_SECONDS).contains(&value))
        {
            return Err(ErrorMessage::new(format!(
                "{name} は 0 から {MAX_DURATION_SECONDS} の範囲で指定してください"
            ))
            .into());
        }
    }

    Ok(InstanceArgs {
        url: url.to_string(),
        namespace: namespace.to_string(),
        tracks,
        subscribe_tracks,
        verify_payload,
        ca_cert,
        vcs,
        vcs_hatch_rate,
        duration,
        repeat_interval,
        max_retry,
        retry_interval,
    })
}

/// `--tracks` の値を解釈する
///
/// 形式は `名前[:レート[:サイズ]]` のカンマ区切り。レートとサイズを省略した場合は
/// 既定値 (`30` objects/sec、`1000` バイト) を使う。
fn parse_tracks(value: &str) -> Result<Vec<TrackSpec>> {
    if value.trim().is_empty() {
        return Err(ErrorMessage::new("--tracks に空文字列は指定できません").into());
    }
    let mut tracks = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(ErrorMessage::new(format!("--tracks の要素が空です: {value}")).into());
        }
        let mut parts = entry.split(':');
        let name = parts.next().unwrap_or_default().trim().to_string();
        if name.is_empty() {
            return Err(
                ErrorMessage::new(format!("--tracks のトラック名が空です: {entry}")).into(),
            );
        }
        if name.len() + TRACK_NAME_SUFFIX_BUDGET > MAX_TRACK_NAME_LENGTH {
            return Err(ErrorMessage::new(format!(
                "--tracks のトラック名は {MAX_TRACK_NAME_LENGTH} バイト以下にしてください (仮想クライアント識別子の suffix を含む): {name}"
            ))
            .into());
        }
        let object_rate = match parts.next() {
            Some(rate) => rate.trim().parse::<f64>().map_err(|_| {
                ErrorMessage::new(format!("--tracks のレートが数値ではありません: {entry}"))
            })?,
            None => DEFAULT_OBJECT_RATE,
        };
        if !OBJECT_RATE_RANGE.contains(&object_rate) {
            return Err(ErrorMessage::new(format!(
                "--tracks のレートは {} から {} の範囲で指定してください: {entry}",
                OBJECT_RATE_RANGE.start(),
                OBJECT_RATE_RANGE.end()
            ))
            .into());
        }
        let object_size = match parts.next() {
            Some(size) => size.trim().parse::<usize>().map_err(|_| {
                ErrorMessage::new(format!("--tracks のサイズが数値ではありません: {entry}"))
            })?,
            None => DEFAULT_OBJECT_SIZE,
        };
        if parts.next().is_some() {
            return Err(ErrorMessage::new(format!(
                "--tracks の形式は 名前[:レート[:サイズ]] です: {entry}"
            ))
            .into());
        }
        if object_size == 0 || object_size > MAX_OBJECT_SIZE {
            return Err(ErrorMessage::new(format!(
                "--tracks のサイズは 1 から {MAX_OBJECT_SIZE} の範囲で指定してください: {entry}"
            ))
            .into());
        }
        if tracks.iter().any(|t: &TrackSpec| t.name == name) {
            return Err(
                ErrorMessage::new(format!("--tracks に同名のトラックがあります: {name}")).into(),
            );
        }
        tracks.push(TrackSpec {
            name,
            object_rate,
            object_size,
        });
    }
    Ok(tracks)
}

/// `--subscribe-tracks` の値を解釈する
///
/// 形式はトラック名のカンマ区切り。Full Track Name は publish と同じく
/// `<名前>-<instance>-<vc>` とする。
fn parse_subscribe_tracks(value: Option<&str>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.trim().is_empty() {
        return Err(ErrorMessage::new("--subscribe-tracks に空文字列は指定できません").into());
    }
    let mut tracks = Vec::new();
    for entry in value.split(',') {
        let name = entry.trim();
        if name.is_empty() {
            return Err(
                ErrorMessage::new(format!("--subscribe-tracks の要素が空です: {value}")).into(),
            );
        }
        if name.len() + TRACK_NAME_SUFFIX_BUDGET > MAX_TRACK_NAME_LENGTH {
            return Err(ErrorMessage::new(format!(
                "--subscribe-tracks のトラック名は {MAX_TRACK_NAME_LENGTH} バイト以下にしてください (仮想クライアント識別子の suffix を含む): {name}"
            ))
            .into());
        }
        if tracks.iter().any(|t: &String| t == name) {
            return Err(ErrorMessage::new(format!(
                "--subscribe-tracks に同名のトラックがあります: {name}"
            ))
            .into());
        }
        tracks.push(name.to_string());
    }
    Ok(tracks)
}

/// CLI 引数を共通 / インスタンス用に分割する
///
/// `is_common_key()` で振り分け、次のトークンを値として取るかどうかは `is_flag()` で判定する。
fn split_cli_argv(cli_argv: Vec<String>) -> Result<(Vec<String>, Vec<String>)> {
    let mut common: Vec<String> = Vec::new();
    let mut instance: Vec<String> = Vec::new();
    let mut iter = cli_argv.into_iter().peekable();
    while let Some(token) = iter.next() {
        // `--key=value` 形式
        if let Some(rest) = token.strip_prefix("--") {
            if let Some((key, value)) = rest.split_once('=') {
                if is_common_key(key) {
                    common.push(format!("--{key}"));
                    common.push(value.to_string());
                } else {
                    instance.push(token);
                }
                continue;
            }
            let is_flag_key = is_flag(rest);
            let takes_value = !is_flag_key && !matches!(rest, "help" | "version");
            let mut pair: Vec<String> = vec![token.clone()];
            if takes_value {
                match iter.peek() {
                    Some(next) if !next.starts_with("--") => {
                        pair.push(iter.next().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            if is_common_key(rest) {
                common.extend(pair);
            } else {
                instance.extend(pair);
            }
            continue;
        }
        instance.push(token);
    }
    Ok((common, instance))
}

/// JSONC 文字列を argv 群へ展開する
fn parse_jsonc_config(content: &str) -> Result<JsoncConfig> {
    let (json, _) = RawJson::parse_jsonc(content)
        .map_err(|e| ErrorMessage::new(format!("設定ファイルのパースに失敗しました: {e}")))?;
    let root = json.value();
    if root.kind() != JsonValueKind::Object {
        return Err(
            ErrorMessage::new("設定ファイルは JSON オブジェクトである必要があります").into(),
        );
    }

    let mut common_argv: Vec<String> = Vec::new();
    let mut template_argv: Vec<String> = Vec::new();
    let mut instances_value: Option<RawJsonValue<'_, '_>> = None;

    let members = root
        .to_object()
        .map_err(|e| ErrorMessage::new(format!("設定ファイルのパースに失敗しました: {e}")))?;
    for (key_value, value) in members {
        let key: String = key_value
            .try_into()
            .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
        if key == "config" {
            return Err(ErrorMessage::new("設定ファイル内に 'config' は指定できません").into());
        }
        if key == "instances" {
            instances_value = Some(value);
            continue;
        }
        if is_common_key(&key) {
            push_kv(&key, value, &mut common_argv)?;
            continue;
        }
        if key == "tracks" {
            push_tracks(value, &mut template_argv)?;
            continue;
        }
        if key == "subscribe-tracks" {
            push_subscribe_tracks(value, &mut template_argv)?;
            continue;
        }
        push_kv(&key, value, &mut template_argv)?;
    }

    let instance_argvs = match instances_value {
        Some(value) => expand_instances(value, &template_argv)?,
        None => vec![template_argv],
    };
    Ok(JsoncConfig {
        common_argv,
        instance_argvs,
    })
}

/// `instances` 配列を argv 群へ展開する
fn expand_instances(value: RawJsonValue<'_, '_>, template: &[String]) -> Result<Vec<Vec<String>>> {
    if value.kind() != JsonValueKind::Array {
        return Err(ErrorMessage::new("'instances' は JSON 配列で指定してください").into());
    }
    let elements: Vec<RawJsonValue<'_, '_>> = value
        .to_array()
        .map_err(|e| ErrorMessage::new(format!("instances のパースに失敗しました: {e}")))?
        .collect();
    if elements.is_empty() || elements.len() > 64 {
        return Err(ErrorMessage::new("instances は 1 から 64 の範囲で指定してください").into());
    }

    let mut result = Vec::new();
    for (i, instance) in elements.into_iter().enumerate() {
        if instance.kind() != JsonValueKind::Object {
            return Err(ErrorMessage::new(format!(
                "instances[{i}] は JSON オブジェクトで指定してください"
            ))
            .into());
        }
        let mut argv = template.to_vec();
        let members = instance.to_object().map_err(|e| {
            ErrorMessage::new(format!("instances[{i}] のパースに失敗しました: {e}"))
        })?;
        for (key_value, value) in members {
            let key: String = key_value
                .try_into()
                .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
            if is_common_key(&key) {
                return Err(ErrorMessage::new(format!(
                    "共通オプション '{key}' は instances[{i}] の中に書けません"
                ))
                .into());
            }
            if key == "config" || key == "instances" {
                return Err(ErrorMessage::new(format!(
                    "'{key}' は instances[{i}] の中に書けません"
                ))
                .into());
            }
            if key == "tracks" {
                push_tracks(value, &mut argv)?;
                continue;
            }
            if key == "subscribe-tracks" {
                push_subscribe_tracks(value, &mut argv)?;
                continue;
            }
            push_kv(&key, value, &mut argv)?;
        }
        result.push(argv);
    }
    Ok(result)
}

/// `tracks` 配列を `--tracks name:rate:size,...` へ変換する
fn push_tracks(value: RawJsonValue<'_, '_>, argv: &mut Vec<String>) -> Result<()> {
    if value.kind() != JsonValueKind::Array {
        return Err(ErrorMessage::new("'tracks' は JSON 配列で指定してください").into());
    }
    let mut specs: Vec<String> = Vec::new();
    for (i, element) in value
        .to_array()
        .map_err(|e| ErrorMessage::new(format!("tracks のパースに失敗しました: {e}")))?
        .enumerate()
    {
        if element.kind() != JsonValueKind::Object {
            return Err(ErrorMessage::new(format!(
                "tracks[{i}] は JSON オブジェクトで指定してください"
            ))
            .into());
        }
        let name: String = element
            .to_member("name")
            .map_err(|_| ErrorMessage::new(format!("tracks[{i}] に name がありません")))?
            .required()
            .map_err(|_| ErrorMessage::new(format!("tracks[{i}] に name がありません")))?
            .try_into()
            .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
        // 未指定は CLI 側と同じ既定値にする。指定されているのに型が違う場合は黙って
        // 既定値へ落とさずエラーにする (指定したつもりの負荷が出ない事故を防ぐ)
        let rate = match member_value(element, "object-rate")? {
            Some(value) => {
                let rate: f64 = value.try_into().map_err(|_| {
                    ErrorMessage::new(format!(
                        "tracks[{i}] の object-rate が数値ではありません: {}",
                        value.as_raw_str()
                    ))
                })?;
                rate
            }
            None => DEFAULT_OBJECT_RATE,
        };
        let size = match member_value(element, "object-size")? {
            Some(value) => {
                let size: usize = value.try_into().map_err(|_| {
                    ErrorMessage::new(format!(
                        "tracks[{i}] の object-size が 0 以上の整数ではありません: {}",
                        value.as_raw_str()
                    ))
                })?;
                size
            }
            None => DEFAULT_OBJECT_SIZE,
        };
        specs.push(format!("{name}:{rate}:{size}"));
    }
    argv.push("--tracks".to_string());
    argv.push(specs.join(","));
    Ok(())
}

/// `subscribe-tracks` 配列を `--subscribe-tracks a,b` へ変換する
fn push_subscribe_tracks(value: RawJsonValue<'_, '_>, argv: &mut Vec<String>) -> Result<()> {
    if value.kind() != JsonValueKind::Array {
        return Err(ErrorMessage::new("'subscribe-tracks' は JSON 配列で指定してください").into());
    }
    let mut names: Vec<String> = Vec::new();
    for (i, element) in value
        .to_array()
        .map_err(|e| ErrorMessage::new(format!("subscribe-tracks のパースに失敗しました: {e}")))?
        .enumerate()
    {
        let name: String = element.try_into().map_err(|_| {
            ErrorMessage::new(format!("subscribe-tracks[{i}] は文字列で指定してください"))
        })?;
        names.push(name);
    }
    argv.push("--subscribe-tracks".to_string());
    argv.push(names.join(","));
    Ok(())
}

/// オブジェクトのメンバーを取り出す (未指定なら `None`)
fn member_value<'a, 'b>(
    object: RawJsonValue<'a, 'b>,
    key: &str,
) -> Result<Option<RawJsonValue<'a, 'b>>> {
    match object.to_member(key) {
        Ok(member) => Ok(member.required().ok()),
        Err(_) => Ok(None),
    }
}

/// JSON のメンバーを `--key value` の形で argv へ積む
fn push_kv(key: &str, value: RawJsonValue<'_, '_>, argv: &mut Vec<String>) -> Result<()> {
    if value.kind() == JsonValueKind::String && value.as_raw_str().contains("${") {
        return Err(ErrorMessage::new(format!(
            "環境変数置換 '${{...}}' は未対応です (key: '{key}')"
        ))
        .into());
    }
    argv.push(format!("--{key}"));
    match value.kind() {
        JsonValueKind::String => {
            let text: String = value
                .try_into()
                .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
            argv.push(text);
        }
        JsonValueKind::Boolean => {
            let flag: bool = value
                .try_into()
                .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
            if is_flag(key) {
                // false の場合はフラグを立てない
                if !flag {
                    argv.pop();
                }
            } else {
                argv.push(flag.to_string());
            }
        }
        _ => {
            argv.push(value.as_raw_str().to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `名前:レート:サイズ` を解釈できること
    #[test]
    fn parse_tracks_reads_full_spec() {
        let tracks = parse_tracks("video:30:1000,audio:50:200").expect("解釈に成功すること");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].name, "video");
        assert_eq!(tracks[0].object_rate, 30.0);
        assert_eq!(tracks[0].object_size, 1000);
        assert_eq!(tracks[1].name, "audio");
        assert_eq!(tracks[1].object_rate, 50.0);
        assert_eq!(tracks[1].object_size, 200);
    }

    /// レートとサイズを省略すると既定値になること
    #[test]
    fn parse_tracks_uses_defaults() {
        let tracks = parse_tracks("video").expect("解釈に成功すること");
        assert_eq!(tracks[0].object_rate, DEFAULT_OBJECT_RATE);
        assert_eq!(tracks[0].object_size, DEFAULT_OBJECT_SIZE);
    }

    /// `Duration` で表現できない時間指定を拒否すること
    ///
    /// `Duration::from_secs_f64` は極小の周期 (1.0 / rate) や inf で panic するため、
    /// 起動時に弾く。
    #[test]
    fn validate_rejects_unrepresentable_durations() {
        let validate = |vcs_hatch_rate: f64,
                        duration: Option<f64>,
                        repeat_interval: Option<f64>,
                        retry_interval: f64| {
            validate_instance_args(
                "moqt://relay.example.com:4433",
                "zakuro",
                Some("video"),
                None,
                false,
                None,
                1,
                vcs_hatch_rate,
                duration,
                repeat_interval,
                0,
                retry_interval,
            )
        };

        // 周期が Duration で表現できない起動レート
        assert!(
            validate(1e-300, None, None, 60.0).is_err(),
            "1e-300 の起動レートを許容してはならない"
        );
        assert!(
            validate(1e7, None, None, 60.0).is_err(),
            "大きすぎる起動レートを許容してはならない"
        );
        // inf / NaN の時間指定
        assert!(
            validate(1.0, Some(f64::INFINITY), None, 60.0).is_err(),
            "inf の --duration を許容してはならない"
        );
        assert!(
            validate(1.0, None, Some(f64::INFINITY), 60.0).is_err(),
            "inf の --repeat-interval を許容してはならない"
        );
        assert!(
            validate(1.0, None, None, f64::NAN).is_err(),
            "NaN の --retry-interval を許容してはならない"
        );
        // 妥当な値は通る
        assert!(validate(10.0, Some(60.0), Some(1.0), 60.0).is_ok());
    }

    /// `--subscribe-tracks` を解釈できること
    #[test]
    fn parse_subscribe_tracks_reads_names() {
        let tracks = parse_subscribe_tracks(Some("video,audio")).expect("解釈に成功すること");
        assert_eq!(tracks, vec!["video".to_string(), "audio".to_string()]);
        assert!(
            parse_subscribe_tracks(None)
                .expect("未指定も成功すること")
                .is_empty()
        );
    }

    /// `--subscribe-tracks` の不正な値は拒否すること
    #[test]
    fn parse_subscribe_tracks_rejects_invalid_entries() {
        assert!(parse_subscribe_tracks(Some("")).is_err());
        assert!(parse_subscribe_tracks(Some("video,,audio")).is_err());
        assert!(parse_subscribe_tracks(Some("video,video")).is_err());
    }

    /// JSONC の真偽値フラグが値なしのオプションとして展開されること
    ///
    /// `--verify-payload` は noargs の flag であり値を取らないため、`true` を値として
    /// 積むと「unexpected argument」で起動に失敗する。
    #[test]
    fn jsonc_boolean_flags_have_no_value() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "subscribe-tracks": ["video"],
            "verify-payload": true,
            "insecure": true
        }"#;
        let config = parse_jsonc_config(content).expect("パースに成功すること");
        let argv = &config.instance_argvs[0];
        assert!(argv.iter().any(|s| s == "--verify-payload"));
        assert!(
            !argv.iter().any(|s| s == "true"),
            "フラグに値が積まれている: {argv:?}"
        );

        // false の場合はフラグ自体を積まない
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "subscribe-tracks": ["video"],
            "verify-payload": false
        }"#;
        let config = parse_jsonc_config(content).expect("パースに成功すること");
        assert!(
            !config.instance_argvs[0]
                .iter()
                .any(|s| s == "--verify-payload")
        );
    }

    /// JSONC の subscribe-tracks 配列が `--subscribe-tracks` へ変換されること
    #[test]
    fn jsonc_subscribe_tracks_array_is_flattened() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "subscribe-tracks": ["video", "audio"]
        }"#;
        let config = parse_jsonc_config(content).expect("パースに成功すること");
        let argv = &config.instance_argvs[0];
        let pos = argv
            .iter()
            .position(|s| s == "--subscribe-tracks")
            .expect("--subscribe-tracks があること");
        assert_eq!(argv[pos + 1], "video,audio");
    }

    /// JSONC の subscribe-tracks は文字列の配列でなければならないこと
    #[test]
    fn jsonc_subscribe_tracks_rejects_non_string() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "subscribe-tracks": [1, 2]
        }"#;
        assert!(parse_jsonc_config(content).is_err());
    }

    /// JSONC のトラック設定が型不正なら拒否すること
    ///
    /// 黙って既定値へ落とすと「指定したつもりの負荷が出ていない」事故になる。
    #[test]
    fn jsonc_tracks_reject_invalid_types() {
        let invalid_rate = r#"{
            "url": "moqt://relay.example.com:4433",
            "tracks": [ { "name": "video", "object-rate": "abc" } ]
        }"#;
        assert!(
            parse_jsonc_config(invalid_rate).is_err(),
            "object-rate の型不正を許容してはならない"
        );

        let negative_size = r#"{
            "url": "moqt://relay.example.com:4433",
            "tracks": [ { "name": "video", "object-size": -5 } ]
        }"#;
        assert!(
            parse_jsonc_config(negative_size).is_err(),
            "object-size の負値を許容してはならない"
        );

        let missing_name = r#"{
            "url": "moqt://relay.example.com:4433",
            "tracks": [ { "object-rate": 30 } ]
        }"#;
        assert!(
            parse_jsonc_config(missing_name).is_err(),
            "name の欠落を許容してはならない"
        );
    }

    /// 同名のトラックは拒否すること
    #[test]
    fn parse_tracks_rejects_duplicates() {
        assert!(parse_tracks("video,video").is_err());
    }

    /// 空要素や不正な数値は拒否すること
    #[test]
    fn parse_tracks_rejects_invalid_entries() {
        assert!(parse_tracks("").is_err());
        assert!(parse_tracks("video,,audio").is_err());
        assert!(parse_tracks("video:abc").is_err());
        assert!(parse_tracks("video:30:0").is_err());
        assert!(parse_tracks("video:30:1000:extra").is_err());
        assert!(parse_tracks(":30:1000").is_err());
    }

    /// 範囲外のレートは拒否すること (Duration 変換の panic を防ぐ)
    #[test]
    fn parse_tracks_rejects_out_of_range_rate() {
        assert!(parse_tracks("video:0.0001").is_err());
        assert!(parse_tracks("video:1000000.5").is_err());
    }

    /// JSONC の tracks 配列が `--tracks` へ変換されること
    #[test]
    fn jsonc_tracks_array_is_flattened() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "namespace": "zakuro",
            "tracks": [
                { "name": "video", "object-rate": 30, "object-size": 1000 },
                { "name": "audio", "object-rate": 50 }
            ]
        }"#;
        let config = parse_jsonc_config(content).expect("パースに成功すること");
        assert_eq!(config.instance_argvs.len(), 1);
        let argv = &config.instance_argvs[0];
        let tracks_pos = argv
            .iter()
            .position(|s| s == "--tracks")
            .expect("--tracks があること");
        assert_eq!(
            argv[tracks_pos + 1],
            format!("video:30:1000,audio:50:{DEFAULT_OBJECT_SIZE}")
        );
    }

    /// JSONC の instances 配列がテンプレートを継承すること
    #[test]
    fn jsonc_instances_inherit_template() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "namespace": "base",
            "vcs": 5,
            "instances": [
                { "namespace": "a" },
                { "namespace": "b", "vcs": 10 }
            ]
        }"#;
        let config = parse_jsonc_config(content).expect("パースに成功すること");
        assert_eq!(config.instance_argvs.len(), 2);
        // テンプレートの namespace=base の後に a が積まれ、後勝ちになる
        assert!(
            config.instance_argvs[0]
                .windows(2)
                .any(|w| w[0] == "--namespace" && w[1] == "base")
        );
        assert!(
            config.instance_argvs[0]
                .windows(2)
                .any(|w| w[0] == "--namespace" && w[1] == "a")
        );
        assert!(
            config.instance_argvs[1]
                .windows(2)
                .any(|w| w[0] == "--vcs" && w[1] == "10")
        );
    }

    /// CLI 引数が共通 / インスタンスに振り分けられること
    #[test]
    fn split_cli_argv_routes_keys() {
        let cli = vec![
            "--http-host".to_string(),
            "127.0.0.1".to_string(),
            "--http-port".to_string(),
            "8080".to_string(),
            "--insecure".to_string(),
            "--url".to_string(),
            "moqt://relay.example.com/".to_string(),
            "--vcs".to_string(),
            "10".to_string(),
        ];
        let (common, instance) = split_cli_argv(cli).expect("分割に成功すること");
        assert_eq!(
            common,
            vec![
                "--http-host",
                "127.0.0.1",
                "--http-port",
                "8080",
                "--insecure"
            ]
        );
        assert_eq!(
            instance,
            vec!["--url", "moqt://relay.example.com/", "--vcs", "10"]
        );
    }

    /// 同じオプションが複数ある場合は後勝ちになること
    #[test]
    fn dedupe_keeps_last_value() {
        let argv = vec![
            "--tracks".to_string(),
            "video:30:1000".to_string(),
            "--vcs".to_string(),
            "1".to_string(),
            "--tracks".to_string(),
            "catalog:10:100".to_string(),
        ];
        assert_eq!(
            dedupe_argv_last_wins(argv),
            vec!["--vcs", "1", "--tracks", "catalog:10:100"]
        );
    }

    /// フラグと `--key=value` 形式も重複排除できること
    #[test]
    fn dedupe_handles_flags_and_equal_form() {
        let argv = vec![
            "--insecure".to_string(),
            "--url=moqt://a.example.com/".to_string(),
            "--insecure".to_string(),
            "--url=moqt://b.example.com/".to_string(),
        ];
        assert_eq!(
            dedupe_argv_last_wins(argv),
            vec!["--insecure", "--url=moqt://b.example.com/"]
        );
    }

    /// 位置引数は重複排除しないこと
    #[test]
    fn dedupe_keeps_positional_arguments() {
        let argv = vec!["a".to_string(), "a".to_string()];
        assert_eq!(dedupe_argv_last_wins(argv), vec!["a", "a"]);
    }

    /// `--key=value` 形式も振り分けられること
    #[test]
    fn split_cli_argv_handles_equal_form() {
        let cli = vec![
            "--log-level=warning".to_string(),
            "--namespace=test".to_string(),
        ];
        let (common, instance) = split_cli_argv(cli).expect("分割に成功すること");
        assert_eq!(common, vec!["--log-level", "warning"]);
        assert_eq!(instance, vec!["--namespace=test"]);
    }
}
