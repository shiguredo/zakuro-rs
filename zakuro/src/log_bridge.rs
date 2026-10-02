//! `log` クレートのファサードを libwebrtc のログ出力へ転送する
//!
//! 共有クレート (`zakuro-core`) はライブラリとして `log` ファサードを使う。出力先と形式は
//! バイナリ側が決めるため、このバイナリでは `shiguredo_webrtc::log::print` へ転送し、
//! 既存のログ形式 (`[時刻][スレッド] (file:line): message`) と `--log-level` の絞り込みを
//! そのまま保つ。

use log::{Level, LevelFilter, Log, Metadata, Record};
use shiguredo_webrtc::log::{Severity, print};

/// libwebrtc のログ出力へ転送するロガー
struct WebRtcLogger;

impl Log for WebRtcLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        // zakuro のクレートからの記録だけを転送する。グローバルロガーを立てると依存
        // クレート (例: sora_sdk 経由の rustls-platform-verifier) の log 記録も拾ってしまい、
        // 従来は出ていなかったログが混ざるため、target で絞る
        if !metadata.target().starts_with("zakuro") {
            return false;
        }
        // `--log-level` で設定した最大レベルで絞り込む
        // (libwebrtc 側の min_severity とは別に、共有クレートのログにも同じ基準を適用する)
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let severity = match record.level() {
            Level::Error => Severity::Error,
            Level::Warn => Severity::Warning,
            Level::Info => Severity::Info,
            Level::Debug | Level::Trace => Severity::Verbose,
        };
        // `rtc_log_*` マクロと同じ `crate::file.rs` 形式にする。`log` の Record には
        // モジュールパス (target) とファイルパスが入っているため、target の先頭
        // (クレート名) とファイル名を組み合わせる
        let file = format_file(record.target(), record.file().unwrap_or("unknown"));
        print(
            severity,
            &file,
            record.line().unwrap_or(0) as i32,
            &record.args().to_string(),
        );
    }

    fn flush(&self) {}
}

/// `rtc_log_format_file` と同じ `crate::file.rs` 形式へ整える
fn format_file(target: &str, file: &str) -> String {
    let crate_name = target.split("::").next().unwrap_or(target);
    let file_name = std::path::Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file);
    format!("{crate_name}::{file_name}")
}

/// ロガーの実体 (log::set_logger は 'static な参照を要求する)
static LOGGER: WebRtcLogger = WebRtcLogger;

/// `log` ファサードの出力先を libwebrtc のログ出力にする
///
/// プロセスで 1 回だけ呼ぶ。2 回目以降は無視される (`log::set_logger` が失敗する)。
pub(crate) fn install(max_level: LevelFilter) {
    if log::set_logger(&LOGGER).is_err() {
        // 既に別のロガーが設定済み (テストなど)。この場合 zakuro-core のログはそのロガーへ
        // 流れるため、フィルタだけは揃えておく
        log::set_max_level(max_level);
        return;
    }
    log::set_max_level(max_level);
}

/// libwebrtc の `Severity` を `log` の最大レベルへ変換する
///
/// `--log-level` は libwebrtc のログに使う値だが、共有クレートのログにも同じ基準を
/// 適用するために流用する。
pub(crate) fn max_level_for(severity: Severity) -> LevelFilter {
    match severity {
        Severity::Verbose => LevelFilter::Debug,
        Severity::Info => LevelFilter::Info,
        Severity::Warning => LevelFilter::Warn,
        Severity::Error => LevelFilter::Error,
        Severity::None => LevelFilter::Off,
        // CLI からは Raw を設定しない
        Severity::Raw(_) => LevelFilter::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--log-level` の値が同じ意味のフィルタへ対応すること
    #[test]
    fn max_level_matches_severity() {
        assert_eq!(max_level_for(Severity::Verbose), LevelFilter::Debug);
        assert_eq!(max_level_for(Severity::Info), LevelFilter::Info);
        assert_eq!(max_level_for(Severity::Warning), LevelFilter::Warn);
        assert_eq!(max_level_for(Severity::Error), LevelFilter::Error);
        assert_eq!(max_level_for(Severity::None), LevelFilter::Off);
    }

    /// ログのファイル表示が `crate::file.rs` 形式になること
    #[test]
    fn format_file_matches_rtc_log_style() {
        assert_eq!(
            format_file("zakuro_core::stats", "zakuro-core/src/stats.rs"),
            "zakuro_core::stats.rs"
        );
        assert_eq!(
            format_file("zakuro::main", "zakuro/src/main.rs"),
            "zakuro::main.rs"
        );
        // パスが無い場合もそのまま使う
        assert_eq!(format_file("a::b", "b.rs"), "a::b.rs");
    }
}
