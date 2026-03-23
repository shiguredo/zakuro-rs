use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// mpsc チャネルのバッファサイズ
/// 典型運用 (instances <= 4 × vcs <= 100) で 1 秒あたり ~2,800 commands を
/// 2 秒分超バッファできるサイズ。最大スケールでは drop が発生しうるが
/// 「サンプリング欠落の許容」を運用ポリシーとする
pub(crate) const CHANNEL_CAPACITY: usize = 8192;

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
