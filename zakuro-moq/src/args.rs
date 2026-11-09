//! CLI 引数と JSONC 設定ファイルの解釈

use noargs::RawArgs;
use nojson::{JsonValueKind, RawJson, RawJsonValue};

use crate::error::{ErrorMessage, Result};
use crate::logging::LogLevel;
use crate::moq_client::MoqTransport;

/// 映像トラックの既定フレームレート (frames/sec)
///
/// draft-ietf-moq-loc-05 §4.2 (Application with one single quality video track) の例に
/// 合わせる。節番号は draft 由来であり将来の改版で変わる可能性がある。
const DEFAULT_VIDEO_FRAMERATE: f64 = 30.0;

/// 映像トラックの既定ビットレート (bit/sec)
const DEFAULT_VIDEO_BITRATE: u64 = 1_000_000;

/// 映像トラックの既定 Group 間隔 (秒)
///
/// IDR 境界の間隔である。この秒数ぶんのフレームを 1 Group に入れる。
const DEFAULT_VIDEO_INTERVAL: f64 = 2.0;

/// 音声トラックの既定ビットレート (bit/sec)
///
/// draft-ietf-moq-loc-05 §4.1 (Application with one audio track) の例に合わせる。
const DEFAULT_AUDIO_BITRATE: u64 = 24_000;

/// 音声トラックの既定チャンク間隔 (秒)
///
/// 1 チャンクが 1 object かつ 1 Group になる。20 ms は音声フレームの一般的な長さである。
const DEFAULT_AUDIO_INTERVAL: f64 = 0.02;

/// 映像の Group 間隔として受け付ける最大秒数
const MAX_VIDEO_INTERVAL: f64 = 60.0;

/// 音声チャンク間隔として受け付ける範囲 (秒)
const AUDIO_INTERVAL_RANGE: std::ops::RangeInclusive<f64> = 0.001..=1.0;

/// `framerate * interval` を整数フレームとみなす許容誤差
const FRAME_COUNT_EPSILON: f64 = 1e-3;

/// 1 Group に入れる object 数の上限
const MAX_OBJECTS_PER_GROUP: u64 = 100_000;

/// datagram 配送で送る object 1 個のサイズ上限の既定値 (バイト)
///
/// RFC 9000 §14 (Datagram Size) は経路が最低限支える datagram サイズを 1200 バイト
/// (QUIC パケットヘッダと AEAD タグを含む UDP payload) と定めている。ここで判定するのは
/// MOQT の OBJECT_DATAGRAM (ヘッダ + Properties + payload) の長さであり、QUIC のパケット
/// ヘッダ、packet number、AEAD タグ、DATAGRAM frame の type / Length は含まれないため、
/// その分の余裕を引いた値を既定にする。
const DEFAULT_DATAGRAM_MAX_SIZE: usize = 1160;

/// datagram のサイズ上限の許容範囲 (バイト)
///
/// 1 は object が入らず、65535 は QUIC DATAGRAM の最大値 (RFC 9221 §3) である。
const DATAGRAM_MAX_SIZE_RANGE: std::ops::RangeInclusive<usize> = 1..=65535;

/// OBJECT_DATAGRAM のヘッダと LOC プロパティに必要な最大バイト数
///
/// Type Flags (1 バイト) と Track Alias / Group ID / Object ID の varint (最大 8 バイトずつ)、
/// LOC の Timestamp / Timescale プロパティ (最大 15 バイト) を保守的に見積もる。
/// 起動時に object サイズが datagram に収まるかを判定するために使う。
const DATAGRAM_HEADER_MAX_SIZE: usize = 48;

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
/// draft-ietf-moq-transport-23 §8.7 (Track Namespace Structure) が Track Namespace と
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

/// object の配送方式 (draft-ietf-moq-transport-23 §2.1.1 (Object Fields))
///
/// Delivery Mode は Original Publisher が object ごとに決める値であり、購読側とは
/// 交渉しない。zakuro-moq はメディア種別で方式を固定する。映像は subgroup、音声は
/// datagram である。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryMode {
    /// subgroup stream で送る
    Subgroup,
    /// OBJECT_DATAGRAM で送る
    Datagram,
}

impl DeliveryMode {
    /// CLI とログに載せる名前を返す
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Subgroup => "subgroup",
            Self::Datagram => "datagram",
        }
    }
}

/// トラックのメディア種別
///
/// payload は符号化しない。ビットレートから求めたバイト数のダミー object を、
/// ここで決めた Group / Subgroup 境界で送る。コーデックを載せるのは負荷試験の
/// 通常経路ではない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TrackMedia {
    /// 単一品質の映像
    ///
    /// Group は `interval` 秒 (IDR 境界) ごとに切り、Group 内のフレームは
    /// Subgroup ID 0 の stream 1 本に載せる (draft-ietf-moq-loc-05 §4.2)。
    /// 節番号は draft 由来であり将来の改版で変わる可能性がある。
    Video {
        /// フレームレート (frames/sec)。1 フレームが 1 object になる
        framerate: f64,
        /// ビットレート (bit/sec)。1 frame のサイズは `bitrate / 8 / framerate`
        bitrate: u64,
        /// Group 間隔 (秒)
        interval: f64,
        /// `bitrate` と `framerate` から求めた 1 object の payload サイズ (バイト)
        object_size: usize,
        /// 1 Group のフレーム数 (`framerate * interval`)
        objects_per_group: u64,
    },
    /// 音声
    ///
    /// 1 チャンクが 1 object かつ 1 Group で、Object ID は 0 のまま進む
    /// (draft-ietf-moq-loc-05 §4.1)。配送は datagram である。
    Audio {
        /// ビットレート (bit/sec)
        bitrate: u64,
        /// チャンク間隔 (秒)。1 チャンクが 1 object になる
        interval: f64,
        /// `bitrate` と `interval` から求めた 1 object の payload サイズ (バイト)
        object_size: usize,
    },
}

impl TrackMedia {
    /// 1 秒あたりに送る object 数を返す
    pub(crate) fn object_rate(self) -> f64 {
        match self {
            Self::Video { framerate, .. } => framerate,
            Self::Audio { interval, .. } => 1.0 / interval,
        }
    }

    /// 1 object の payload サイズ (バイト) を返す
    pub(crate) fn object_size(self) -> usize {
        match self {
            Self::Video { object_size, .. } | Self::Audio { object_size, .. } => object_size,
        }
    }

    /// 1 Group に入れる object 数を返す
    ///
    /// 音声は常に 1 である。
    pub(crate) fn objects_per_group(self) -> u64 {
        match self {
            Self::Video {
                objects_per_group, ..
            } => objects_per_group,
            Self::Audio { .. } => 1,
        }
    }

    /// このメディアの配送方式を返す
    pub(crate) fn delivery(self) -> DeliveryMode {
        match self {
            Self::Video { .. } => DeliveryMode::Subgroup,
            Self::Audio { .. } => DeliveryMode::Datagram,
        }
    }

    /// CLI とログに載せる種別名を返す
    pub(crate) fn kind_name(self) -> &'static str {
        match self {
            Self::Video { .. } => "video",
            Self::Audio { .. } => "audio",
        }
    }
}

/// トラック 1 本の設定
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrackSpec {
    /// トラック名の接頭辞 (実行時に `<名前>-<instance>-<vc>` とする)
    pub(crate) name: String,
    /// メディアのシミュレーション
    pub(crate) media: TrackMedia,
}

impl TrackSpec {
    /// CLI の `--tracks` に戻せる文字列を返す
    fn to_cli(&self) -> String {
        match self.media {
            TrackMedia::Video {
                framerate,
                bitrate,
                interval,
                ..
            } => format!(
                "{}:video:{}:{bitrate}:{}",
                self.name,
                format_f64(framerate),
                format_f64(interval),
            ),
            TrackMedia::Audio {
                bitrate, interval, ..
            } => format!("{}:audio:{bitrate}:{}", self.name, format_f64(interval),),
        }
    }
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
    /// 下位トランスポート種別 (QUIC / WebTransport over HTTP/3 / QMux)
    pub(crate) transport: MoqTransport,
    /// Track Namespace
    pub(crate) namespace: String,
    /// publish するトラック (1 仮想クライアントが全トラックを publish する)
    pub(crate) tracks: Vec<TrackSpec>,
    /// datagram 配送で送る object 1 個のサイズ上限 (バイト)
    pub(crate) datagram_max_size: usize,
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
    /// GOAWAY を受信したときに New Session URI へ再接続する
    pub(crate) reconnect_on_goaway: bool,
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

    let transport: MoqTransport = noargs::opt("transport")
        .doc("Transport (quic | wt-h3 | wt-h2 | qmux, default: quic)")
        .take(&mut args)
        .present_and_then(|o| MoqTransport::parse(o.value()))?
        .unwrap_or_default();

    let namespace: String = noargs::opt("namespace")
        .doc("Track Namespace (default: zakuro)")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?
        .unwrap_or_else(|| "zakuro".to_string());

    let tracks_arg: Option<String> = noargs::opt("tracks")
        .doc("Tracks to publish (video[:framerate:bitrate:interval] or audio[:bitrate:interval], comma separated, default: video)")
        .example("video:30:1000000:2,audio:24000:0.02")
        .take(&mut args)
        .present_and_then(|o| Ok::<_, &str>(o.value().to_string()))?;

    let datagram_max_size: usize = noargs::opt("datagram-max-size")
        .doc("Maximum size of an OBJECT_DATAGRAM in bytes (default: 1160)")
        .take(&mut args)
        .present_and_then(|o| o.value().parse::<usize>())?
        .unwrap_or(DEFAULT_DATAGRAM_MAX_SIZE);

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

    let reconnect_on_goaway: bool = noargs::opt("reconnect-on-goaway")
        .doc("Reconnect to the New Session URI when a GOAWAY is received (true/false, default: true)")
        .take(&mut args)
        .present_and_then(|o| match o.value() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("--reconnect-on-goaway は true または false で指定してください"),
        })?
        .unwrap_or(true);

    let help = args.finish()?.unwrap_or_default();
    let _ = warnings;

    let instance = validate_instance_args(
        &url,
        transport,
        &namespace,
        tracks_arg.as_deref(),
        datagram_max_size,
        subscribe_tracks_arg.as_deref(),
        verify_payload,
        ca_cert.clone(),
        vcs,
        vcs_hatch_rate,
        duration,
        repeat_interval,
        max_retry,
        retry_interval,
        reconnect_on_goaway,
    )?;
    Ok((instance, help))
}

/// 検証を通ったインスタンス設定を組み立てる
#[expect(clippy::too_many_arguments)]
fn validate_instance_args(
    url: &str,
    transport: MoqTransport,
    namespace: &str,
    tracks_arg: Option<&str>,
    datagram_max_size: usize,
    subscribe_tracks_arg: Option<&str>,
    verify_payload: bool,
    ca_cert: Option<String>,
    vcs: u32,
    vcs_hatch_rate: f64,
    duration: Option<f64>,
    repeat_interval: Option<f64>,
    max_retry: u32,
    retry_interval: f64,
    reconnect_on_goaway: bool,
) -> Result<InstanceArgs> {
    crate::moq_client::MoqEndpoint::parse(url)?;

    if namespace.is_empty() {
        return Err(ErrorMessage::new("--namespace に空文字列は指定できません").into());
    }
    // `.` と `.session` は publish できない予約名前空間
    // (draft-ietf-moq-transport-23 §6.5 (Session-Level Tracks and Namespaces))
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

    // datagram は分割できず 1 つの QUIC パケットに収まる必要がある (RFC 9221 §5)。
    // 経路の上限を超えた datagram は通知なく破棄されるため (draft-ietf-moq-transport-23
    // §11.2 (Datagrams))、起動時に object サイズが収まるかを判定する
    if !DATAGRAM_MAX_SIZE_RANGE.contains(&datagram_max_size) {
        return Err(ErrorMessage::new(format!(
            "--datagram-max-size は {} から {} の範囲で指定してください",
            DATAGRAM_MAX_SIZE_RANGE.start(),
            DATAGRAM_MAX_SIZE_RANGE.end()
        ))
        .into());
    }
    for track in tracks
        .iter()
        .filter(|track| track.media.delivery() == DeliveryMode::Datagram)
    {
        let required = track.media.object_size() + DATAGRAM_HEADER_MAX_SIZE;
        if required > datagram_max_size {
            return Err(ErrorMessage::new(format!(
                "datagram 配送のトラックが --datagram-max-size に収まりません: track={} object-size={} ヘッダ込み={} --datagram-max-size={}",
                track.name,
                track.media.object_size(),
                required,
                datagram_max_size
            ))
            .into());
        }
    }

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
        transport,
        namespace: namespace.to_string(),
        tracks,
        datagram_max_size,
        subscribe_tracks,
        verify_payload,
        ca_cert,
        vcs,
        vcs_hatch_rate,
        duration,
        repeat_interval,
        max_retry,
        retry_interval,
        reconnect_on_goaway,
    })
}

/// `--tracks` の値を解釈する
///
/// 映像は `名前[:video]:[framerate[:bitrate[:interval]]]`、音声は
/// `名前[:audio]:[bitrate[:interval]]` である。名前が `video` または `audio` のときは
/// 種別を省略できる。省略したパラメータはメディア種別の既定値になる。payload は
/// 符号化せず、ビットレートから求めたサイズのダミーバイト列を送る。
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
        let spec = parse_track_entry(entry)?;
        if tracks.iter().any(|t: &TrackSpec| t.name == spec.name) {
            return Err(ErrorMessage::new(format!(
                "--tracks に同名のトラックがあります: {}",
                spec.name
            ))
            .into());
        }
        tracks.push(spec);
    }
    Ok(tracks)
}

/// `--tracks` の 1 要素を解釈する
fn parse_track_entry(entry: &str) -> Result<TrackSpec> {
    let parts: Vec<&str> = entry.split(':').map(str::trim).collect();
    let name = parts.first().copied().unwrap_or_default();
    if name.is_empty() {
        return Err(ErrorMessage::new(format!("--tracks のトラック名が空です: {entry}")).into());
    }
    if name.len() + TRACK_NAME_SUFFIX_BUDGET > MAX_TRACK_NAME_LENGTH {
        return Err(ErrorMessage::new(format!(
            "--tracks のトラック名は {MAX_TRACK_NAME_LENGTH} バイト以下にしてください (仮想クライアント識別子の suffix を含む): {name}"
        ))
        .into());
    }
    let (explicit_kind, params) = match parts.get(1) {
        Some(token) if is_media_kind(token) => (Some(*token), &parts[2..]),
        _ => (None, &parts[1..]),
    };
    let kind = match explicit_kind {
        Some("video") => "video",
        Some("audio") => "audio",
        Some(other) => {
            return Err(ErrorMessage::new(format!(
                "--tracks の種別は video または audio で指定してください: {other}"
            ))
            .into());
        }
        None if is_media_kind(name) => name,
        None => {
            return Err(ErrorMessage::new(format!(
                "--tracks の種別が分かりません。名前:video:... または 名前:audio:... で指定してください: {entry}"
            ))
            .into());
        }
    };
    match kind {
        "video" => parse_video_track(name, params, entry),
        "audio" => parse_audio_track(name, params, entry),
        _ => Err(ErrorMessage::new(format!(
            "--tracks の種別は video または audio で指定してください: {entry}"
        ))
        .into()),
    }
}

/// トークンがメディア種別かどうか
fn is_media_kind(token: &str) -> bool {
    matches!(token, "video" | "audio")
}

/// 映像トラックを組み立てる
fn parse_video_track(name: &str, params: &[&str], entry: &str) -> Result<TrackSpec> {
    if params.len() > 3 {
        return Err(ErrorMessage::new(format!(
            "--tracks の映像は 名前[:video]:[framerate[:bitrate[:interval]]] です: {entry}"
        ))
        .into());
    }
    let framerate = match params.first() {
        Some(token) => parse_f64(token, "framerate", entry)?,
        None => DEFAULT_VIDEO_FRAMERATE,
    };
    let bitrate = match params.get(1) {
        Some(token) => parse_u64(token, "bitrate", entry)?,
        None => DEFAULT_VIDEO_BITRATE,
    };
    let interval = match params.get(2) {
        Some(token) => parse_f64(token, "interval", entry)?,
        None => DEFAULT_VIDEO_INTERVAL,
    };
    Ok(TrackSpec {
        name: name.to_string(),
        media: video_media(name, framerate, bitrate, interval)?,
    })
}

/// 音声トラックを組み立てる
fn parse_audio_track(name: &str, params: &[&str], entry: &str) -> Result<TrackSpec> {
    if params.len() > 2 {
        return Err(ErrorMessage::new(format!(
            "--tracks の音声は 名前[:audio]:[bitrate[:interval]] です: {entry}"
        ))
        .into());
    }
    let bitrate = match params.first() {
        Some(token) => parse_u64(token, "bitrate", entry)?,
        None => DEFAULT_AUDIO_BITRATE,
    };
    let interval = match params.get(1) {
        Some(token) => parse_f64(token, "interval", entry)?,
        None => DEFAULT_AUDIO_INTERVAL,
    };
    Ok(TrackSpec {
        name: name.to_string(),
        media: audio_media(name, bitrate, interval)?,
    })
}

/// 映像の Group / object サイズを決める
///
/// # Errors
///
/// フレームレート、ビットレート、Group 間隔が範囲外の場合、または
/// `framerate * interval` が整数フレームにならない場合はエラーになる。
fn video_media(name: &str, framerate: f64, bitrate: u64, interval: f64) -> Result<TrackMedia> {
    if !OBJECT_RATE_RANGE.contains(&framerate) {
        return Err(ErrorMessage::new(format!(
            "トラック '{name}' の framerate は {} から {} の範囲で指定してください",
            OBJECT_RATE_RANGE.start(),
            OBJECT_RATE_RANGE.end()
        ))
        .into());
    }
    if !interval.is_finite() || interval <= 0.0 || interval > MAX_VIDEO_INTERVAL {
        return Err(ErrorMessage::new(format!(
            "トラック '{name}' の interval は 0 より大きく {MAX_VIDEO_INTERVAL} 以下の秒数で指定してください"
        ))
        .into());
    }
    let frames = framerate * interval;
    let rounded = frames.round();
    if !rounded.is_finite()
        || (frames - rounded).abs() > FRAME_COUNT_EPSILON
        || rounded < 1.0
        || rounded > MAX_OBJECTS_PER_GROUP as f64
    {
        return Err(ErrorMessage::new(format!(
            "トラック '{name}' の framerate と interval の積が 1 から {MAX_OBJECTS_PER_GROUP} の整数フレームになりません: {frames}"
        ))
        .into());
    }
    let object_size = object_size_from_bitrate(name, bitrate, 1.0 / framerate)?;
    Ok(TrackMedia::Video {
        framerate,
        bitrate,
        interval,
        object_size,
        objects_per_group: rounded as u64,
    })
}

/// 音声の object サイズを決める
///
/// 1 チャンクが 1 Group になるため、Group あたりの object 数は 1 で固定する。
///
/// # Errors
///
/// ビットレートまたはチャンク間隔が範囲外の場合はエラーになる。
fn audio_media(name: &str, bitrate: u64, interval: f64) -> Result<TrackMedia> {
    if !AUDIO_INTERVAL_RANGE.contains(&interval) {
        return Err(ErrorMessage::new(format!(
            "トラック '{name}' の interval は {} から {} 秒の範囲で指定してください",
            AUDIO_INTERVAL_RANGE.start(),
            AUDIO_INTERVAL_RANGE.end()
        ))
        .into());
    }
    let rate = 1.0 / interval;
    if !OBJECT_RATE_RANGE.contains(&rate) {
        return Err(ErrorMessage::new(format!(
            "トラック '{name}' の送信レートが範囲外です: {rate}"
        ))
        .into());
    }
    let object_size = object_size_from_bitrate(name, bitrate, interval)?;
    Ok(TrackMedia::Audio {
        bitrate,
        interval,
        object_size,
    })
}

/// ビットレートと 1 object の秒数から payload サイズを求める
///
/// 実コーデックのフレームサイズではなく、平均ビットレートを均等に割ったダミーサイズである。
fn object_size_from_bitrate(name: &str, bitrate: u64, seconds: f64) -> Result<usize> {
    if bitrate == 0 {
        return Err(ErrorMessage::new(format!(
            "トラック '{name}' の bitrate は 1 以上で指定してください"
        ))
        .into());
    }
    let bytes = (bitrate as f64) * seconds / 8.0;
    let rounded = bytes.round();
    if !rounded.is_finite() || rounded < 1.0 || rounded > MAX_OBJECT_SIZE as f64 {
        return Err(ErrorMessage::new(format!(
            "トラック '{name}' の bitrate から求めた object サイズが 1 から {MAX_OBJECT_SIZE} の範囲にありません: {bytes}"
        ))
        .into());
    }
    Ok(rounded as usize)
}

/// 小数を CLI に戻す
///
/// 整数値は小数点を付けない。`30.0` を `30` にすると、JSONC から展開した文字列が
/// 人が書く `--tracks` と同じ形になる。
fn format_f64(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// CLI の数値トークンを `f64` として読む
fn parse_f64(token: &str, field: &str, entry: &str) -> Result<f64> {
    token.parse::<f64>().map_err(|_| {
        ErrorMessage::new(format!("--tracks の {field} が数値ではありません: {entry}")).into()
    })
}

/// CLI の数値トークンを `u64` として読む
fn parse_u64(token: &str, field: &str, entry: &str) -> Result<u64> {
    token.parse::<u64>().map_err(|_| {
        ErrorMessage::new(format!(
            "--tracks の {field} が 0 以上の整数ではありません: {entry}"
        ))
        .into()
    })
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

/// `tracks` 配列を `--tracks` の文字列へ変換する
///
/// 変換後の文字列は CLI と同じ解釈になる。未指定のパラメータは CLI の既定値を埋めてから
/// 文字列にする。
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
        specs.push(track_spec_from_json(i, element)?.to_cli());
    }
    argv.push("--tracks".to_string());
    argv.push(specs.join(","));
    Ok(())
}

/// JSONC のトラック 1 件を解釈する
fn track_spec_from_json(index: usize, element: RawJsonValue<'_, '_>) -> Result<TrackSpec> {
    let mut name: Option<String> = None;
    let mut kind: Option<&str> = None;
    let mut framerate: Option<f64> = None;
    let mut bitrate: Option<u64> = None;
    let mut interval: Option<f64> = None;
    let members = element
        .to_object()
        .map_err(|e| ErrorMessage::new(format!("tracks[{index}] のパースに失敗しました: {e}")))?;
    for (key_value, value) in members {
        let key: String = key_value
            .try_into()
            .map_err(|e: nojson::JsonParseError| ErrorMessage::new(format!("{e}")))?;
        match key.as_str() {
            "name" => {
                name = Some(json_string(index, "name", value)?);
            }
            "kind" => {
                let text = json_string(index, "kind", value)?;
                if !is_media_kind(&text) {
                    return Err(ErrorMessage::new(format!(
                        "tracks[{index}] の kind は video または audio で指定してください: {text}"
                    ))
                    .into());
                }
                kind = Some(if text == "video" { "video" } else { "audio" });
            }
            "framerate" => {
                framerate = Some(json_f64(index, "framerate", value)?);
            }
            "bitrate" => {
                bitrate = Some(json_u64(index, "bitrate", value)?);
            }
            "interval" => {
                interval = Some(json_f64(index, "interval", value)?);
            }
            "object-rate" | "object-size" | "delivery-mode" => {
                return Err(ErrorMessage::new(format!(
                    "tracks[{index}] の {key} は指定できません。映像は framerate / bitrate / interval、音声は bitrate / interval で指定してください"
                ))
                .into());
            }
            other => {
                return Err(ErrorMessage::new(format!(
                    "tracks[{index}] に未知のキーがあります: {other}"
                ))
                .into());
            }
        }
    }
    let Some(name) = name else {
        return Err(ErrorMessage::new(format!("tracks[{index}] に name がありません")).into());
    };
    // kind はループ内で 'static の "video" / "audio" だけを残している。
    // 名前からの推定はここでコピーし、以降 name を動かせるようにする。
    let resolved = match kind {
        Some("video") => "video",
        Some("audio") => "audio",
        Some(_) => {
            return Err(ErrorMessage::new(format!(
                "tracks[{index}] の kind は video または audio で指定してください"
            ))
            .into());
        }
        None if name == "video" || name == "audio" => {
            if name == "video" {
                "video"
            } else {
                "audio"
            }
        }
        None => {
            return Err(ErrorMessage::new(format!(
                "tracks[{index}] の種別が分かりません。kind を video または audio で指定してください"
            ))
            .into());
        }
    };
    match resolved {
        "video" => {
            let media = video_media(
                &name,
                framerate.unwrap_or(DEFAULT_VIDEO_FRAMERATE),
                bitrate.unwrap_or(DEFAULT_VIDEO_BITRATE),
                interval.unwrap_or(DEFAULT_VIDEO_INTERVAL),
            )?;
            Ok(TrackSpec { name, media })
        }
        "audio" => {
            if framerate.is_some() {
                return Err(ErrorMessage::new(format!(
                    "tracks[{index}] の音声に framerate は指定できません"
                ))
                .into());
            }
            let media = audio_media(
                &name,
                bitrate.unwrap_or(DEFAULT_AUDIO_BITRATE),
                interval.unwrap_or(DEFAULT_AUDIO_INTERVAL),
            )?;
            Ok(TrackSpec { name, media })
        }
        _ => Err(ErrorMessage::new(format!(
            "tracks[{index}] の kind は video または audio で指定してください"
        ))
        .into()),
    }
}

/// JSON の文字列メンバーを読む
fn json_string(index: usize, field: &str, value: RawJsonValue<'_, '_>) -> Result<String> {
    value.try_into().map_err(|_| {
        ErrorMessage::new(format!(
            "tracks[{index}] の {field} が文字列ではありません: {}",
            value.as_raw_str()
        ))
        .into()
    })
}

/// JSON の数値メンバーを `f64` として読む
fn json_f64(index: usize, field: &str, value: RawJsonValue<'_, '_>) -> Result<f64> {
    value.try_into().map_err(|_| {
        ErrorMessage::new(format!(
            "tracks[{index}] の {field} が数値ではありません: {}",
            value.as_raw_str()
        ))
        .into()
    })
}

/// JSON の数値メンバーを `u64` として読む
fn json_u64(index: usize, field: &str, value: RawJsonValue<'_, '_>) -> Result<u64> {
    value.try_into().map_err(|_| {
        ErrorMessage::new(format!(
            "tracks[{index}] の {field} が 0 以上の整数ではありません: {}",
            value.as_raw_str()
        ))
        .into()
    })
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

    /// 映像は Group を interval で切り、音声は 1 object を 1 Group にすること
    #[test]
    fn parse_tracks_reads_video_and_audio() {
        let tracks =
            parse_tracks("video:30:1000000:2,audio:24000:0.02").expect("解釈に成功すること");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].name, "video");
        assert_eq!(tracks[0].media.object_rate(), 30.0);
        // 1_000_000 bit/s / 30 fps / 8 = 4166.666... を 4167 バイトに丸める
        assert_eq!(tracks[0].media.object_size(), 4167);
        assert_eq!(tracks[0].media.objects_per_group(), 60);
        assert_eq!(tracks[0].media.delivery(), DeliveryMode::Subgroup);
        assert_eq!(tracks[1].name, "audio");
        assert_eq!(tracks[1].media.object_rate(), 50.0);
        assert_eq!(tracks[1].media.object_size(), 60);
        assert_eq!(tracks[1].media.objects_per_group(), 1);
        assert_eq!(tracks[1].media.delivery(), DeliveryMode::Datagram);
        assert_eq!(tracks[1].media.delivery().as_str(), "datagram");
    }

    /// 名前が video / audio 以外のときは種別を明示すること
    #[test]
    fn parse_tracks_reads_explicit_kind() {
        let tracks = parse_tracks("screen:video:15:500000:2").expect("解釈に成功すること");
        assert_eq!(tracks[0].name, "screen");
        assert_eq!(tracks[0].media.object_rate(), 15.0);
        assert_eq!(tracks[0].media.objects_per_group(), 30);
        assert_eq!(tracks[0].media.delivery(), DeliveryMode::Subgroup);
    }

    /// パラメータを省略するとメディア種別の既定値になること
    #[test]
    fn parse_tracks_uses_defaults() {
        let video = parse_tracks("video").expect("解釈に成功すること");
        assert_eq!(video[0].media.object_rate(), DEFAULT_VIDEO_FRAMERATE);
        assert_eq!(video[0].media.objects_per_group(), 60);
        assert_eq!(video[0].media.delivery(), DeliveryMode::Subgroup);

        let audio = parse_tracks("audio").expect("解釈に成功すること");
        assert_eq!(audio[0].media.object_rate(), 50.0);
        assert_eq!(audio[0].media.object_size(), 60);
        assert_eq!(audio[0].media.objects_per_group(), 1);
        assert_eq!(audio[0].media.delivery(), DeliveryMode::Datagram);
    }

    /// framerate と interval の積が整数フレームにならない映像は拒否すること
    #[test]
    fn parse_tracks_rejects_non_integer_frame_count() {
        assert!(parse_tracks("video:29.97:1000000:2").is_err());
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
                MoqTransport::Quic,
                "zakuro",
                Some("video"),
                DEFAULT_DATAGRAM_MAX_SIZE,
                None,
                false,
                None,
                1,
                vcs_hatch_rate,
                duration,
                repeat_interval,
                0,
                retry_interval,
                true,
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
            "tracks": [ { "name": "video", "framerate": "abc" } ]
        }"#;
        assert!(
            parse_jsonc_config(invalid_rate).is_err(),
            "framerate の型不正を許容してはならない"
        );

        let negative_bitrate = r#"{
            "url": "moqt://relay.example.com:4433",
            "tracks": [ { "name": "video", "bitrate": -5 } ]
        }"#;
        assert!(
            parse_jsonc_config(negative_bitrate).is_err(),
            "bitrate の負値を許容してはならない"
        );

        let removed_key = r#"{
            "url": "moqt://relay.example.com:4433",
            "tracks": [ { "name": "video", "object-rate": 30 } ]
        }"#;
        assert!(
            parse_jsonc_config(removed_key).is_err(),
            "object-rate を許容してはならない"
        );

        let missing_name = r#"{
            "url": "moqt://relay.example.com:4433",
            "tracks": [ { "framerate": 30 } ]
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
                { "name": "video", "framerate": 30, "bitrate": 1000000, "interval": 2 },
                { "name": "mic", "kind": "audio", "bitrate": 24000, "interval": 0.02 }
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
            "video:video:30:1000000:2,mic:audio:24000:0.02"
        );
        let tracks = parse_tracks(&argv[tracks_pos + 1]).expect("展開結果を再解釈できること");
        assert_eq!(tracks[0].media.objects_per_group(), 60);
        assert_eq!(tracks[1].media.objects_per_group(), 1);
        assert_eq!(tracks[1].media.delivery(), DeliveryMode::Datagram);
    }

    /// JSONC の音声トラックは framerate を受け取らないこと
    #[test]
    fn jsonc_audio_rejects_framerate() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "tracks": [ { "name": "audio", "framerate": 50 } ]
        }"#;
        assert!(
            parse_jsonc_config(content).is_err(),
            "音声の framerate を許容してはならない"
        );
    }

    /// datagram 配送の object が上限に収まるかを起動時に検証すること
    ///
    /// datagram は分割できず、上限を超えた object は通知なく破棄される
    /// (draft-ietf-moq-transport-23 §11.2 (Datagrams))。起動時にエラーにする。
    #[test]
    fn validate_rejects_datagram_object_that_does_not_fit() {
        let validate = |tracks: Option<&str>, datagram_max_size: usize| {
            validate_instance_args(
                "moqt://relay.example.com:4433",
                MoqTransport::Quic,
                "zakuro",
                tracks,
                datagram_max_size,
                None,
                false,
                None,
                1,
                1.0,
                None,
                None,
                0,
                60.0,
                true,
            )
        };

        // 既定の上限に収まる音声相当のサイズは通る (24 kbit/s * 20 ms = 60 バイト)
        assert!(
            validate(Some("audio:24000:0.02"), DEFAULT_DATAGRAM_MAX_SIZE).is_ok(),
            "上限に収まる datagram トラックを拒否してはならない"
        );
        // ヘッダ込みで上限を超えるサイズは拒否する (480 kbit/s * 20 ms = 1200 バイト)
        assert!(
            validate(Some("audio:480000:0.02"), DEFAULT_DATAGRAM_MAX_SIZE).is_err(),
            "ヘッダ込みで上限を超える datagram トラックを許容してはならない"
        );
        // 映像は subgroup なので datagram の下限でも拒否しない
        assert!(
            validate(Some("video"), 1).is_ok(),
            "映像トラックを datagram の上限で拒否してはならない"
        );
        // 上限の範囲外は拒否する
        assert!(validate(Some("audio"), 0).is_err());
        assert!(validate(Some("audio"), 65536).is_err());
    }

    /// `--datagram-max-size` を解釈できること
    #[test]
    fn datagram_max_size_is_parsed() {
        let (instance, _help) = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url=moqt://relay.example.com:4433".to_string(),
                "--tracks=audio".to_string(),
                "--datagram-max-size=1200".to_string(),
            ],
            &mut Vec::new(),
        )
        .expect("解釈に成功すること");
        assert_eq!(instance.datagram_max_size, 1200);
        assert_eq!(instance.tracks[0].media.delivery(), DeliveryMode::Datagram);

        let (default_instance, _help) = parse_instance_args(
            "zakuro-moq",
            vec!["--url=moqt://relay.example.com:4433".to_string()],
            &mut Vec::new(),
        )
        .expect("解釈に成功すること");
        assert_eq!(
            default_instance.datagram_max_size,
            DEFAULT_DATAGRAM_MAX_SIZE
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

    /// `--reconnect-on-goaway` は既定で有効になること
    #[test]
    fn reconnect_on_goaway_defaults_to_true() {
        let (instance, _help) = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url".to_string(),
                "moqt://relay.example.com:4433".to_string(),
                "--tracks".to_string(),
                "video".to_string(),
            ],
            &mut Vec::new(),
        )
        .expect("解釈に成功すること");
        assert!(instance.reconnect_on_goaway);
    }

    /// `--reconnect-on-goaway false` で無効にできること
    #[test]
    fn reconnect_on_goaway_can_be_disabled() {
        let (instance, _help) = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url".to_string(),
                "moqt://relay.example.com:4433".to_string(),
                "--reconnect-on-goaway".to_string(),
                "false".to_string(),
            ],
            &mut Vec::new(),
        )
        .expect("解釈に成功すること");
        assert!(!instance.reconnect_on_goaway);
    }

    /// `--reconnect-on-goaway=false` の `--key=value` 形式でも解釈できること
    #[test]
    fn reconnect_on_goaway_accepts_equal_form() {
        let (instance, _help) = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url=moqt://relay.example.com:4433".to_string(),
                "--reconnect-on-goaway=false".to_string(),
            ],
            &mut Vec::new(),
        )
        .expect("解釈に成功すること");
        assert!(!instance.reconnect_on_goaway);
    }

    /// `--reconnect-on-goaway` に true / false 以外は指定できないこと
    #[test]
    fn reconnect_on_goaway_rejects_invalid_value() {
        let result = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url".to_string(),
                "moqt://relay.example.com:4433".to_string(),
                "--reconnect-on-goaway".to_string(),
                "yes".to_string(),
            ],
            &mut Vec::new(),
        );
        assert!(result.is_err());
    }

    /// JSONC の reconnect-on-goaway が値付きオプションとして展開されること
    ///
    /// 値なしフラグとして扱うと `true` / `false` が値として積まれず、起動時に
    /// unexpected argument になる。
    #[test]
    fn jsonc_reconnect_on_goaway_is_value_option() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "subscribe-tracks": ["video"],
            "reconnect-on-goaway": false
        }"#;
        let config = parse_jsonc_config(content).expect("パースに成功すること");
        let argv = &config.instance_argvs[0];
        let pos = argv
            .iter()
            .position(|s| s == "--reconnect-on-goaway")
            .expect("--reconnect-on-goaway があること");
        assert_eq!(argv[pos + 1], "false");
    }

    /// `--transport` を省略した場合は QUIC 直接接続になること
    #[test]
    fn transport_defaults_to_quic() {
        let (instance, _help) = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url".to_string(),
                "moqt://relay.example.com:4433".to_string(),
            ],
            &mut Vec::new(),
        )
        .expect("解釈に成功すること");
        assert_eq!(instance.transport, MoqTransport::Quic);
    }

    /// WebTransport over HTTP/3 を CLI と JSONC の双方で選べること
    #[test]
    fn webtransport_selects_wt_h3() {
        let (instance, _) = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url=moqt://localhost/moq?room=test".into(),
                "--transport=wt-h3".into(),
            ],
            &mut Vec::new(),
        )
        .expect("WT 経路を選べること");
        assert_eq!(instance.transport, MoqTransport::WtH3);

        let config = parse_jsonc_config(r#"{"url":"moqt://localhost/", "transport":"wt-h3"}"#)
            .expect("JSONC を解釈できること");
        let (instance, _) = parse_instance_args(
            "zakuro-moq",
            config.instance_argvs[0].clone(),
            &mut Vec::new(),
        )
        .expect("JSONC から WT 経路を選べること");
        assert_eq!(instance.transport, MoqTransport::WtH3);
    }

    /// `--transport wt-h2` と JSONC の `"transport": "wt-h2"` を解釈できること
    ///
    /// WebTransport over HTTP/2 は TCP + TLS 上で動くため QUIC を使わない。
    #[test]
    fn transport_accepts_wt_h2() {
        for options in [vec!["--transport=wt-h2"], vec!["--transport", "wt-h2"]] {
            let mut argv: Vec<String> = vec!["--url=moqt://relay.example.com:4433".into()];
            argv.extend(options.into_iter().map(str::to_owned));
            let (instance, _help) = parse_instance_args("zakuro-moq", argv, &mut Vec::new())
                .expect("解釈に成功すること");
            assert_eq!(instance.transport, MoqTransport::WtH2);
        }
        let config = parse_jsonc_config(r#"{"url":"moqt://localhost/", "transport":"wt-h2"}"#)
            .expect("JSONC を解釈できること");
        let (instance, _) = parse_instance_args(
            "zakuro-moq",
            config.instance_argvs[0].clone(),
            &mut Vec::new(),
        )
        .expect("JSONC から wt-h2 経路を選べること");
        assert_eq!(instance.transport, MoqTransport::WtH2);
    }

    /// `--transport qmux` と `--transport=qmux` を解釈できること
    #[test]
    fn transport_accepts_qmux() {
        for argv in [
            vec![
                "--url".to_string(),
                "moqt://relay.example.com:4433".to_string(),
                "--transport".to_string(),
                "qmux".to_string(),
            ],
            vec![
                "--url=moqt://relay.example.com:4433".to_string(),
                "--transport=qmux".to_string(),
            ],
        ] {
            let (instance, _help) = parse_instance_args("zakuro-moq", argv, &mut Vec::new())
                .expect("解釈に成功すること");
            assert_eq!(instance.transport, MoqTransport::Qmux);
        }
    }

    /// 未知のトランスポートは指定できないこと
    #[test]
    fn transport_rejects_unknown_value() {
        let result = parse_instance_args(
            "zakuro-moq",
            vec![
                "--url".to_string(),
                "moqt://relay.example.com:4433".to_string(),
                "--transport".to_string(),
                "unknown".to_string(),
            ],
            &mut Vec::new(),
        );
        assert!(result.is_err());
    }

    /// JSONC の transport が値付きオプションとして展開されること
    #[test]
    fn jsonc_transport_is_value_option() {
        let content = r#"{
            "url": "moqt://relay.example.com:4433",
            "transport": "qmux"
        }"#;
        let config = parse_jsonc_config(content).expect("パースに成功すること");
        let argv = &config.instance_argvs[0];
        let pos = argv
            .iter()
            .position(|s| s == "--transport")
            .expect("--transport があること");
        assert_eq!(argv[pos + 1], "qmux");
    }
}
