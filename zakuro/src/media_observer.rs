//! RTCStats のサンプルからメディアの送受信を観測する
//!
//! `get_stats()` が返す累積カウンタを前回のサンプルと比較し、映像 / 音声の
//! パケットが流れた時刻と、SFU からのレポート (remote-inbound-rtp) が届いた時刻を
//! 接続ライフサイクルへ記録する。
//!
//! カウンタはストリーム (stats id) ごとに保持する。再ネゴシエーションで新しい
//! ストリームが生まれるとカウンタは 0 から始まるため、単純な合計では増減を誤る。
//! 初めて見る id は基準値として扱い、値が 0 より大きければ「その時点までに
//! 既に流れている」とみなす。同じ id で値が減った場合はリセットとして扱い、
//! 増加とはみなさない。

use std::collections::HashMap;
use std::time::SystemTime;

use nojson::{RawJsonOwned, RawJsonValue};

use crate::connection_lifecycle::{ConnectionLifecycle, MediaKind};

/// ストリーム 1 本の直前に観測したカウンタ
#[derive(Debug, Clone, Copy)]
struct StreamCounters {
    /// 送信パケット数 (outbound-rtp) または受信パケット数 (inbound-rtp)
    packets: u64,
    /// 送受信バイト数
    bytes: u64,
    /// エンコード済みフレーム数 (outbound-rtp のみ)
    frames: u64,
}

impl StreamCounters {
    /// カウンタが動いているか
    ///
    /// 初回観測でも、値が 0 より大きければストリーム開始後にメディアが流れている。
    fn has_traffic(&self) -> bool {
        self.packets > 0 || self.bytes > 0 || self.frames > 0
    }

    /// 直前の観測から増えているか
    ///
    /// 値が減っている場合 (ストリームのリセット) は増加とみなさない。
    fn increased_from(&self, previous: &Self) -> bool {
        self.packets > previous.packets
            || self.bytes > previous.bytes
            || self.frames > previous.frames
    }
}

/// メディアの送受信の観測状態
///
/// 接続 1 本につき 1 つ作る。統計収集タスクが所有し、切断時に一緒に破棄される。
#[derive(Debug, Default)]
pub(crate) struct MediaObserver {
    /// 送信ストリーム (stats id → 直前に観測したカウンタ)
    outbound: HashMap<String, StreamCounters>,
    /// 受信ストリーム (stats id → 直前に観測したカウンタ)
    inbound: HashMap<String, StreamCounters>,
}

impl MediaObserver {
    /// 観測状態を作る
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 統計サンプル 1 件を取り込み、観測できた事実をライフサイクルへ反映する
    ///
    /// パースできないサンプルと、メディアの統計を含まないサンプルは
    /// 判定材料にならないため数えない。
    pub(crate) fn observe(
        &mut self,
        lifecycle: &mut ConnectionLifecycle,
        stats_json: &str,
        observed_at: SystemTime,
    ) {
        let Ok(json) = RawJsonOwned::parse(stats_json) else {
            return;
        };
        let Ok(elements) = json.value().to_array() else {
            return;
        };

        let mut has_media_entry = false;
        let mut has_activity = false;
        let mut has_delivery_report = false;
        for element in elements {
            let Some(stats_type) = get_string(element, "type") else {
                continue;
            };
            match stats_type.as_str() {
                "outbound-rtp" => {
                    let Some(id) = get_string(element, "id") else {
                        continue;
                    };
                    let Some(kind) = media_kind(element) else {
                        continue;
                    };
                    has_media_entry = true;
                    let counters = StreamCounters {
                        packets: get_u64(element, "packetsSent"),
                        bytes: get_u64(element, "bytesSent"),
                        frames: get_u64(element, "framesEncoded"),
                    };
                    if observe_stream(&mut self.outbound, &id, counters) {
                        lifecycle.on_media_sent(kind, observed_at);
                        has_activity = true;
                    }
                }
                "inbound-rtp" => {
                    let Some(id) = get_string(element, "id") else {
                        continue;
                    };
                    let Some(kind) = media_kind(element) else {
                        continue;
                    };
                    has_media_entry = true;
                    let counters = StreamCounters {
                        packets: get_u64(element, "packetsReceived"),
                        bytes: get_u64(element, "bytesReceived"),
                        frames: 0,
                    };
                    if observe_stream(&mut self.inbound, &id, counters) {
                        lifecycle.on_media_received(kind, observed_at);
                        has_activity = true;
                    }
                }
                // SFU からのレポートが届いた = 送信したパケットが SFU に届いた証拠
                "remote-inbound-rtp" => {
                    has_delivery_report = true;
                }
                _ => {}
            }
        }

        if has_media_entry {
            lifecycle.on_sample_observed();
            // 増加が無いサンプルが続くほどメディアが止まっている可能性が高い
            lifecycle.on_sample_activity(observed_at, has_activity);
        }
        if has_delivery_report {
            lifecycle.on_delivery_report(observed_at);
        }
    }
}

/// ストリームのカウンタを更新し、メディアが流れたと判断できるかを返す
fn observe_stream(
    streams: &mut HashMap<String, StreamCounters>,
    id: &str,
    counters: StreamCounters,
) -> bool {
    match streams.get_mut(id) {
        Some(previous) => {
            let increased = counters.increased_from(previous);
            *previous = counters;
            increased
        }
        None => {
            let has_traffic = counters.has_traffic();
            streams.insert(id.to_string(), counters);
            has_traffic
        }
    }
}

/// JSON 要素からメンバーを取り出すヘルパー (Option<String>)
fn get_string(v: RawJsonValue<'_, '_>, key: &str) -> Option<String> {
    v.to_member(key)
        .ok()?
        .optional()
        .and_then(|val| val.try_into().ok())
}

/// JSON 要素から非負の整数を取り出すヘルパー (取得できない場合は 0)
fn get_u64(v: RawJsonValue<'_, '_>, key: &str) -> u64 {
    let value: Option<i64> = v
        .to_member(key)
        .ok()
        .and_then(|m| m.optional())
        .and_then(|val| val.try_into().ok());
    value.unwrap_or(0).max(0) as u64
}

/// JSON 要素からメディアの種別を取り出す
fn media_kind(v: RawJsonValue<'_, '_>) -> Option<MediaKind> {
    match get_string(v, "kind")?.as_str() {
        "video" => Some(MediaKind::Video),
        "audio" => Some(MediaKind::Audio),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定の基準時刻からの経過秒で SystemTime を作る
    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
    }

    /// 送信と受信と SFU からのレポートを含むサンプル
    fn sample(outbound_packets: u64, outbound_frames: u64, inbound_packets: u64) -> String {
        format!(
            r#"[
                {{"type":"outbound-rtp","id":"OUT1","kind":"video","packetsSent":{outbound_packets},"bytesSent":{},"framesEncoded":{outbound_frames}}},
                {{"type":"inbound-rtp","id":"IN1","kind":"audio","packetsReceived":{inbound_packets},"bytesReceived":{}}},
                {{"type":"remote-inbound-rtp","id":"RI1"}}
            ]"#,
            outbound_packets * 100,
            inbound_packets * 10,
        )
    }

    /// 初回のサンプルでもカウンタが動いていれば流れたと判断すること
    #[test]
    fn first_sample_with_traffic_is_detected() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        observer.observe(&mut lifecycle, &sample(10, 3, 20), at(100));

        assert_eq!(
            lifecycle.first_video_sent_at,
            Some(at(100)),
            "映像の送信が記録されること"
        );
        assert_eq!(
            lifecycle.first_audio_received_at,
            Some(at(100)),
            "音声の受信が記録されること"
        );
        assert_eq!(
            lifecycle.first_delivery_report_at,
            Some(at(100)),
            "SFU からのレポート到着が記録されること"
        );
        assert_eq!(lifecycle.samples, 1, "サンプル数が記録されること");
        assert_eq!(
            lifecycle.last_media_activity_at,
            Some(at(100)),
            "最後にメディアが動いた時刻が記録されること"
        );
        assert_eq!(
            lifecycle.max_idle_samples, 0,
            "増加が無いサンプルは数えないこと"
        );
    }

    /// 増加が無いサンプルが連続したら数えること
    #[test]
    fn idle_samples_are_counted_when_media_stops() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        // メディアが流れている
        observer.observe(&mut lifecycle, &sample(10, 3, 20), at(100));
        // 増加が止まる
        observer.observe(&mut lifecycle, &sample(10, 3, 20), at(200));
        observer.observe(&mut lifecycle, &sample(10, 3, 20), at(300));
        assert_eq!(
            lifecycle.max_idle_samples, 2,
            "増加が無いサンプルが連続して数えられること"
        );

        // 再び流れる
        observer.observe(&mut lifecycle, &sample(20, 6, 40), at(400));
        assert_eq!(
            lifecycle.last_media_activity_at,
            Some(at(400)),
            "最後に動いた時刻が更新されること"
        );

        // 止まると最大値が更新される
        observer.observe(&mut lifecycle, &sample(20, 6, 40), at(500));
        observer.observe(&mut lifecycle, &sample(20, 6, 40), at(600));
        observer.observe(&mut lifecycle, &sample(20, 6, 40), at(700));
        assert_eq!(
            lifecycle.max_idle_samples, 3,
            "最も長く止まった連続サンプル数を保持すること"
        );
        assert!(lifecycle.is_stalled(), "止まった状態として判定されること");
    }

    /// カウンタが 0 のままなら流れたと判断しないこと
    #[test]
    fn zero_counters_are_not_treated_as_traffic() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        observer.observe(&mut lifecycle, &sample(0, 0, 0), at(100));

        assert!(
            lifecycle.first_video_sent_at.is_none() && lifecycle.first_audio_received_at.is_none(),
            "カウンタが 0 のサンプルでは送受信を記録しないこと"
        );
        assert_eq!(
            lifecycle.samples, 1,
            "メディアの統計を含むサンプルとして数えること"
        );
    }

    /// 2 回目のサンプルで増加したら記録されること
    #[test]
    fn increased_counters_are_detected() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        observer.observe(&mut lifecycle, &sample(0, 0, 0), at(100));
        observer.observe(&mut lifecycle, &sample(10, 3, 20), at(200));

        assert_eq!(
            lifecycle.first_video_sent_at,
            Some(at(200)),
            "増加した時点を記録すること"
        );
        assert_eq!(lifecycle.samples, 2, "サンプル数が加算されること");
    }

    /// 増加が無いサンプルでは記録しないこと
    #[test]
    fn unchanged_counters_are_not_detected() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        observer.observe(&mut lifecycle, &sample(10, 3, 20), at(100));
        observer.observe(&mut lifecycle, &sample(10, 3, 20), at(200));

        assert_eq!(
            lifecycle.first_video_sent_at,
            Some(at(100)),
            "初回の観測時刻を保持すること"
        );
    }

    /// カウンタが減った場合はストリームのリセットとして扱うこと
    #[test]
    fn decreased_counters_are_not_treated_as_increase() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        observer.observe(&mut lifecycle, &sample(10, 3, 0), at(100));
        // 再ネゴシエーションで同じ id のカウンタが 0 から始まった状況
        observer.observe(&mut lifecycle, &sample(1, 0, 0), at(200));
        // リセット後に再び増えた状況
        observer.observe(&mut lifecycle, &sample(5, 2, 0), at(300));

        assert_eq!(
            lifecycle.first_video_sent_at,
            Some(at(100)),
            "リセットを増加と誤認せず、最初の観測時刻を保持すること"
        );
    }

    /// メディアの統計を含まないサンプルは数えないこと
    #[test]
    fn sample_without_media_entries_is_not_counted() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        observer.observe(
            &mut lifecycle,
            r#"[{"type":"codec","id":"C1","mimeType":"video/VP8"}]"#,
            at(100),
        );
        assert_eq!(
            lifecycle.samples, 0,
            "メディアの統計が無いサンプルは数えないこと"
        );

        observer.observe(&mut lifecycle, "not json", at(200));
        assert_eq!(lifecycle.samples, 0, "パースできないサンプルは数えないこと");
    }

    /// 映像と音声を分けて記録すること
    #[test]
    fn video_and_audio_are_recorded_separately() {
        let mut observer = MediaObserver::new();
        let mut lifecycle = ConnectionLifecycle::new(at(0));

        observer.observe(
            &mut lifecycle,
            r#"[{"type":"outbound-rtp","id":"OUT1","kind":"audio","packetsSent":5,"bytesSent":50}]"#,
            at(100),
        );

        assert_eq!(
            lifecycle.first_audio_sent_at,
            Some(at(100)),
            "音声の送信が記録されること"
        );
        assert!(
            lifecycle.first_video_sent_at.is_none(),
            "映像は流れていないため記録しないこと"
        );
    }
}
