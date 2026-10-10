use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// 統計サンプル (接続 1 本 × `get_stats` 1 回) を運ぶチャネルの容量
///
/// 1 メッセージが 1 接続の 1 tick なので、8192 は 100 接続でも約 80 秒分。
/// writer は取り出した分を接続ごとに最新の 1 サンプルへ畳む。容量を超えた tick だけ落とす。
/// 制御コマンド (接続行、ライフサイクル、codec) はこのチャネルを使わない。
pub(crate) const STATS_CHANNEL_CAPACITY: usize = 8192;

/// 未知 RTCStats type の warn を初回のみ出すための全局集合
/// (transport / candidate-pair 等の未対応 type が毎秒 warn で洪水化するのを防ぐ)
static UNKNOWN_TYPES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub(crate) fn unknown_types() -> &'static Mutex<HashSet<String>> {
    UNKNOWN_TYPES.get_or_init(|| Mutex::new(HashSet::new()))
}

/// テスト用: 未知 type 集合をクリアする
#[cfg(test)]
pub(crate) fn clear_unknown_types_for_test() {
    unknown_types()
        .lock()
        .expect("UNKNOWN_TYPES mutex poisoned")
        .clear();
}
