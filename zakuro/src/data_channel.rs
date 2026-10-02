use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nojson::RawJsonOwned;
use shiguredo_webrtc::rtc_log_info;
use sora_sdk::{ConnectDataChannel, SoraConnectionHandle};
use tokio_util::sync::CancellationToken;

use crate::error::{ErrorMessage, Result};

/// DataChannel メッセージの最小サイズ (ZAKURO ヘッダ 48 バイトのみの合計)
pub(crate) const MESSAGE_SIZE_MIN: usize = 48;
/// DataChannel メッセージの最大サイズ (ZAKURO ヘッダを含む合計)
pub(crate) const MESSAGE_SIZE_MAX: usize = 256_000;

/// DataChannel メッセージング用のチャネル設定
#[derive(Debug, Clone)]
pub(crate) struct MessageChannel {
    pub(crate) label: String,
    pub(crate) interval_ms: u32,
    pub(crate) size_min: usize,
    pub(crate) size_max: usize,
}

/// --sora-data-channels の JSON をパースする
///
/// 戻り値: (Sora connect 用の DataChannel 設定, メッセージ送信対象のチャネル)
pub(crate) fn parse_data_channels(
    json_str: &str,
) -> Result<(Vec<ConnectDataChannel>, Vec<MessageChannel>)> {
    let json = RawJsonOwned::parse(json_str)
        .map_err(|e| ErrorMessage::new(format!("sora-data-channels の JSON パースに失敗: {e}")))?;

    let mut connect_channels = Vec::new();
    let mut message_channels = Vec::new();

    for element in json.value().to_array().map_err(|e| {
        ErrorMessage::new(format!("sora-data-channels は配列で指定してください: {e}"))
    })? {
        let label: String = element
            .to_member("label")
            .and_then(|m| m.required())
            .and_then(|v| v.try_into())
            .map_err(|e| ErrorMessage::new(format!("label の取得に失敗: {e}")))?;

        let direction: String = element
            .to_member("direction")
            .and_then(|m| m.required())
            .and_then(|v| v.try_into())
            .map_err(|e| ErrorMessage::new(format!("direction の取得に失敗: {e}")))?;

        let ordered: Option<bool> = element
            .to_member("ordered")
            .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
            .map_err(|e| ErrorMessage::new(format!("ordered の取得に失敗: {e}")))?;

        let max_packet_life_time: Option<i32> = element
            .to_member("max_packet_life_time")
            .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
            .map_err(|e| ErrorMessage::new(format!("max_packet_life_time の取得に失敗: {e}")))?;

        let max_retransmits: Option<i32> = element
            .to_member("max_retransmits")
            .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
            .map_err(|e| ErrorMessage::new(format!("max_retransmits の取得に失敗: {e}")))?;

        let protocol: Option<String> = element
            .to_member("protocol")
            .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
            .map_err(|e| ErrorMessage::new(format!("protocol の取得に失敗: {e}")))?;

        let compress: Option<bool> = element
            .to_member("compress")
            .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
            .map_err(|e| ErrorMessage::new(format!("compress の取得に失敗: {e}")))?;

        connect_channels.push(ConnectDataChannel {
            label: label.clone(),
            direction: direction.clone(),
            ordered,
            max_packet_life_time,
            max_retransmits,
            protocol,
            compress,
            header: None,
        });

        // sendonly/sendrecv のチャネルのみメッセージ送信対象
        if direction == "sendonly" || direction == "sendrecv" {
            let interval_ms: u32 = element
                .to_member("interval")
                .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
                .map_err(|e| ErrorMessage::new(format!("interval の取得に失敗: {e}")))?
                .unwrap_or(500);

            let size_min: usize = element
                .to_member("size-min")
                .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
                .map_err(|e| ErrorMessage::new(format!("size-min の取得に失敗: {e}")))?
                .unwrap_or(MESSAGE_SIZE_MIN);

            let size_max: usize = element
                .to_member("size-max")
                .and_then(|m| m.optional().map(|v| v.try_into()).transpose())
                .map_err(|e| ErrorMessage::new(format!("size-max の取得に失敗: {e}")))?
                .unwrap_or(MESSAGE_SIZE_MIN);

            if !(MESSAGE_SIZE_MIN..=MESSAGE_SIZE_MAX).contains(&size_min) {
                return Err(ErrorMessage::new(format!(
                    "size-min は {MESSAGE_SIZE_MIN} から {MESSAGE_SIZE_MAX} の範囲で指定してください: {size_min}"
                ))
                .into());
            }
            if !(MESSAGE_SIZE_MIN..=MESSAGE_SIZE_MAX).contains(&size_max) {
                return Err(ErrorMessage::new(format!(
                    "size-max は {MESSAGE_SIZE_MIN} から {MESSAGE_SIZE_MAX} の範囲で指定してください: {size_max}"
                ))
                .into());
            }

            message_channels.push(MessageChannel {
                label,
                interval_ms,
                size_min: size_min.max(MESSAGE_SIZE_MIN),
                size_max: size_max.max(size_min),
            });
        }
    }

    Ok((connect_channels, message_channels))
}

/// ZAKURO ヘッダ付きメッセージを構築する
///
/// ペイロードサイズは合計サイズからヘッダ分 (48 バイト) を除いた大きさで指定する。
/// C++ 版と同じヘッダ構造で、DataChannel 連続送信とシナリオ操作の両方から使う。
///
/// ```text
/// Bytes 0-5:   "ZAKURO" (シグネチャ)
/// Bytes 6-13:  現在時刻 (マイクロ秒 UNIX Time, big-endian)
/// Bytes 14-21: ラベルごとのカウンター (big-endian)
/// Bytes 22-47: Connection ID (最大 26 バイト、余りは 0 埋め)
/// + ペイロード: ランダムバイナリ
/// ```
pub(crate) fn build_message(
    counter: u64,
    connection_id: &str,
    payload_size: usize,
    xorshift_state: &mut u32,
) -> Vec<u8> {
    let total_size = MESSAGE_SIZE_MIN + payload_size;
    let mut buf = vec![0u8; total_size];

    // シグネチャ
    buf[..6].copy_from_slice(b"ZAKURO");

    // 現在時刻 (マイクロ秒 UNIX Time, big-endian)
    let time_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    buf[6..14].copy_from_slice(&time_us.to_be_bytes());

    // カウンター (big-endian)
    buf[14..22].copy_from_slice(&counter.to_be_bytes());

    // Connection ID (最大 26 バイト)
    let conn_bytes = connection_id.as_bytes();
    let copy_len = conn_bytes.len().min(26);
    buf[22..22 + copy_len].copy_from_slice(&conn_bytes[..copy_len]);

    // ランダムペイロード
    for chunk in buf[MESSAGE_SIZE_MIN..].chunks_mut(4) {
        let val = xorshift32(xorshift_state);
        let bytes = val.to_le_bytes();
        let len = chunk.len().min(4);
        chunk[..len].copy_from_slice(&bytes[..len]);
    }

    buf
}

/// xorshift32 のステップ更新
///
/// ペイロードのランダムバイナリとペイロードサイズの決定に使う。
pub(crate) fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// 合計サイズ (ZAKURO ヘッダ込み) の範囲からペイロードサイズを決定する
///
/// min_size / max_size は ZAKURO ヘッダ (48 バイト) を含む合計サイズで、
/// 呼び出し元で 48..=256000 の範囲検証と max_size >= min_size の正規化が
/// 済んでいること。ペイロードは min_size - 48 〜 max_size - 48 バイトの
/// ランダムな大きさになる。契約違反は実装バグとして panic する。
pub(crate) fn payload_size_from(min_size: usize, max_size: usize, random: u32) -> usize {
    assert!(
        min_size >= MESSAGE_SIZE_MIN,
        "payload_size_from: min_size ({min_size}) must be >= {MESSAGE_SIZE_MIN}"
    );
    assert!(
        max_size >= min_size,
        "payload_size_from: max_size ({max_size}) must be >= min_size ({min_size})"
    );
    let min_payload = min_size - MESSAGE_SIZE_MIN;
    let max_payload = max_size - MESSAGE_SIZE_MIN;
    let payload_range = max_payload - min_payload;
    let payload_extra = if payload_range > 0 {
        random as usize % (payload_range + 1)
    } else {
        0
    };
    min_payload + payload_extra
}

/// xorshift32 の初期 seed を `instance_id` と `vc_id` から計算する
///
/// 黄金比 * 2^32 を表す 2 つの定数 (`2654435761` = `0x9E3779B1` と `0x9E3779B9`)
/// を使い、`instance_id` と `vc_id` が混ざらないように別の係数を適用する。
/// `0xDEAD_BEEF` の XOR は元実装と同じ。
///
/// xorshift32 は state=0 のとき永久に 0 を返す LFSR 性質を持つため、計算結果が
/// 0 になった場合は 1 を返してフェイルセーフにする。
///
/// 後方互換: `instance_id=0` のとき `instance_id.wrapping_mul(0x9E3779B9) == 0`
/// で XOR の単位元として作用し、`state == 0xDEAD_BEEF ^ vc_id.wrapping_mul(2654435761)`
/// となる。`vcs <= 1000` の実用範囲で state=0 にはならないため、現行実装の
/// xorshift32 出力と完全一致する。
pub(crate) fn compute_seed(instance_id: u32, vc_id: u32) -> u32 {
    let state = 0xDEAD_BEEF ^ vc_id.wrapping_mul(2654435761) ^ instance_id.wrapping_mul(0x9E3779B9);
    if state == 0 { 1 } else { state }
}

/// DataChannel メッセージ送信タスク
///
/// 各チャネルに対して interval_ms ごとに ZAKURO ヘッダ付きメッセージを送信する
pub(crate) async fn run_messaging(
    instance_id: u32,
    vc_id: u32,
    handle: SoraConnectionHandle,
    channels: Vec<MessageChannel>,
    token: CancellationToken,
) {
    let mut counters: HashMap<String, u64> = HashMap::new();
    let mut xorshift_state: u32 = compute_seed(instance_id, vc_id);

    // 各チャネルごとの次の送信時刻を管理
    let mut next_send: Vec<tokio::time::Instant> = channels
        .iter()
        .map(|_| tokio::time::Instant::now())
        .collect();

    loop {
        // 最も早い次の送信時刻を探す
        let (idx, &earliest) =
            next_send.iter().enumerate().min_by_key(|(_, t)| *t).expect(
                "logical invariant: run_messaging is only spawned when channels is non-empty",
            );

        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            _ = tokio::time::sleep_until(earliest) => {}
        }

        let ch = &channels[idx];
        let counter = counters.entry(ch.label.clone()).or_insert(0);

        // ペイロードサイズをランダムに決定
        // (min == max の固定サイズ時は乱数を消費しない。旧実装との乱数列の互換を保つ)
        let payload_size = if ch.size_max > ch.size_min {
            payload_size_from(ch.size_min, ch.size_max, xorshift32(&mut xorshift_state))
        } else {
            ch.size_min - MESSAGE_SIZE_MIN
        };

        // Connection ID は空文字列（Sora SDK から取得する方法がないため）
        let msg = build_message(*counter, "", payload_size, &mut xorshift_state);

        rtc_log_info!(
            "[i{}/vc-{}] Send DataChannel label={} counter={} size={}",
            instance_id,
            vc_id,
            ch.label,
            counter,
            msg.len(),
        );

        if let Err(e) = handle.send_message(&ch.label, &msg).await {
            rtc_log_info!(
                "[i{}/vc-{}] DataChannel send failed: label={} error={}",
                instance_id,
                vc_id,
                ch.label,
                e,
            );
            break;
        }

        *counter += 1;
        next_send[idx] = tokio::time::Instant::now() + Duration::from_millis(ch.interval_ms as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧実装 (instance_id 概念なし) の seed 計算
    ///
    /// `vcs <= 1000` バリデーション範囲内では旧実装は state=0 にならない
    /// (state=0 となる vc_id = 416_041_631 は範囲外) ため、補正前の値そのものを返す。
    fn legacy_seed(vc_id: u32) -> u32 {
        0xDEAD_BEEFu32 ^ vc_id.wrapping_mul(2654435761)
    }

    #[test]
    fn compute_seed_matches_legacy_when_instance_id_is_zero() {
        // 後方互換: instance_id=0 のとき、vcs バリデーション範囲 (0..=1000) で
        // 旧実装と完全に一致することを網羅的に検証する
        for vc_id in 0u32..=1000 {
            assert_eq!(
                compute_seed(0, vc_id),
                legacy_seed(vc_id),
                "instance_id=0, vc_id={} で旧実装と seed が一致しない",
                vc_id,
            );
        }
    }

    #[test]
    fn compute_seed_is_never_zero_in_valid_range() {
        // vc_id ∈ 0..=1000 (バリデーション範囲) の代表値と境界値で state=0 にならないこと
        // (xorshift32 の LFSR 退化防止)
        let representatives = [0u32, 1, 2, 100, 500, 999, 1000];
        for instance_id in [0u32, 1, 8, 64] {
            for vc_id in representatives {
                let seed = compute_seed(instance_id, vc_id);
                assert_ne!(
                    seed, 0,
                    "instance_id={}, vc_id={} で seed=0 になっている",
                    instance_id, vc_id,
                );
            }
        }
    }

    #[test]
    fn compute_seed_differs_between_instances_for_same_vc_id() {
        // 同じ vc_id でも instance_id が違えば seed が分かれるはず
        // (DataChannel payload 乱数列が複数 instance で同一にならないことを保証)
        let vc_id = 0u32;
        let seed_i0 = compute_seed(0, vc_id);
        let seed_i1 = compute_seed(1, vc_id);
        let seed_i2 = compute_seed(2, vc_id);
        assert_ne!(
            seed_i0, seed_i1,
            "instance_id=0,1 の seed が同じになっている"
        );
        assert_ne!(
            seed_i1, seed_i2,
            "instance_id=1,2 の seed が同じになっている"
        );
        assert_ne!(
            seed_i0, seed_i2,
            "instance_id=0,2 の seed が同じになっている"
        );
    }

    /// ヘッダの各フィールドが仕様どおりの位置・バイト列で埋まることを検証する
    #[test]
    fn test_build_message_header_layout() {
        let mut state = 0x12345678u32;
        let msg = build_message(42, "conn-abc", 100, &mut state);

        assert_eq!(
            msg.len(),
            MESSAGE_SIZE_MIN + 100,
            "合計サイズはヘッダ 48 バイト + ペイロード 100 バイトになること"
        );
        assert_eq!(&msg[..6], b"ZAKURO", "シグネチャが先頭 6 バイトに並ぶこと");
        // 時刻フィールドは現在時刻なので、0 でないことだけを検証する
        assert_ne!(
            &msg[6..14],
            &[0u8; 8],
            "時刻フィールド (8 バイト) が 0 でないこと"
        );
        assert_eq!(
            &msg[14..22],
            &42u64.to_be_bytes(),
            "カウンタが big-endian で 14 バイト目から 8 バイトに並ぶこと"
        );
        assert_eq!(
            &msg[22..30],
            b"conn-abc",
            "connection_id が 22 バイト目から並ぶこと"
        );
        assert_eq!(
            &msg[30..48],
            &[0u8; 18],
            "connection_id の余り領域は 0 埋めされること"
        );
    }

    /// 26 バイトちょうどの connection_id は切り詰められず全バイトが並ぶことを検証する
    #[test]
    fn test_build_message_connection_id_exact_26_bytes() {
        let mut state = 0x1234_5678u32;
        let id = "b".repeat(26);
        let msg = build_message(0, &id, 0, &mut state);

        assert_eq!(
            &msg[22..48],
            &[b'b'; 26],
            "26 バイトちょうどは切り詰められず全バイトが並ぶこと"
        );
    }

    /// 空文字列の connection_id は領域全体が 0 埋めされることを検証する
    ///
    /// connection_id 未確定時は空文字列として送信するため、このケースは実運用の経路。
    #[test]
    fn test_build_message_empty_connection_id_zero_fills() {
        let mut state = 0xABCD_EF01u32;
        let msg = build_message(0, "", 0, &mut state);

        assert_eq!(
            &msg[22..48],
            &[0u8; 26],
            "空文字列の connection_id は領域全体が 0 埋めされること"
        );
    }

    /// 26 バイトを超える connection_id は先頭 26 バイトに切り詰められることを検証する
    #[test]
    fn test_build_message_truncates_long_connection_id() {
        let mut state = 0xDEADBEEFu32;
        let long_id = "a".repeat(40);
        let msg = build_message(0, &long_id, 0, &mut state);

        assert_eq!(
            &msg[22..48],
            &[b'a'; 26],
            "connection_id は先頭 26 バイトで切り詰められること"
        );
    }

    /// ペイロード領域は xorshift32 の出力で埋まり、同じ初期状態なら同じ列になることを検証する
    #[test]
    fn test_build_message_payload_is_deterministic() {
        let mut state1 = 0xCAFEBABEu32;
        let msg1 = build_message(0, "c", 16, &mut state1);
        let mut state2 = 0xCAFEBABEu32;
        let msg2 = build_message(0, "c", 16, &mut state2);

        assert_eq!(
            &msg1[48..],
            &msg2[48..],
            "同じ初期状態・同じパラメータならペイロードは同じ列になること"
        );
        // ペイロードが全て 0 にならないこと (xorshift32 が退化していないこと)
        assert_ne!(&msg1[48..], &[0u8; 16], "ペイロードが全て 0 でないこと");
    }

    /// 4 の倍数でないペイロードでも端数チャンクが正しく埋まることを検証する
    #[test]
    fn test_build_message_partial_last_chunk() {
        let mut state = 0x2222_3333u32;
        // 26 バイト = 4 バイト × 6 チャンク + 2 バイトの端数
        let msg = build_message(1, "c", 26, &mut state);

        assert_eq!(
            msg.len(),
            MESSAGE_SIZE_MIN + 26,
            "合計サイズがヘッダ 48 バイト + ペイロード 26 バイトになること"
        );
        // ペイロード先頭 4 バイトは 1 回目の xorshift32 出力の little-endian
        let mut expected_state = 0x2222_3333u32;
        let first = xorshift32(&mut expected_state);
        assert_eq!(
            &msg[48..52],
            &first.to_le_bytes(),
            "ペイロードの先頭チャンクが xorshift32 出力で埋まること"
        );
        // ペイロード末尾 2 バイト (端数チャンク) は 7 回目 (6 フルチャンク + 端数) の
        // xorshift32 出力の先頭 2 バイト
        let mut last = first;
        for _ in 0..6 {
            last = xorshift32(&mut expected_state);
        }
        assert_eq!(
            &msg[72..74],
            &last.to_le_bytes()[..2],
            "端数チャンクが xorshift32 出力の先頭バイトで埋まること"
        );
    }

    /// ペイロードサイズが min_size - 48 〜 max_size - 48 の範囲に収まることを検証する
    #[test]
    fn test_payload_size_from_within_range() {
        for random in [0u32, 1, 12345, u32::MAX / 2, u32::MAX] {
            let payload = payload_size_from(100, 200, random);
            assert!(
                (52..=152).contains(&payload),
                "random={random} のとき payload={payload} が [52, 152] の範囲内であること"
            );
        }
    }

    /// 乱数を使ったループでもペイロードサイズが範囲内に収まることを検証する
    #[test]
    fn test_payload_size_from_random_loop() {
        let mut state = 0x89AB_CDEFu32;
        for _ in 0..1000 {
            let random = xorshift32(&mut state);
            let payload = payload_size_from(48, 256_000, random);
            assert!(
                (0..=255_952).contains(&payload),
                "random={random} のとき payload={payload} が [0, 255952] の範囲内であること"
            );
        }
    }

    /// min_size == max_size のときは常に固定のペイロードサイズになることを検証する
    #[test]
    fn test_payload_size_from_min_equals_max() {
        for random in [0u32, 1, 12345, u32::MAX] {
            let payload = payload_size_from(48, 48, random);
            assert_eq!(
                payload, 0,
                "random={random} でもペイロードが 0 バイトになること"
            );
        }
        // 上限側の min == max でも固定サイズになること (ヘッダ込み 256000 バイトちょうど)
        for random in [0u32, 1, 12345, u32::MAX] {
            let payload = payload_size_from(256_000, 256_000, random);
            assert_eq!(
                payload, 255_952,
                "random={random} でもペイロードが 255952 バイトになること"
            );
        }
    }

    /// ペイロードサイズが最小値のときは 0 バイトになることを検証する
    #[test]
    fn test_payload_size_from_lower_bound() {
        let payload = payload_size_from(48, 256_000, 0);
        assert_eq!(payload, 0, "random=0 のときペイロードが 0 バイトになること");
    }

    /// 契約違反 (min_size < 48) は実装バグとして panic することを検証する
    #[test]
    #[should_panic(expected = "payload_size_from: min_size (47) must be >= 48")]
    fn test_payload_size_from_min_below_48_panics() {
        payload_size_from(47, 100, 0);
    }

    /// 契約違反 (max_size < min_size) は実装バグとして panic することを検証する
    #[test]
    #[should_panic(expected = "payload_size_from: max_size (50) must be >= min_size (100)")]
    fn test_payload_size_from_max_below_min_panics() {
        payload_size_from(100, 50, 0);
    }

    /// 手計算した決定的な期待値と一致することを検証する
    ///
    /// 旧 run_messaging 実装との等価変換の退行検出用 (剰余の基数や -48 の適用位置が
    /// 変わると値がずれる)
    #[test]
    fn test_payload_size_from_golden_values() {
        assert_eq!(
            payload_size_from(100, 200, 42),
            94,
            "min=100, max=200, random=42 のとき 52 + 42 % 101 = 94 になること"
        );
        assert_eq!(
            payload_size_from(100, 100, u32::MAX),
            52,
            "min == max のとき range=0 で固定のペイロードになること"
        );
        // random % (255952 + 1) == 255952 となる値で最大ペイロードになること
        assert_eq!(
            payload_size_from(48, 256_000, 255_952),
            255_952,
            "random=255952 のとき最大ペイロードになること"
        );
    }
}
