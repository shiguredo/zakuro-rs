//! libwebrtc / sora-rust-sdk のログを抑制する LogSink
//!
//! 抑制する対象は次の 2 つ。
//!
//! 1. dummy ADM が出す無害なエラーログ `failed to retrieve the playout delay`
//!
//!    libwebrtc の `AudioDeviceModuleImpl::PlayoutDelay()` (`modules/audio_device/
//!    audio_device_impl.cc`) は、下位の音声デバイスが playout delay を返せないと
//!    LS_ERROR でこのメッセージを出力する。zakuro は受信音声を再生しないため受信側
//!    インスタンスを `AdmConfig::NoAudioDevice` で動かしており、sora-sdk はこのとき
//!    dummy ADM (`AudioDeviceModuleAudioLayer::Dummy`) を作る。dummy ADM の
//!    `PlayoutDelay()` は常に -1 を返す実装なので、受信音声チャネルを持つ VC ごとに
//!    このエラーログが出続ける。値そのものは zakuro では使われないため無害であり、
//!    このメッセージはコード組み込みで常に抑制する
//!
//! 2. `--log-suppress` / JSONC `"log-suppress"` で運用者が指定した文字列に一致するログ
//!
//!    メッセージ本体または発生元ファイル名の部分一致で判定する。抑制するかどうかは
//!    運用者の判断であり、既定では何も指定されない
//!
//! `--log-level` では消せない (dummy ADM のメッセージは LS_ERROR のため error 指定でも
//! 残り、運用者指定の対象は INFO 以上に混在する) ので、libwebrtc の LogSink で行単位に
//! 捨てる。ただし stderr への直接出力は sink とは別経路 (`rtc_base/logging.cc` の
//! `LogMessage::~LogMessage()`) のため、`set_log_to_stderr(false)` で止めた上で、この sink が
//! 抑制対象以外の全行を `default_log_line()` で再出力する。呼び出し側の設定は main.rs の
//! ログ初期化ブロックを参照すること。
//!
//! この構成の制約:
//!
//! - sink の min severity は LS_INFO 固定 (`webrtc::LogSink` の private メンバで
//!   C API に setter がない) のため、`--log-level=verbose` の verbose 行は sink に
//!   届かず、この構成では出力されない
//! - `--log-level` の絞り込みは sink の配信条件に反映されないため、このモジュール側で
//!   改めて絞り込む (`meets_min_severity` のコメントを参照)

use std::io::Write;

use shiguredo_webrtc::log::{LogLineRef, LogSink, LogSinkHandler, Severity};

/// dummy ADM が playout delay を取得できずに出すエラーメッセージ
///
/// `LogLineRef::message()` は末尾に改行を含むため、比較時は trim する。
const DUMMY_PLAYOUT_DELAY_MESSAGE: &str = "failed to retrieve the playout delay";

/// dummy ADM 由来の無害なログかどうかを判定する
///
/// メッセージの完全一致で判定する。このメッセージを出せるのは dummy ADM だけであり、
/// zakuro で dummy ADM が使われるのは `AdmConfig::NoAudioDevice` のインスタンスだけの
/// ため、起動引数による切り替えは不要。将来 `AdmConfig::UseBuiltIn` (実デバイス) を
/// 使う構成にした場合は、実障害による同メッセージまで隠さないようここを見直すこと。
fn is_dummy_playout_delay(message: &str) -> bool {
    message.trim_end() == DUMMY_PLAYOUT_DELAY_MESSAGE
}

/// `--log-suppress` で指定された文字列のいずれかを含むかどうかを判定する
///
/// 部分一致で判定する。ファイル名 (`transport_feedback_adapter.cc` など) を指定すると、
/// そのファイルから出るログをまとめて抑制できる。
fn matches_suppress_patterns(text: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| text.contains(pattern.as_str()))
}

/// `--log-level` で指定した最低重大度を満たすかどうかを判定する
///
/// libwebrtc の sink は配信条件に `webrtc::LogSink::min_severity_` (LS_INFO 固定) を
/// 使うため、`--log-level=warning` 以上を指定しても LS_INFO / LS_WARNING の行が sink へ
/// 届く。ここで改めて絞り込まないと `--log-level` の指定より軽い行が出力されてしまう。
///
/// `Severity` は `Ord` を実装していないため、libwebrtc の列挙値 (LS_VERBOSE=0 <
/// LS_INFO=1 < LS_WARNING=2 < LS_ERROR=3 < LS_NONE=4) の大小で比較する。
fn meets_min_severity(severity: Severity, min_severity: Severity) -> bool {
    severity.to_int() >= min_severity.to_int()
}

/// 抑制対象以外を stderr へ再出力する sink ハンドラ
///
/// ログ初期化前に生成して以降は書き換えない設定だけを持ち、可変状態は持たない。
/// sink は複数のスレッドから呼ばれ得るため、判定は毎回この設定だけから行う。
struct SuppressingLogHandler {
    /// `--log-level` で指定された最低重大度
    min_severity: Severity,
    /// `--log-suppress` で指定された部分文字列
    suppress_patterns: Vec<String>,
}

impl LogSinkHandler for SuppressingLogHandler {
    fn on_log_message(&mut self, line: LogLineRef<'_>) {
        if !meets_min_severity(line.severity(), self.min_severity) {
            return;
        }
        // 抑制対象は `default_log_line()` を呼ぶ前に落とす。dummy ADM のメッセージは
        // 全ログの大半を占めるため、整形と stderr への書き込みを省くことがそのまま
        // 負荷削減になる
        if is_dummy_playout_delay(line.message()) {
            return;
        }
        if matches_suppress_patterns(line.message(), &self.suppress_patterns)
            || matches_suppress_patterns(line.filename(), &self.suppress_patterns)
        {
            return;
        }
        let Ok(text) = line.default_log_line() else {
            return;
        };
        // 従来の stderr 出力と同じ見た目のまま書き戻す
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    }
}

/// ログ初期化時に LoggingConfig へ追加する sink を作る
///
/// `min_severity` には `set_min_severity` / `set_debug_severity` へ渡すのと同じ値を
/// 渡すこと。片方だけ変えると、`--log-level` の指定と sink の再出力の絞り込みがずれる。
/// `suppress_patterns` は `--log-suppress` / JSONC `"log-suppress"` の値で、空なら
/// dummy ADM の組み込み抑制だけが有効になる。
pub(crate) fn build_sink(min_severity: Severity, suppress_patterns: &[String]) -> LogSink {
    LogSink::new_with_handler(Box::new(SuppressingLogHandler {
        min_severity,
        suppress_patterns: suppress_patterns.to_vec(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dummy ADM のメッセージだけを抑制すること
    #[test]
    fn suppresses_dummy_playout_delay_message() {
        // 実際は末尾に改行付きで届くため、改行あり / なしの両方で一致すること
        assert!(
            is_dummy_playout_delay("failed to retrieve the playout delay\n"),
            "末尾改行付きの dummy ADM のメッセージは抑制対象であるべき"
        );
        assert!(
            is_dummy_playout_delay("failed to retrieve the playout delay"),
            "末尾改行なしでも抑制対象であるべき"
        );
    }

    /// 完全一致で判定し、似た別のログを巻き込まないこと
    #[test]
    fn keeps_other_messages() {
        assert!(
            !is_dummy_playout_delay("failed to retrieve the playout delay from the real device\n"),
            "前方一致で別のメッセージを巻き込んではならない"
        );
        assert!(
            !is_dummy_playout_delay("some other error\n"),
            "無関係なメッセージは抑制対象ではない"
        );
        assert!(
            !is_dummy_playout_delay("failed to retrieve the playout delay: retry\n"),
            "後方一致で別のメッセージを巻き込んではならない"
        );
    }

    /// `--log-suppress` の指定がメッセージの部分一致で効くこと
    #[test]
    fn suppresses_messages_matching_patterns() {
        let patterns = vec![
            "Failed to lookup send time for packet".to_string(),
            "Packet buffer fully flushed.".to_string(),
        ];
        assert!(
            matches_suppress_patterns(
                "Failed to lookup send time for packet with 1234. Send time history too small\n",
                &patterns
            ),
            "メッセージの途中に含まれる指定でも抑制されるべき"
        );
        assert!(
            matches_suppress_patterns("Packet buffer fully flushed.\n", &patterns),
            "メッセージ全体の指定でも抑制されるべき"
        );
        assert!(
            !matches_suppress_patterns("Packet received on SSRC: 1234\n", &patterns),
            "指定に一致しないメッセージは抑制してはならない"
        );
    }

    /// `--log-suppress` の指定がファイル名の部分一致でも効くこと
    #[test]
    fn suppresses_messages_matching_filename_pattern() {
        // ファイル名全体を指定した場合
        let full = vec!["transport_feedback_adapter.cc".to_string()];
        assert!(
            matches_suppress_patterns("transport_feedback_adapter.cc", &full),
            "ファイル名の指定で抑制されるべき"
        );
        assert!(
            !matches_suppress_patterns("packet_buffer.cc", &full),
            "別のファイル名は抑制してはならない"
        );
        // ファイル名の一部を指定した場合
        let partial = vec!["transport_feedback_adapter".to_string()];
        assert!(
            matches_suppress_patterns("transport_feedback_adapter.cc", &partial),
            "ファイル名に部分一致する指定でも抑制されるべき"
        );
    }

    /// 抑制パターンが空なら何も抑制しないこと
    #[test]
    fn keeps_all_messages_without_patterns() {
        let patterns: Vec<String> = Vec::new();
        assert!(
            !matches_suppress_patterns("Packet buffer fully flushed.\n", &patterns),
            "パターン未指定では何も抑制してはならない"
        );
        assert!(
            !matches_suppress_patterns("packet_buffer.cc", &patterns),
            "パターン未指定ではファイル名でも抑制してはならない"
        );
    }

    /// `--log-level` の指定どおりに絞り込まれること
    #[test]
    fn meets_min_severity_filters_by_log_level() {
        // info 指定では verbose を落とし、info 以上を通すこと
        assert!(
            meets_min_severity(Severity::Info, Severity::Info),
            "info 指定で info を落としてはならない"
        );
        assert!(
            !meets_min_severity(Severity::Verbose, Severity::Info),
            "info 指定で verbose を出してはならない"
        );
        // warning 指定では info を落とすこと (sink の min severity が LS_INFO のため、
        // この絞り込みが無いと info が漏れる)
        assert!(
            !meets_min_severity(Severity::Info, Severity::Warning),
            "warning 指定で info を出してはならない"
        );
        assert!(
            meets_min_severity(Severity::Warning, Severity::Warning),
            "warning 指定で warning を落としてはならない"
        );
        // error 指定では warning を落とすこと
        assert!(
            !meets_min_severity(Severity::Warning, Severity::Error),
            "error 指定で warning を出してはならない"
        );
        assert!(
            meets_min_severity(Severity::Error, Severity::Error),
            "error 指定で error を落としてはならない"
        );
        // none 指定では何も出さないこと
        assert!(
            !meets_min_severity(Severity::Error, Severity::None),
            "none 指定で error を出してはならない"
        );
        // verbose 指定では verbose 以上を通すこと (sink へは LS_INFO 以上しか
        // 届かないため verbose 自体は出力されないが、判定としては通す)
        assert!(
            meets_min_severity(Severity::Verbose, Severity::Verbose),
            "verbose 指定で verbose を落としてはならない"
        );
    }
}
