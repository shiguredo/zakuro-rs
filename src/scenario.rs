use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// シナリオ操作
#[derive(Debug, Clone)]
pub(crate) enum ScenarioOp {
    /// ランダムな時間スリープする
    Sleep { min_ms: u64, max_ms: u64 },
    /// 切断する
    Disconnect,
}

/// シナリオ定義
///
/// ops を先頭から順に実行し、末尾に到達したら loop_index に戻ってループする。
/// Disconnect 操作に到達すると呼び出し元に制御を返す。
#[derive(Debug, Clone)]
pub(crate) struct Scenario {
    ops: Vec<ScenarioOp>,
    loop_index: usize,
}

/// シナリオ種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScenarioType {
    Reconnect,
}

impl ScenarioType {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "reconnect" => Some(Self::Reconnect),
            _ => None,
        }
    }
}

/// シナリオを構築する
pub(crate) fn build_scenario(scenario_type: ScenarioType) -> Scenario {
    match scenario_type {
        ScenarioType::Reconnect => build_reconnect_scenario(),
    }
}

/// reconnect シナリオ
///
/// C++ 版の再現:
///   Reconnect (= 接続) → [Sleep(1-5s)] × 9 → ループ先頭に戻る
///
/// C++ 版では Sleep の間に PlayVoiceNumberClient が挟まるが、
/// zakuro-rs では音声再生未対応のため Sleep のみ。
/// ループ先頭に戻ると呼び出し元が再接続する。
fn build_reconnect_scenario() -> Scenario {
    let mut ops = Vec::new();
    // 9 回のランダムスリープ (合計 9-45 秒)
    for _ in 0..9 {
        ops.push(ScenarioOp::Sleep {
            min_ms: 1000,
            max_ms: 5000,
        });
    }
    // 切断してループ先頭に戻る
    ops.push(ScenarioOp::Disconnect);
    Scenario { ops, loop_index: 0 }
}

/// min..=max の範囲でランダムな値を返す
fn random_range(min: u64, max: u64) -> u64 {
    assert!(
        max >= min,
        "random_range: max ({max}) must be >= min ({min})"
    );
    // u128 で計算することで max=u64::MAX / min=0 の u64::MAX+1 も安全に扱える
    let range = max as u128 - min as u128 + 1;
    let mut buf = [0u8; 8];
    aws_lc_rs::rand::fill(&mut buf).expect("random fill failed");
    (min as u128 + (u64::from_ne_bytes(buf) as u128) % range) as u64
}

/// シナリオプレイヤー
///
/// 接続中のクライアントに対してシナリオ操作を順次実行する。
/// Disconnect 操作に到達すると完了し、呼び出し元が切断と再接続を行う。
pub(crate) struct ScenarioPlayer {
    scenario: Scenario,
    op_index: usize,
}

impl ScenarioPlayer {
    pub(crate) fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            op_index: 0,
        }
    }

    /// シナリオを実行し、Disconnect に到達するまで待機する。
    /// キャンセルされた場合は即座に返る。
    pub(crate) async fn run_until_disconnect(&mut self, token: &CancellationToken) {
        loop {
            if token.is_cancelled() {
                return;
            }

            let op = &self.scenario.ops[self.op_index];

            match op {
                ScenarioOp::Sleep { min_ms, max_ms } => {
                    let ms = random_range(*min_ms, *max_ms);
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
                    }
                }
                ScenarioOp::Disconnect => {
                    self.advance();
                    return;
                }
            }

            self.advance();
        }
    }

    fn advance(&mut self) {
        self.op_index += 1;
        if self.op_index >= self.scenario.ops.len() {
            self.op_index = self.scenario.loop_index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// max < min のケースで assert! が発動することを検証する
    #[test]
    #[should_panic(expected = "max (5) must be >= min (10)")]
    fn test_random_range_max_less_than_min_panics() {
        random_range(10, 5);
    }

    /// 既存の呼び出し元の範囲で戻り値が範囲内であることを検証する
    #[test]
    fn test_random_range_existing_range() {
        for _ in 0..1000 {
            let v = random_range(1000, 5000);
            assert!(
                (1000..=5000).contains(&v),
                "戻り値 {v} が [1000, 5000] の範囲内であること"
            );
        }
    }

    /// max=u64::MAX, min=0 のケースで range > 0 かつパニックしないことを検証する
    #[test]
    fn test_random_range_u64_max_boundary() {
        for _ in 0..100 {
            // パニックしないことだけを検証する (戻り値は常に u64 の範囲内)
            let _v = random_range(0, u64::MAX);
        }
    }

    /// min == max のケースで常にその値が返ることを検証する
    #[test]
    fn test_random_range_min_equals_max() {
        for _ in 0..100 {
            let v = random_range(42, 42);
            assert_eq!(v, 42, "min == max のときは常にその値が返ること");
        }
    }
}
