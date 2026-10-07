//! しきい値による合否判定
//!
//! 試験全体の集計結果 ([`StatsSummary`]) をしきい値と突き合わせ、CI が終了コードで
//! 判定できるようにする。判定は集計結果としきい値だけを受け取る純粋な処理にする。
//!
//! しきい値が指定されているのに判定に必要なデータが無い場合は、満たしたとみなさず
//! 違反として扱う。測定できなかった試験が CI で成功になるのを避けるため。

use zakuro_core::stats::StatsSummary;

/// 判定に使うしきい値 (未指定の項目は判定しない)
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Thresholds {
    /// 成功接続率の下限
    pub(crate) success_rate: Option<f64>,
    /// 接続確立までの所要時間 p95 の上限 (ミリ秒)
    pub(crate) connect_time_p95_ms: Option<f64>,
    /// 停止した接続数の上限
    pub(crate) stalled: Option<u32>,
}

impl Thresholds {
    /// しきい値が 1 つも指定されていないか
    pub(crate) fn is_empty(&self) -> bool {
        self.success_rate.is_none() && self.connect_time_p95_ms.is_none() && self.stalled.is_none()
    }
}

/// しきい値を満たさなかった項目
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ThresholdViolation {
    /// 成功接続率が下限を下回った
    ///
    /// 判定した接続が 1 本も無い場合は `actual` が None になる。
    SuccessRate { actual: Option<f64>, limit: f64 },
    /// 接続確立までの所要時間 p95 が上限を上回った
    ///
    /// 確立できた接続が 1 本も無い場合は `actual` が None になる。
    ConnectTimeP95 { actual: Option<f64>, limit: f64 },
    /// 停止した接続数が上限を上回った
    Stalled { actual: u32, limit: u32 },
}

impl ThresholdViolation {
    /// ログとエラーメッセージに使う文字列を返す
    pub(crate) fn message(self) -> String {
        match self {
            Self::SuccessRate {
                actual: Some(actual),
                limit,
            } => format!("success-rate: {actual:.4} < {limit:.4}"),
            Self::SuccessRate {
                actual: None,
                limit,
            } => {
                format!("success-rate: no judged connections < {limit:.4}")
            }
            Self::ConnectTimeP95 {
                actual: Some(actual),
                limit,
            } => format!("connect-time-p95-ms: {actual:.3} > {limit:.3}"),
            Self::ConnectTimeP95 {
                actual: None,
                limit,
            } => {
                format!("connect-time-p95-ms: no data > {limit:.3}")
            }
            Self::Stalled { actual, limit } => format!("stalled: {actual} > {limit}"),
        }
    }
}

/// 集計結果をしきい値と突き合わせ、満たさなかった項目を返す
pub(crate) fn evaluate(thresholds: &Thresholds, summary: &StatsSummary) -> Vec<ThresholdViolation> {
    let mut violations = Vec::new();

    if let Some(limit) = thresholds.success_rate {
        let actual = summary.success_rate();
        if actual.is_none_or(|rate| rate < limit) {
            violations.push(ThresholdViolation::SuccessRate { actual, limit });
        }
    }

    if let Some(limit) = thresholds.connect_time_p95_ms {
        let actual = summary.connect_time_p95_ms;
        if actual.is_none_or(|p95| p95 > limit) {
            violations.push(ThresholdViolation::ConnectTimeP95 { actual, limit });
        }
    }

    if let Some(limit) = thresholds.stalled
        && summary.stalled > limit
    {
        violations.push(ThresholdViolation::Stalled {
            actual: summary.stalled,
            limit,
        });
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判定に使う集計結果を作る
    fn summary() -> StatsSummary {
        StatsSummary {
            success: 99,
            failure: 1,
            unjudged: 0,
            stalled: 1,
            warmup_excluded: 0,
            failure_reasons: vec![("no-media-sent", 1)],
            connect_time_p50_ms: Some(100.0),
            connect_time_p95_ms: Some(2000.0),
            connect_time_p99_ms: Some(3000.0),
        }
    }

    /// しきい値を指定していなければ違反なしとすること
    #[test]
    fn no_thresholds_means_no_violations() {
        let thresholds = Thresholds::default();
        assert!(thresholds.is_empty(), "未指定と判定できること");
        assert!(
            evaluate(&thresholds, &summary()).is_empty(),
            "しきい値が無ければ違反しないこと"
        );
    }

    /// しきい値を満たしていれば違反なしとすること
    #[test]
    fn satisfied_thresholds_have_no_violations() {
        let thresholds = Thresholds {
            success_rate: Some(0.99),
            connect_time_p95_ms: Some(2000.0),
            stalled: Some(1),
        };
        assert!(
            evaluate(&thresholds, &summary()).is_empty(),
            "境界値ちょうどは違反としないこと"
        );
    }

    /// 成功接続率が下限を下回ったら違反とすること
    #[test]
    fn success_rate_below_limit_is_a_violation() {
        let thresholds = Thresholds {
            success_rate: Some(0.995),
            ..Thresholds::default()
        };
        let violations = evaluate(&thresholds, &summary());
        assert_eq!(
            violations,
            vec![ThresholdViolation::SuccessRate {
                actual: Some(0.99),
                limit: 0.995,
            }],
            "成功接続率の違反を 1 件返すこと"
        );
        assert_eq!(
            violations[0].message(),
            "success-rate: 0.9900 < 0.9950",
            "実際値としきい値が分かるメッセージを返すこと"
        );
    }

    /// 判定した接続が無い場合は違反とすること
    #[test]
    fn success_rate_without_judged_connections_is_a_violation() {
        let thresholds = Thresholds {
            success_rate: Some(0.99),
            ..Thresholds::default()
        };
        let summary = StatsSummary {
            unjudged: 10,
            ..StatsSummary::default()
        };
        let violations = evaluate(&thresholds, &summary);
        assert_eq!(
            violations,
            vec![ThresholdViolation::SuccessRate {
                actual: None,
                limit: 0.99,
            }],
            "測定できなかった場合は満たしたとみなさないこと"
        );
        assert_eq!(
            violations[0].message(),
            "success-rate: no judged connections < 0.9900",
            "測定できなかったことが分かるメッセージを返すこと"
        );
    }

    /// 確立までの所要時間が上限を上回ったら違反とすること
    #[test]
    fn connect_time_above_limit_is_a_violation() {
        let thresholds = Thresholds {
            connect_time_p95_ms: Some(1000.0),
            ..Thresholds::default()
        };
        let violations = evaluate(&thresholds, &summary());
        assert_eq!(
            violations,
            vec![ThresholdViolation::ConnectTimeP95 {
                actual: Some(2000.0),
                limit: 1000.0,
            }],
            "所要時間の違反を 1 件返すこと"
        );
        assert_eq!(
            violations[0].message(),
            "connect-time-p95-ms: 2000.000 > 1000.000",
            "実際値としきい値が分かるメッセージを返すこと"
        );
    }

    /// 確立できた接続が無い場合は違反とすること
    #[test]
    fn connect_time_without_data_is_a_violation() {
        let thresholds = Thresholds {
            connect_time_p95_ms: Some(1000.0),
            ..Thresholds::default()
        };
        let summary = StatsSummary {
            failure: 5,
            ..StatsSummary::default()
        };
        let violations = evaluate(&thresholds, &summary);
        assert_eq!(
            violations,
            vec![ThresholdViolation::ConnectTimeP95 {
                actual: None,
                limit: 1000.0,
            }],
            "所要時間が 1 件も無い場合は満たしたとみなさないこと"
        );
    }

    /// 停止した接続数が上限を上回ったら違反とすること
    #[test]
    fn stalled_above_limit_is_a_violation() {
        let thresholds = Thresholds {
            stalled: Some(0),
            ..Thresholds::default()
        };
        let violations = evaluate(&thresholds, &summary());
        assert_eq!(
            violations,
            vec![ThresholdViolation::Stalled {
                actual: 1,
                limit: 0,
            }],
            "停止した接続数の違反を 1 件返すこと"
        );
        assert_eq!(
            violations[0].message(),
            "stalled: 1 > 0",
            "実際値としきい値が分かるメッセージを返すこと"
        );
    }

    /// 複数のしきい値を同時に判定すること
    #[test]
    fn multiple_violations_are_reported_together() {
        let thresholds = Thresholds {
            success_rate: Some(1.0),
            connect_time_p95_ms: Some(1000.0),
            stalled: Some(0),
        };
        let violations = evaluate(&thresholds, &summary());
        assert_eq!(violations.len(), 3, "違反した項目をすべて返すこと");
        assert!(
            matches!(violations[0], ThresholdViolation::SuccessRate { .. }),
            "成功接続率の違反を返すこと"
        );
        assert!(
            matches!(violations[1], ThresholdViolation::ConnectTimeP95 { .. }),
            "所要時間の違反を返すこと"
        );
        assert!(
            matches!(violations[2], ThresholdViolation::Stalled { .. }),
            "停止した接続数の違反を返すこと"
        );
    }
}
