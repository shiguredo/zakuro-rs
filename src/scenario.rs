use std::collections::HashMap;
use std::time::Duration;

use shiguredo_webrtc::{rtc_log_info, rtc_log_warning};
use sora_sdk::SoraConnectionHandle;
use tokio_util::sync::CancellationToken;

use crate::data_channel::{
    MESSAGE_SIZE_MAX, MESSAGE_SIZE_MIN, build_message, compute_seed, payload_size_from, xorshift32,
};
use crate::duckdb_stats::ConnectionIds;

/// シナリオ操作
#[derive(Debug, Clone)]
pub(crate) enum ScenarioOp {
    /// ランダムな時間スリープする
    Sleep { min_ms: u64, max_ms: u64 },
    /// 切断する
    Disconnect,
    /// 指定ラベルで DataChannel メッセージを 1 回送信する
    ///
    /// min_size / max_size は ZAKURO ヘッダを含む合計サイズ (C++ 版と同じ)。
    /// max_size < min_size の場合は max_size を min_size にクランプする。
    ///
    /// この操作を組み込んだシナリオ種別はまだ存在しない。既存 reconnect シナリオ
    /// への組み込みは後続対応のスコープであり、組み込み時に本 expect を外すこと。
    #[expect(dead_code)]
    SendDataChannelMessage {
        label: String,
        min_size: usize,
        max_size: usize,
    },
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
/// 接続ループの外で 1 回生成され、ラベル別カウンタと xorshift 状態は
/// 再接続をまたいで保持する。
pub(crate) struct ScenarioPlayer {
    scenario: Scenario,
    op_index: usize,
    instance_id: u32,
    vc_id: u32,
    /// DataChannel 送信のラベル別カウンタ (再接続をまたいで永続する)
    dc_counter: HashMap<String, u64>,
    /// DataChannel ペイロード生成用の xorshift 状態 (送信ごとに更新する)
    xorshift_state: u32,
}

impl ScenarioPlayer {
    pub(crate) fn new(scenario: Scenario, instance_id: u32, vc_id: u32) -> Self {
        Self {
            scenario,
            op_index: 0,
            instance_id,
            vc_id,
            dc_counter: HashMap::new(),
            xorshift_state: compute_seed(instance_id, vc_id),
        }
    }

    /// シナリオを実行し、Disconnect に到達するまで待機する。
    /// キャンセルされた場合は即座に返る。
    ///
    /// handle と ids は接続ごとに変わるため実行時引数として受け取る。
    /// ids は offer 受信後に connection_id が確定するため、実行時に読み取って使う。
    pub(crate) async fn run_until_disconnect(
        &mut self,
        token: &CancellationToken,
        handle: &SoraConnectionHandle,
        ids: &std::sync::Mutex<Option<ConnectionIds>>,
    ) {
        loop {
            if token.is_cancelled() {
                return;
            }

            // ミュータブル借用と共存させるため clone してからマッチする
            let op = self.scenario.ops[self.op_index].clone();

            match op {
                ScenarioOp::Sleep { min_ms, max_ms } => {
                    let ms = random_range(min_ms, max_ms);
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
                ScenarioOp::SendDataChannelMessage {
                    label,
                    min_size,
                    max_size,
                } => {
                    self.send_data_channel_message(handle, ids, &label, min_size, max_size)
                        .await;
                }
            }

            self.advance();
        }
    }

    /// SendDataChannelMessage 操作を実行する
    ///
    /// 送信失敗時は warning ログを出して操作は完了として扱う (C++ 版互換)。
    async fn send_data_channel_message(
        &mut self,
        handle: &SoraConnectionHandle,
        ids: &std::sync::Mutex<Option<ConnectionIds>>,
        label: &str,
        min_size: usize,
        max_size: usize,
    ) {
        let (min_size, max_size) = normalize_message_sizes(min_size, max_size);
        // ペイロードサイズの決定にも xorshift を使う (連続送信と同じ方式)
        let random = xorshift32(&mut self.xorshift_state);
        let payload_size = payload_size_from(min_size, max_size, random);

        let connection_id = read_connection_id(ids, self.instance_id, self.vc_id);

        let counter = self.dc_counter.entry(label.to_string()).or_insert(0);
        let msg = build_message(
            *counter,
            &connection_id,
            payload_size,
            &mut self.xorshift_state,
        );

        rtc_log_info!(
            "[i{}/vc-{}] Send DataChannel label={} counter={} size={}",
            self.instance_id,
            self.vc_id,
            label,
            counter,
            msg.len(),
        );

        // 送信結果を待たずに増加させる (C++ 版は送信要求の直後に増加する。
        // 送信失敗時も増加する点は C++ 版互換。await 中に future がドロップされて
        // も同一カウンタでの再送 (重番) が生じないことを保証する)
        *counter += 1;

        if let Err(e) = handle.send_message(label, &msg).await {
            rtc_log_warning!(
                "[i{}/vc-{}] DataChannel send failed: label={} error={}",
                self.instance_id,
                self.vc_id,
                label,
                e,
            );
        }
    }

    fn advance(&mut self) {
        self.op_index += 1;
        if self.op_index >= self.scenario.ops.len() {
            self.op_index = self.scenario.loop_index;
        }
    }
}

/// DataChannel 送信サイズ (ZAKURO ヘッダ込みの合計サイズ) を検証して正規化する
///
/// max_size < min_size の場合は max_size を min_size にクランプする。
/// 48..=256000 の範囲外はシナリオ定義エラーとして panic する。
fn normalize_message_sizes(min_size: usize, max_size: usize) -> (usize, usize) {
    assert!(
        (MESSAGE_SIZE_MIN..=MESSAGE_SIZE_MAX).contains(&min_size),
        "SendDataChannelMessage: min_size ({min_size}) must be in {MESSAGE_SIZE_MIN}..={MESSAGE_SIZE_MAX}"
    );
    assert!(
        (MESSAGE_SIZE_MIN..=MESSAGE_SIZE_MAX).contains(&max_size),
        "SendDataChannelMessage: max_size ({max_size}) must be in {MESSAGE_SIZE_MIN}..={MESSAGE_SIZE_MAX}"
    );
    (min_size, max_size.max(min_size))
}

/// ids から connection_id を読み取る
///
/// 未確定 (None) やロック失敗 (poison) の場合は空文字列を返す。
/// poison 時は既存コードと同じく warning ログを出す。
fn read_connection_id(
    ids: &std::sync::Mutex<Option<ConnectionIds>>,
    instance_id: u32,
    vc_id: u32,
) -> String {
    match ids.lock() {
        Ok(guard) => guard
            .as_ref()
            .map(|ids| ids.connection_id.clone())
            .unwrap_or_default(),
        Err(_) => {
            rtc_log_warning!(
                "[i{}/vc-{}] connection_ids mutex poisoned in scenario data channel send",
                instance_id,
                vc_id,
            );
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    /// max_size < min_size の場合は max_size が min_size にクランプされることを検証する
    #[test]
    fn test_normalize_message_sizes_clamps_max_below_min() {
        let (min, max) = normalize_message_sizes(100, 50);
        assert_eq!(min, 100, "min_size はそのまま残ること");
        assert_eq!(
            max, 100,
            "max_size < min_size なら max_size が min_size にクランプされること"
        );
    }

    /// 範囲外の min_size はシナリオ定義エラーとして panic することを検証する
    #[test]
    #[should_panic(expected = "min_size (47) must be in 48..=256000")]
    fn test_normalize_message_sizes_min_below_48_panics() {
        normalize_message_sizes(47, 100);
    }

    /// min_size が上限超過の場合は min_size 側の検証で panic することを検証する
    #[test]
    #[should_panic(expected = "min_size (256001) must be in 48..=256000")]
    fn test_normalize_message_sizes_min_over_256000_panics() {
        normalize_message_sizes(256_001, 100);
    }

    /// 範囲外の max_size はシナリオ定義エラーとして panic することを検証する
    #[test]
    #[should_panic(expected = "max_size (256001) must be in 48..=256000")]
    fn test_normalize_message_sizes_max_over_256000_panics() {
        normalize_message_sizes(48, 256_001);
    }

    /// min_size は範囲内でも max_size が範囲外 (48 未満) なら panic することを検証する
    #[test]
    #[should_panic(expected = "max_size (47) must be in 48..=256000")]
    fn test_normalize_message_sizes_max_below_48_panics() {
        normalize_message_sizes(48, 47);
    }

    /// 正常系の下限境界 (48, 48) が通ることを検証する
    #[test]
    fn test_normalize_message_sizes_lower_bound_ok() {
        let (min, max) = normalize_message_sizes(48, 48);
        assert_eq!(min, 48, "min_size がそのまま残ること");
        assert_eq!(max, 48, "max_size がそのまま残ること");
    }

    /// 正常系の上限境界 (256000, 256000) が通ることを検証する
    #[test]
    fn test_normalize_message_sizes_upper_bound_ok() {
        let (min, max) = normalize_message_sizes(256_000, 256_000);
        assert_eq!(min, 256_000, "min_size がそのまま残ること");
        assert_eq!(max, 256_000, "max_size がそのまま残ること");
    }

    /// 送信パス全体 (サイズ正規化 → ペイロードサイズ決定 → ヘッダ構築) を合成した
    /// ときに、メッセージ合計サイズがクランプ後の [min_size, max_size] に収まることを
    /// 検証する (個別関数のテストでは拾えない統合退行の検出用)
    #[test]
    fn test_message_total_size_within_normalized_range() {
        let mut state = 0x1111_2222u32;
        let cases = [(100, 50), (48, 48), (256_000, 256_000), (5000, 10_000)];
        for (min_size, max_size) in cases {
            let (min_size, max_size) = normalize_message_sizes(min_size, max_size);
            let random = xorshift32(&mut state);
            let payload_size = payload_size_from(min_size, max_size, random);
            let msg = build_message(0, "conn", payload_size, &mut state);
            assert!(
                (min_size..=max_size).contains(&msg.len()),
                "min={min_size}, max={max_size} のとき合計サイズ {} が範囲内であること",
                msg.len(),
            );
        }
    }

    /// connection_id が確定している場合はその値が返ることを検証する
    #[test]
    fn test_read_connection_id_returns_value() {
        let ids = Arc::new(std::sync::Mutex::new(Some(ConnectionIds {
            connection_id: "conn-1".to_string(),
            session_id: "sess-1".to_string(),
        })));
        let id = read_connection_id(&ids, 0, 0);
        assert_eq!(id, "conn-1", "確定済みの connection_id が返ること");
    }

    /// connection_id が未確定 (None) の場合は空文字列が返ることを検証する
    #[test]
    fn test_read_connection_id_returns_empty_when_none() {
        let ids = Arc::new(std::sync::Mutex::new(None::<ConnectionIds>));
        let id = read_connection_id(&ids, 0, 0);
        assert_eq!(id, "", "未確定の場合は空文字列が返ること");
    }

    /// poison された ids からは空文字列が返り、パニックしないことを検証する
    #[test]
    fn test_read_connection_id_returns_empty_when_poisoned() {
        let ids = Arc::new(std::sync::Mutex::new(None::<ConnectionIds>));
        // Mutex を poison させる
        let ids_clone = ids.clone();
        let handle = std::thread::spawn(move || {
            let _guard = ids_clone
                .lock()
                .expect("poison 検証用の lock は成功すること");
            panic!("意図的に mutex を poison する");
        });
        let _ = handle.join();

        let id = read_connection_id(&ids, 0, 0);
        assert_eq!(id, "", "poison 時は空文字列が返り、パニックしないこと");
    }
}
