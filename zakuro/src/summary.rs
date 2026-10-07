//! 試験全体の集計結果の出力
//!
//! `StatsSummary` を人が読める形でログへ出し、`--summary-json` が指定されていれば
//! 同じ内容を JSON ファイルへ書く。JSON は CI から読むことを想定している。

use std::path::Path;

use zakuro_core::stats::StatsSummary;

/// 集計結果をログへ出す
pub(crate) fn log_summary(summary: &StatsSummary) {
    log::info!(
        "[summary] connections: success={} failure={} unjudged={} stalled={}",
        summary.success,
        summary.failure,
        summary.unjudged,
        summary.stalled,
    );
    match summary.success_rate() {
        Some(rate) => log::info!(
            "[summary] success rate: {:.4} ({} / {})",
            rate,
            summary.success,
            summary.judged(),
        ),
        None => log::info!("[summary] success rate: - (no judged connections)"),
    }
    for (reason, count) in &summary.failure_reasons {
        log::info!("[summary] failure reason: {} = {}", reason, count);
    }
    if summary.warmup_excluded > 0 {
        log::info!(
            "[summary] excluded from the summary (warmup): {}",
            summary.warmup_excluded,
        );
    }
    log::info!(
        "[summary] connect time (ms): p50={} p95={} p99={}",
        format_optional_ms(summary.connect_time_p50_ms),
        format_optional_ms(summary.connect_time_p95_ms),
        format_optional_ms(summary.connect_time_p99_ms),
    );
}

/// 集計結果を JSON ファイルへ書く
///
/// 値はすべて数値と、こちらで決めた ASCII の識別子だけなので、手で組み立てる。
pub(crate) fn write_summary_json(path: &Path, summary: &StatsSummary) -> std::io::Result<()> {
    // 失敗理由が無い場合は空配列にする (要素がある場合だけ改行して並べる)
    let reasons = if summary.failure_reasons.is_empty() {
        "[]".to_string()
    } else {
        let mut s = String::from("[");
        for (index, (reason, count)) in summary.failure_reasons.iter().enumerate() {
            if index > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "\n    {{\"reason\":\"{}\",\"count\":{}}}",
                reason, count
            ));
        }
        s.push_str("\n  ]");
        s
    };
    let json = format!(
        "{{\n  \"success\": {},\n  \"failure\": {},\n  \"unjudged\": {},\n  \
         \"judged\": {},\n  \"success_rate\": {},\n  \"stalled\": {},\n  \
         \"warmup_excluded\": {},\n  \
         \"failure_reasons\": {},\n  \
         \"connect_time_ms\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}}}\n}}\n",
        summary.success,
        summary.failure,
        summary.unjudged,
        summary.judged(),
        format_optional_rate(summary.success_rate()),
        summary.stalled,
        summary.warmup_excluded,
        reasons,
        format_optional_ms(summary.connect_time_p50_ms),
        format_optional_ms(summary.connect_time_p95_ms),
        format_optional_ms(summary.connect_time_p99_ms),
    );
    std::fs::write(path, json)
}

/// ミリ秒を JSON の数値にする (値が無い場合は null)
fn format_optional_ms(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{:.3}", v),
        None => "null".to_string(),
    }
}

/// 割合を JSON の数値にする (値が無い場合は null)
fn format_optional_rate(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{:.6}", v),
        None => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 集計結果の JSON を組み立てられること
    #[test]
    fn summary_json_has_all_fields() {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("summary.json");
        let summary = StatsSummary {
            success: 98,
            failure: 2,
            unjudged: 5,
            stalled: 1,
            warmup_excluded: 3,
            failure_reasons: vec![("no-media-sent", 2), ("connect-failed", 1)],
            connect_time_p50_ms: Some(120.5),
            connect_time_p95_ms: Some(300.25),
            connect_time_p99_ms: Some(500.0),
        };

        write_summary_json(&path, &summary).expect("JSON の書き出しに成功すること");

        let written = std::fs::read_to_string(&path).expect("書き出したファイルを読めること");
        assert!(written.contains("\"success\": 98"), "成功数を書くこと");
        assert!(written.contains("\"failure\": 2"), "失敗数を書くこと");
        assert!(written.contains("\"unjudged\": 5"), "判定不能を書くこと");
        assert!(
            written.contains("\"judged\": 100"),
            "判定した接続数を書くこと"
        );
        assert!(
            written.contains("\"success_rate\": 0.980000"),
            "成功接続率を書くこと"
        );
        assert!(
            written.contains("\"stalled\": 1"),
            "停止した接続数を書くこと"
        );
        assert!(
            written.contains("{\"reason\":\"no-media-sent\",\"count\":2}"),
            "失敗理由ごとの接続数を書くこと"
        );
        assert!(
            written.contains("\"p95\": 300.250"),
            "確立までの所要時間の p95 を書くこと"
        );
    }

    /// 判定した接続が無い場合は null を書くこと
    #[test]
    fn summary_json_writes_null_for_missing_values() {
        let dir = tempfile::TempDir::new().expect("一時ディレクトリの作成に失敗");
        let path = dir.path().join("summary.json");
        let summary = StatsSummary {
            unjudged: 3,
            ..StatsSummary::default()
        };

        write_summary_json(&path, &summary).expect("JSON の書き出しに成功すること");

        let written = std::fs::read_to_string(&path).expect("書き出したファイルを読めること");
        assert!(
            written.contains("\"success_rate\": null"),
            "成功接続率が無い場合は null を書くこと"
        );
        assert!(
            written.contains("\"p50\": null"),
            "所要時間が無い場合は null を書くこと"
        );
        assert!(
            written.contains("\"failure_reasons\": []"),
            "失敗理由が無い場合は空配列を書くこと"
        );
    }
}
