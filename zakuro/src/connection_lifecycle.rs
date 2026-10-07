//! 接続 1 本のライフサイクル
//!
//! 試行開始から切断までに観測した時刻と状態を保持する。Sora のシグナリングが
//! 成立した時点 (offer の受信) と、WebRTC の接続が確立した時点
//! (PeerConnection が Connected になった時点) を別々に記録する。
//! どちらで止まったのかを後から切り分けるための材料になる。

use std::time::SystemTime;

use shiguredo_webrtc::{
    IceConnectionState, IceGatheringState, PeerConnectionState, SignalingState,
};

/// 接続が終了した理由
///
/// 負荷試験の合否判定とは独立に、「なぜこの接続が終わったのか」という事実だけを表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleEnd {
    /// 接続の構築に失敗した (接続そのものを開始できなかった)
    BuildFailed,
    /// プロセス全体の停止 (Ctrl+C など)
    Shutdown,
    /// `--duration` の経過
    DurationExpired,
    /// シナリオの切断操作 (Reconnect / Disconnect)
    ScenarioDisconnect,
    /// シナリオの Exit 操作
    ScenarioExit,
    /// 予期しない切断
    Unexpected,
}

impl LifecycleEnd {
    /// DuckDB へ記録する文字列を返す
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BuildFailed => "build-failed",
            Self::Shutdown => "shutdown",
            Self::DurationExpired => "duration-expired",
            Self::ScenarioDisconnect => "scenario-disconnect",
            Self::ScenarioExit => "scenario-exit",
            Self::Unexpected => "unexpected",
        }
    }
}

/// メディアの種別
///
/// 映像と音声を分けて記録する。映像トラックだけが拒否される (音声だけ流れる)
/// といった失敗を切り分けるために必要になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaKind {
    /// 映像
    Video,
    /// 音声
    Audio,
}

/// 接続 1 本のライフサイクル
///
/// 時刻の更新はこのモジュールのメソッド経由で行う。同じ状態が複数回通知されても
/// (Connected の再通知など) 最初に観測した時刻を上書きしない。
///
/// 観測時刻は呼び出し側から渡す。ハンドラは `SystemTime::now()` を渡し、
/// テストは固定値を渡すことで、時刻に依存しない検証ができる。
#[derive(Debug, Clone)]
pub(crate) struct ConnectionLifecycle {
    /// 接続の試行を開始した時刻
    pub(crate) attempt_started_at: SystemTime,
    /// offer を受信して connection_id / session_id を確定した時刻 (Sora のシグナリング成立)
    pub(crate) offer_received_at: Option<SystemTime>,
    /// PeerConnection が Connected になった時刻 (ICE と DTLS を含めた WebRTC の確立)
    pub(crate) webrtc_connected_at: Option<SystemTime>,
    /// ICE が接続済みになった時刻 (Connected / Completed のいずれか早い方)
    pub(crate) ice_connected_at: Option<SystemTime>,
    /// ICE の候補収集が完了した時刻
    pub(crate) ice_gathering_complete_at: Option<SystemTime>,
    /// 映像のパケットが流れたことを最初に観測した時刻
    pub(crate) first_video_sent_at: Option<SystemTime>,
    /// 映像のパケットが届いたことを最初に観測した時刻
    pub(crate) first_video_received_at: Option<SystemTime>,
    /// 音声のパケットが流れたことを最初に観測した時刻
    pub(crate) first_audio_sent_at: Option<SystemTime>,
    /// 音声のパケットが届いたことを最初に観測した時刻
    pub(crate) first_audio_received_at: Option<SystemTime>,
    /// SFU からのレポート (remote-inbound-rtp) を最初に観測した時刻
    ///
    /// 送信したパケットが SFU に届いた証拠になる。
    pub(crate) first_delivery_report_at: Option<SystemTime>,
    /// 判定に使った統計サンプル数
    pub(crate) samples: u32,
    /// 接続が終了した時刻
    pub(crate) disconnected_at: Option<SystemTime>,
    /// 接続が終了した理由
    pub(crate) end: Option<LifecycleEnd>,
    /// 最後に観測した PeerConnection の状態
    pub(crate) peer_connection_state: Option<&'static str>,
    /// 最後に観測した ICE 接続の状態
    pub(crate) ice_connection_state: Option<&'static str>,
    /// 最後に観測した ICE 候補収集の状態
    pub(crate) ice_gathering_state: Option<&'static str>,
    /// 最後に観測したシグナリングの状態
    pub(crate) signaling_state: Option<&'static str>,
}

impl ConnectionLifecycle {
    /// 試行開始時刻だけを持つライフサイクルを作る
    pub(crate) fn new(attempt_started_at: SystemTime) -> Self {
        Self {
            attempt_started_at,
            offer_received_at: None,
            webrtc_connected_at: None,
            ice_connected_at: None,
            ice_gathering_complete_at: None,
            first_video_sent_at: None,
            first_video_received_at: None,
            first_audio_sent_at: None,
            first_audio_received_at: None,
            first_delivery_report_at: None,
            samples: 0,
            disconnected_at: None,
            end: None,
            peer_connection_state: None,
            ice_connection_state: None,
            ice_gathering_state: None,
            signaling_state: None,
        }
    }

    /// offer の受信を記録する
    pub(crate) fn on_offer_received(&mut self, observed_at: SystemTime) {
        if self.offer_received_at.is_none() {
            self.offer_received_at = Some(observed_at);
        }
    }

    /// PeerConnection の状態変化を記録する
    ///
    /// Connected を最初に観測した時刻を WebRTC の確立時刻として保持する。
    pub(crate) fn on_peer_connection_state(
        &mut self,
        state: PeerConnectionState,
        observed_at: SystemTime,
    ) {
        self.peer_connection_state = Some(peer_connection_state_name(state));
        if state == PeerConnectionState::Connected && self.webrtc_connected_at.is_none() {
            self.webrtc_connected_at = Some(observed_at);
        }
    }

    /// ICE 接続の状態変化を記録する
    ///
    /// Connected / Completed を最初に観測した時刻を ICE の接続時刻として保持する。
    pub(crate) fn on_ice_connection_state(
        &mut self,
        state: IceConnectionState,
        observed_at: SystemTime,
    ) {
        self.ice_connection_state = Some(ice_connection_state_name(state));
        let connected = matches!(
            state,
            IceConnectionState::Connected | IceConnectionState::Completed
        );
        if connected && self.ice_connected_at.is_none() {
            self.ice_connected_at = Some(observed_at);
        }
    }

    /// ICE 候補収集の状態変化を記録する
    ///
    /// Complete を最初に観測した時刻を候補収集の完了時刻として保持する。
    pub(crate) fn on_ice_gathering_state(
        &mut self,
        state: IceGatheringState,
        observed_at: SystemTime,
    ) {
        self.ice_gathering_state = Some(ice_gathering_state_name(state));
        if state == IceGatheringState::Complete && self.ice_gathering_complete_at.is_none() {
            self.ice_gathering_complete_at = Some(observed_at);
        }
    }

    /// シグナリングの状態変化を記録する
    ///
    /// 最後に観測した状態のみを保持する (所要時間の算出には使わない)。
    pub(crate) fn on_signaling_state(&mut self, state: SignalingState) {
        self.signaling_state = Some(signaling_state_name(state));
    }

    /// 統計サンプルを 1 件取り込んだことを記録する
    pub(crate) fn on_sample_observed(&mut self) {
        self.samples = self.samples.saturating_add(1);
    }

    /// 指定した種別のメディアが送信されたことを記録する
    pub(crate) fn on_media_sent(&mut self, kind: MediaKind, observed_at: SystemTime) {
        let slot = match kind {
            MediaKind::Video => &mut self.first_video_sent_at,
            MediaKind::Audio => &mut self.first_audio_sent_at,
        };
        if slot.is_none() {
            *slot = Some(observed_at);
        }
    }

    /// 指定した種別のメディアが受信されたことを記録する
    pub(crate) fn on_media_received(&mut self, kind: MediaKind, observed_at: SystemTime) {
        let slot = match kind {
            MediaKind::Video => &mut self.first_video_received_at,
            MediaKind::Audio => &mut self.first_audio_received_at,
        };
        if slot.is_none() {
            *slot = Some(observed_at);
        }
    }

    /// SFU からのレポートの到着を記録する
    pub(crate) fn on_delivery_report(&mut self, observed_at: SystemTime) {
        if self.first_delivery_report_at.is_none() {
            self.first_delivery_report_at = Some(observed_at);
        }
    }

    /// 接続の終了を記録する
    ///
    /// 最初に観測した終了理由と時刻を保持する。2 回目以降の通知は
    /// 最初の終了要因に付随する後続の状態変化として扱い、上書きしない。
    pub(crate) fn mark_disconnected(&mut self, end: LifecycleEnd, observed_at: SystemTime) {
        if self.end.is_none() {
            self.end = Some(end);
        }
        if self.disconnected_at.is_none() {
            self.disconnected_at = Some(observed_at);
        }
    }
}

/// PeerConnection の状態を DuckDB に記録する文字列へ変換する
fn peer_connection_state_name(state: PeerConnectionState) -> &'static str {
    match state {
        PeerConnectionState::New => "new",
        PeerConnectionState::Connecting => "connecting",
        PeerConnectionState::Connected => "connected",
        PeerConnectionState::Disconnected => "disconnected",
        PeerConnectionState::Failed => "failed",
        PeerConnectionState::Closed => "closed",
        PeerConnectionState::Unknown(_) => "unknown",
    }
}

/// ICE 接続の状態を DuckDB に記録する文字列へ変換する
fn ice_connection_state_name(state: IceConnectionState) -> &'static str {
    match state {
        IceConnectionState::New => "new",
        IceConnectionState::Checking => "checking",
        IceConnectionState::Connected => "connected",
        IceConnectionState::Completed => "completed",
        IceConnectionState::Failed => "failed",
        IceConnectionState::Disconnected => "disconnected",
        IceConnectionState::Closed => "closed",
        // Max は C++ 側の番兵値であり、状態としては現れない
        IceConnectionState::Max => "max",
        IceConnectionState::Unknown(_) => "unknown",
    }
}

/// ICE 候補収集の状態を DuckDB に記録する文字列へ変換する
fn ice_gathering_state_name(state: IceGatheringState) -> &'static str {
    match state {
        IceGatheringState::New => "new",
        IceGatheringState::Gathering => "gathering",
        IceGatheringState::Complete => "complete",
        IceGatheringState::Unknown(_) => "unknown",
    }
}

/// シグナリングの状態を DuckDB に記録する文字列へ変換する
fn signaling_state_name(state: SignalingState) -> &'static str {
    match state {
        SignalingState::Stable => "stable",
        SignalingState::HaveLocalOffer => "have-local-offer",
        SignalingState::HaveRemoteOffer => "have-remote-offer",
        SignalingState::HaveLocalPranswer => "have-local-pranswer",
        SignalingState::HaveRemotePranswer => "have-remote-pranswer",
        SignalingState::Closed => "closed",
        SignalingState::Unknown(_) => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定の基準時刻からの経過秒で SystemTime を作る
    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
    }

    /// 生成直後は試行開始時刻だけが入っていること
    #[test]
    fn new_records_only_attempt_started_at() {
        let lifecycle = ConnectionLifecycle::new(at(100));
        assert_eq!(
            lifecycle.attempt_started_at,
            at(100),
            "試行開始時刻が設定されること"
        );
        assert!(
            lifecycle.offer_received_at.is_none() && lifecycle.webrtc_connected_at.is_none(),
            "確立に関する時刻は未設定であること"
        );
        assert!(
            lifecycle.end.is_none() && lifecycle.disconnected_at.is_none(),
            "終了に関する情報は未設定であること"
        );
    }

    /// offer の受信は最初の 1 回だけ記録されること
    #[test]
    fn offer_received_keeps_first_observation() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_offer_received(at(10));
        lifecycle.on_offer_received(at(20));
        assert_eq!(
            lifecycle.offer_received_at,
            Some(at(10)),
            "最初に観測した offer 受信時刻を保持すること"
        );
    }

    /// WebRTC の確立は Connected の初回だけ記録されること
    #[test]
    fn webrtc_connected_keeps_first_observation() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_peer_connection_state(PeerConnectionState::New, at(1));
        lifecycle.on_peer_connection_state(PeerConnectionState::Connecting, at(2));
        lifecycle.on_peer_connection_state(PeerConnectionState::Connected, at(3));
        lifecycle.on_peer_connection_state(PeerConnectionState::Connected, at(9));
        assert_eq!(
            lifecycle.webrtc_connected_at,
            Some(at(3)),
            "最初に Connected を観測した時刻を保持すること"
        );
        assert_eq!(
            lifecycle.peer_connection_state,
            Some("connected"),
            "最後に観測した状態を保持すること"
        );
    }

    /// Failed / Disconnected では確立時刻を記録しないこと
    #[test]
    fn peer_connection_failure_does_not_record_establishment() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_peer_connection_state(PeerConnectionState::Connecting, at(1));
        lifecycle.on_peer_connection_state(PeerConnectionState::Failed, at(2));
        assert_eq!(
            lifecycle.webrtc_connected_at, None,
            "Connected を観測していなければ確立時刻は未設定であること"
        );
        assert_eq!(
            lifecycle.peer_connection_state,
            Some("failed"),
            "失敗状態は最後の観測値として残ること"
        );
    }

    /// ICE は Connected / Completed の早い方を記録すること
    #[test]
    fn ice_connected_records_the_earlier_state() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_ice_connection_state(IceConnectionState::Checking, at(1));
        lifecycle.on_ice_connection_state(IceConnectionState::Connected, at(2));
        lifecycle.on_ice_connection_state(IceConnectionState::Completed, at(3));
        assert_eq!(
            lifecycle.ice_connected_at,
            Some(at(2)),
            "Connected を最初に観測した時刻を保持すること"
        );
        assert_eq!(
            lifecycle.ice_connection_state,
            Some("completed"),
            "最後に観測した状態を保持すること"
        );
    }

    /// 候補収集の完了は Complete の初回だけ記録されること
    #[test]
    fn ice_gathering_complete_keeps_first_observation() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_ice_gathering_state(IceGatheringState::Gathering, at(1));
        lifecycle.on_ice_gathering_state(IceGatheringState::Complete, at(2));
        lifecycle.on_ice_gathering_state(IceGatheringState::Complete, at(5));
        assert_eq!(
            lifecycle.ice_gathering_complete_at,
            Some(at(2)),
            "最初に Complete を観測した時刻を保持すること"
        );
    }

    /// 終了は最初の 1 回だけ記録されること
    #[test]
    fn disconnected_keeps_first_observation() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.mark_disconnected(LifecycleEnd::DurationExpired, at(30));
        lifecycle.mark_disconnected(LifecycleEnd::Unexpected, at(40));
        assert_eq!(
            lifecycle.disconnected_at,
            Some(at(30)),
            "最初に観測した切断時刻を保持すること"
        );
        assert_eq!(
            lifecycle.end,
            Some(LifecycleEnd::DurationExpired),
            "最初に観測した終了理由を保持すること"
        );
    }

    /// 終了理由が DuckDB 用の文字列へ変換されること
    #[test]
    fn lifecycle_end_strings_are_stable() {
        let cases = [
            (LifecycleEnd::BuildFailed, "build-failed"),
            (LifecycleEnd::Shutdown, "shutdown"),
            (LifecycleEnd::DurationExpired, "duration-expired"),
            (LifecycleEnd::ScenarioDisconnect, "scenario-disconnect"),
            (LifecycleEnd::ScenarioExit, "scenario-exit"),
            (LifecycleEnd::Unexpected, "unexpected"),
        ];
        for (end, expected) in cases {
            assert_eq!(end.as_str(), expected, "終了理由の文字列が一致すること");
        }
    }

    /// シグナリングの状態は最後の観測値だけを保持すること
    #[test]
    fn signaling_state_keeps_the_last_observation() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_signaling_state(SignalingState::HaveRemoteOffer);
        lifecycle.on_signaling_state(SignalingState::Stable);
        assert_eq!(
            lifecycle.signaling_state,
            Some("stable"),
            "最後に観測した状態を保持すること"
        );
    }

    /// メディアの初回観測は種別ごとに 1 度だけ記録されること
    #[test]
    fn media_observation_keeps_the_first_time_per_kind() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_media_sent(MediaKind::Video, at(10));
        lifecycle.on_media_sent(MediaKind::Video, at(20));
        lifecycle.on_media_received(MediaKind::Audio, at(30));
        lifecycle.on_media_received(MediaKind::Audio, at(40));

        assert_eq!(
            lifecycle.first_video_sent_at,
            Some(at(10)),
            "映像の送信は最初の観測時刻を保持すること"
        );
        assert_eq!(
            lifecycle.first_audio_received_at,
            Some(at(30)),
            "音声の受信は最初の観測時刻を保持すること"
        );
        assert!(
            lifecycle.first_video_received_at.is_none() && lifecycle.first_audio_sent_at.is_none(),
            "観測していない種別の時刻は未設定であること"
        );
    }

    /// 統計サンプル数とレポート到着が記録されること
    #[test]
    fn sample_count_and_delivery_report_are_recorded() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_sample_observed();
        lifecycle.on_sample_observed();
        lifecycle.on_delivery_report(at(50));
        lifecycle.on_delivery_report(at(60));

        assert_eq!(lifecycle.samples, 2, "サンプル数が加算されること");
        assert_eq!(
            lifecycle.first_delivery_report_at,
            Some(at(50)),
            "レポート到着は最初の観測時刻を保持すること"
        );
    }
}
