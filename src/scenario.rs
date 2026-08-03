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
///
/// Exit / SendDataChannelMessage は組み込んだシナリオ種別がまだ存在しないため
/// #[expect(dead_code)] を付けている。既存 reconnect シナリオへの組み込みは
/// 後続対応のスコープであり、組み込み時に各 expect を外すこと。
#[derive(Debug, Clone)]
pub(crate) enum ScenarioOp {
    /// ランダムな時間スリープする
    Sleep { min_ms: u64, max_ms: u64 },
    /// 切断する
    Disconnect,
    /// 切断して vc タスクを終了する
    ///
    /// 実行ループを完了として呼び出し元に制御を返し、呼び出し元は再接続せず
    /// vc タスクを終了する (C++ 版は Exit 後もシナリオが継続するが、zakuro-rs
    /// では以降の操作を実行しない)。
    ///
    /// 本バリアントはテストで構築するため、テストビルドでは expect を外す。
    #[cfg_attr(not(test), expect(dead_code))]
    Exit,
    /// 指定ラベルで DataChannel メッセージを 1 回送信する
    ///
    /// min_size / max_size は ZAKURO ヘッダを含む合計サイズ (C++ 版と同じ)。
    /// max_size < min_size の場合は max_size を min_size にクランプする。
    #[expect(dead_code)]
    SendDataChannelMessage {
        label: String,
        min_size: usize,
        max_size: usize,
    },
}

/// シナリオ実行の終了理由
///
/// 呼び出し元は Reconnect で返ったら切断して再接続し、Exit で返ったら切断して
/// vc タスクを終了する。Reconnect は現状 Disconnect 操作で返り、将来追加される
/// Reconnect 操作も同じ値を返す予定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScenarioEnd {
    /// 再接続が必要 (Disconnect 操作)
    Reconnect,
    /// 切断して vc タスクを終了する (Exit 操作)
    Exit,
}

/// シナリオ定義
///
/// ops を先頭から順に実行し、末尾に到達したら loop_index に戻ってループする。
/// Disconnect / Exit 操作に到達すると呼び出し元に制御を返す。
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
/// Disconnect 操作に到達すると呼び出し元が切断と再接続を行い、
/// Exit 操作に到達すると呼び出し元が切断して vc タスクを終了する。
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

    /// シナリオを実行し、Disconnect / Exit 操作に到達するまで待機する。
    /// キャンセルされた場合は即座に返る。キャンセル時は vc タスクの終了を意図する
    /// Exit を返すが、呼び出し元の biased select は token.cancelled() を優先する
    /// ため、キャンセル済みの場合は通常 Shutdown 経路が選択される。競合で Exit
    /// 経路が選択されても vc タスク終了という安全な動作になる。
    ///
    /// handle と ids は接続ごとに変わるため実行時引数として受け取る。
    /// ids は offer 受信後に connection_id が確定するため、実行時に読み取って使う。
    pub(crate) async fn run_until_disconnect(
        &mut self,
        token: &CancellationToken,
        handle: &SoraConnectionHandle,
        ids: &std::sync::Mutex<Option<ConnectionIds>>,
    ) -> ScenarioEnd {
        loop {
            if token.is_cancelled() {
                return ScenarioEnd::Exit;
            }

            // ミュータブル借用と共存させるため clone してからマッチする
            let op = self.scenario.ops[self.op_index].clone();

            match op {
                ScenarioOp::Sleep { min_ms, max_ms } => {
                    let ms = random_range(min_ms, max_ms);
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => return ScenarioEnd::Exit,
                        _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
                    }
                }
                ScenarioOp::Disconnect => {
                    self.advance();
                    return ScenarioEnd::Reconnect;
                }
                ScenarioOp::SendDataChannelMessage {
                    label,
                    min_size,
                    max_size,
                } => {
                    self.send_data_channel_message(handle, ids, &label, min_size, max_size)
                        .await;
                }
                ScenarioOp::Exit => {
                    // Exit で vc タスクが終了しプレイヤーが破棄されるため op_index は進めない
                    return ScenarioEnd::Exit;
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

    /// op_index がシナリオ末尾に達すると loop_index に戻ることを検証する
    ///
    /// Disconnect 操作で制御が返った後も op_index は保持され、再接続時の
    /// run_until_disconnect は続きの操作から再開される (op_index が接続をまたいで
    /// 継続する)。本テストはその土台となる advance() の進行と折返しを検証する。
    #[test]
    fn test_advance_wraps_to_loop_index() {
        let scenario = Scenario {
            ops: vec![
                ScenarioOp::Sleep {
                    min_ms: 1000,
                    max_ms: 5000,
                },
                ScenarioOp::Disconnect,
            ],
            loop_index: 1,
        };
        let mut player = ScenarioPlayer::new(scenario, 0, 0);
        assert_eq!(player.op_index, 0, "初期 op_index が 0 であること");
        player.advance();
        assert_eq!(player.op_index, 1, "1 回の advance で 1 になること");
        // 末尾 (2) に達すると loop_index (1) に戻る
        player.advance();
        assert_eq!(player.op_index, 1, "末尾に達すると loop_index に戻ること");

        // 実運用の構成 (loop_index: 0) への折返し
        let scenario0 = Scenario {
            ops: vec![
                ScenarioOp::Sleep {
                    min_ms: 1000,
                    max_ms: 5000,
                },
                ScenarioOp::Disconnect,
            ],
            loop_index: 0,
        };
        let mut player0 = ScenarioPlayer::new(scenario0, 0, 0);
        player0.advance();
        player0.advance();
        assert_eq!(player0.op_index, 0, "実運用の loop_index=0 に戻ること");
    }

    /// Exit 操作に到達すると ScenarioEnd::Exit が返り、op_index が進まないことを検証する
    ///
    /// SoraConnectionHandle は実サーバー接続なしで build() できる (run() を呼ばない
    /// ため接続は開始されない)。Exit 操作は handle を使わないため、この構築で
    /// 実行分岐を検証できる。
    #[tokio::test]
    async fn test_run_until_disconnect_exit_op() {
        let (_connection, handle) = build_test_connection();

        let scenario = Scenario {
            ops: vec![ScenarioOp::Exit],
            loop_index: 0,
        };
        let mut player = ScenarioPlayer::new(scenario, 0, 0);
        let token = CancellationToken::new();
        let ids = std::sync::Mutex::new(None::<ConnectionIds>);

        let end = player.run_until_disconnect(&token, &handle, &ids).await;
        assert_eq!(
            end,
            ScenarioEnd::Exit,
            "Exit 操作で ScenarioEnd::Exit が返ること"
        );
        assert_eq!(player.op_index, 0, "Exit 操作では op_index が進まないこと");
    }

    /// Disconnect 操作に到達すると ScenarioEnd::Reconnect が返り、op_index が進むことを検証する
    ///
    /// Disconnect 後の再接続時に続きの操作 (Exit 等) から再開される仕組みの検証。
    #[tokio::test]
    async fn test_run_until_disconnect_disconnect_op() {
        let (_connection, handle) = build_test_connection();

        let scenario = Scenario {
            ops: vec![
                ScenarioOp::Disconnect,
                ScenarioOp::Sleep {
                    min_ms: 1,
                    max_ms: 1,
                },
            ],
            loop_index: 0,
        };
        let mut player = ScenarioPlayer::new(scenario, 0, 0);
        let token = CancellationToken::new();
        let ids = std::sync::Mutex::new(None::<ConnectionIds>);

        let end = player.run_until_disconnect(&token, &handle, &ids).await;
        assert_eq!(
            end,
            ScenarioEnd::Reconnect,
            "Disconnect 操作で ScenarioEnd::Reconnect が返ること"
        );
        assert_eq!(
            player.op_index, 1,
            "Disconnect 操作では op_index が進み、再接続時に続きの操作から再開されること"
        );
    }

    /// キャンセル済みの場合は vc タスクの終了を意図する ScenarioEnd::Exit が返ることを検証する
    ///
    /// 呼び出し元の biased select が token.cancelled() を優先するため通常は消費されない
    /// が、万一 Exit 経路が選択されても vc タスク終了という安全な動作になる (fail-safe)。
    #[tokio::test]
    async fn test_run_until_disconnect_cancelled_returns_exit() {
        let (_connection, handle) = build_test_connection();

        let scenario = Scenario {
            ops: vec![ScenarioOp::Sleep {
                min_ms: 1000,
                max_ms: 5000,
            }],
            loop_index: 0,
        };
        let mut player = ScenarioPlayer::new(scenario, 0, 0);
        let token = CancellationToken::new();
        token.cancel();
        let ids = std::sync::Mutex::new(None::<ConnectionIds>);

        let end = player.run_until_disconnect(&token, &handle, &ids).await;
        assert_eq!(
            end,
            ScenarioEnd::Exit,
            "キャンセル済みなら ScenarioEnd::Exit が返ること"
        );
    }

    /// テスト用の実サーバー接続を伴わない SoraConnection とハンドルを構築する
    fn build_test_connection() -> (sora_sdk::SoraConnection, sora_sdk::SoraConnectionHandle) {
        let context =
            sora_sdk::SoraConnectionContext::new().expect("SoraConnectionContext の生成に失敗");
        sora_sdk::SoraConnection::builder(
            context,
            Vec::new(),
            "channel".to_string(),
            sora_sdk::Role::SendRecv,
            NoOpEventHandler,
        )
        .build()
        .expect("SoraConnection の構築に失敗")
    }

    /// テスト用の何もしないイベントハンドラ
    struct NoOpEventHandler;

    impl sora_sdk::SoraConnectionEventHandler for NoOpEventHandler {}
}
