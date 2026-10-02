//! ログ出力の初期化
//!
//! ログは `tracing` を使う。共有クレート (`zakuro-core`) は `log` ファサードを使うため、
//! `tracing-log` で `log` の記録も tracing へ取り込む。

use tracing_subscriber::filter::LevelFilter;

/// `--log-level` の値
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogLevel {
    /// 詳細ログ
    Verbose,
    /// 通常ログ (既定)
    Info,
    /// 警告以上
    Warning,
    /// エラー以上
    Error,
    /// 出力しない
    None,
}

impl LogLevel {
    /// コマンドライン / JSONC の文字列から解釈する
    pub(crate) fn parse(value: &str) -> std::result::Result<Self, &'static str> {
        match value {
            "verbose" => Ok(Self::Verbose),
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            "none" => Ok(Self::None),
            _ => Err("verbose / info / warning / error / none のいずれかを指定してください"),
        }
    }

    /// tracing のレベルフィルタへ変換する
    fn to_filter(self) -> LevelFilter {
        match self {
            Self::Verbose => LevelFilter::DEBUG,
            Self::Info => LevelFilter::INFO,
            Self::Warning => LevelFilter::WARN,
            Self::Error => LevelFilter::ERROR,
            Self::None => LevelFilter::OFF,
        }
    }
}

/// ログ出力を初期化する
///
/// プロセスで 1 回だけ呼ぶ。`log` ファサードの記録 (zakuro-core のログ) も
/// `tracing-log` 経由で同じ出力へ流す。
pub(crate) fn init(level: LogLevel) {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level.to_filter())
        .with_target(true)
        .finish();
    // 既に設定済みの場合 (テストなど) は無視する
    let _ = tracing::subscriber::set_global_default(subscriber);
    let _ = tracing_log::LogTracer::init();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文字列からログレベルを解釈できること
    #[test]
    fn parse_accepts_known_levels() {
        assert_eq!(LogLevel::parse("verbose"), Ok(LogLevel::Verbose));
        assert_eq!(LogLevel::parse("info"), Ok(LogLevel::Info));
        assert_eq!(LogLevel::parse("warning"), Ok(LogLevel::Warning));
        assert_eq!(LogLevel::parse("error"), Ok(LogLevel::Error));
        assert_eq!(LogLevel::parse("none"), Ok(LogLevel::None));
    }

    /// 未知の文字列はエラーになること
    #[test]
    fn parse_rejects_unknown_level() {
        assert!(LogLevel::parse("debug").is_err());
    }

    /// レベルが tracing のフィルタへ対応すること
    #[test]
    fn filters_match_levels() {
        assert_eq!(LogLevel::Verbose.to_filter(), LevelFilter::DEBUG);
        assert_eq!(LogLevel::Info.to_filter(), LevelFilter::INFO);
        assert_eq!(LogLevel::Warning.to_filter(), LevelFilter::WARN);
        assert_eq!(LogLevel::Error.to_filter(), LevelFilter::ERROR);
        assert_eq!(LogLevel::None.to_filter(), LevelFilter::OFF);
    }
}
