use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nojson::RawJsonOwned;
use shiguredo_webrtc::rtc_log_info;
use sora_sdk::{ConnectDataChannel, SoraClientHandle};
use tokio_util::sync::CancellationToken;

use crate::error::{ErrorMessage, Result};

const MESSAGE_SIZE_MIN: usize = 48;
const MESSAGE_SIZE_MAX: usize = 256_000;

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
/// ```text
/// Bytes 0-5:   "ZAKURO" (シグネチャ)
/// Bytes 6-13:  現在時刻 (マイクロ秒 UNIX Time, big-endian)
/// Bytes 14-21: ラベルごとのカウンター (big-endian)
/// Bytes 22-47: Connection ID (最大 26 バイト、余りは 0 埋め)
/// + ペイロード: ランダムバイナリ
/// ```
fn build_message(
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

fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// DataChannel メッセージ送信タスク
///
/// 各チャネルに対して interval_ms ごとに ZAKURO ヘッダ付きメッセージを送信する
pub(crate) async fn run_messaging(
    id: u32,
    handle: SoraClientHandle,
    channels: Vec<MessageChannel>,
    token: CancellationToken,
) {
    let mut counters: HashMap<String, u64> = HashMap::new();
    let mut xorshift_state: u32 = 0xDEAD_BEEF ^ (id.wrapping_mul(2654435761));

    // 各チャネルごとの次の送信時刻を管理
    let mut next_send: Vec<tokio::time::Instant> = channels
        .iter()
        .map(|_| tokio::time::Instant::now())
        .collect();

    loop {
        // 最も早い次の送信時刻を探す
        let (idx, &earliest) = next_send
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| *t)
            .unwrap();

        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            _ = tokio::time::sleep_until(earliest) => {}
        }

        let ch = &channels[idx];
        let counter = counters.entry(ch.label.clone()).or_insert(0);

        // ペイロードサイズをランダムに決定
        let payload_range = ch.size_max - ch.size_min;
        let payload_extra = if payload_range > 0 {
            (xorshift32(&mut xorshift_state) as usize) % (payload_range + 1)
        } else {
            0
        };
        let payload_size = (ch.size_min + payload_extra).saturating_sub(MESSAGE_SIZE_MIN);

        // Connection ID は空文字列（Sora SDK から取得する方法がないため）
        let msg = build_message(*counter, "", payload_size, &mut xorshift_state);

        rtc_log_info!(
            "[vc-{}] Send DataChannel label={} counter={} size={}",
            id,
            ch.label,
            counter,
            msg.len(),
        );

        if let Err(e) = handle.send_message(&ch.label, &msg).await {
            rtc_log_info!(
                "[vc-{}] DataChannel send failed: label={} error={}",
                id,
                ch.label,
                e,
            );
            break;
        }

        *counter += 1;
        next_send[idx] = tokio::time::Instant::now() + Duration::from_millis(ch.interval_ms as u64);
    }
}
