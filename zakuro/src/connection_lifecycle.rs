//! 接続 1 本のライフサイクル
//!
//! 試行開始から切断までに観測した時刻と状態を保持する。Sora のシグナリングが
//! 成立した時点 (offer の受信) と、WebRTC の接続が確立した時点
//! (PeerConnection が Connected になった時点) を別々に記録する。
//! どちらで止まったのかを後から切り分けるための材料になる。

use std::time::{Duration, SystemTime};

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

/// 接続の判定結果
///
/// 「接続が確立し、メディアが流れていることを観測できたか」を表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionOutcome {
    /// 確立し、必要なメディアの観測が揃った
    Success,
    /// 判定した結果、失敗だった
    Failure(ConnectionFailure),
    /// 判定できない (統計サンプルが無い、確立後の猶予が足りないなど)
    Unjudged,
}

impl ConnectionOutcome {
    /// DuckDB へ記録する文字列を返す
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure(_) => "failure",
            Self::Unjudged => "unjudged",
        }
    }

    /// 失敗理由を返す (成功と判定不能の場合は None)
    pub(crate) fn failure_reason(self) -> Option<&'static str> {
        match self {
            Self::Failure(reason) => Some(reason.as_str()),
            Self::Success | Self::Unjudged => None,
        }
    }
}

/// 接続が失敗した理由
///
/// 判定は優先順に行い、最初に当てはまった理由を 1 つだけ記録する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionFailure {
    /// 接続の構築に失敗した
    BuildFailed,
    /// 接続を確立できなかった
    ConnectFailed,
    /// 確立したが、送るはずの種別のメディアが流れなかった
    NoMediaSent,
    /// 送信はあったが SFU からのレポートが届かなかった
    NoDeliveryReport,
    /// 確立したが、受けるはずの種別のメディアが届かなかった
    NoMediaReceived,
    /// メディアの観測は満たしたが、想定外の切断が起きた
    UnexpectedDisconnect,
}

impl ConnectionFailure {
    /// DuckDB へ記録する文字列を返す
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BuildFailed => "build-failed",
            Self::ConnectFailed => "connect-failed",
            Self::NoMediaSent => "no-media-sent",
            Self::NoDeliveryReport => "no-delivery-report",
            Self::NoMediaReceived => "no-media-received",
            Self::UnexpectedDisconnect => "unexpected-disconnect",
        }
    }
}

/// 判定に使う接続の設定
///
/// ロールと映像 / 音声の有効 / 無効は接続ごとに変わらないため、判定の引数として渡す。
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutcomeSettings {
    /// 確立後にメディアの観測を判定するまでの猶予
    ///
    /// 確立直後はメディアが流れ始めるまで時間がかかるため、猶予の間に終了した接続は
    /// 「メディアが流れなかった」ではなく判定不能として扱う。
    pub(crate) grace: Duration,
    /// 送信を期待するロールか
    pub(crate) expects_send: bool,
    /// 受信を期待するロールか
    pub(crate) expects_receive: bool,
    /// 映像が有効か
    pub(crate) video_enabled: bool,
    /// 音声が有効か
    pub(crate) audio_enabled: bool,
}

/// メディアの観測を待つ猶予の既定値
pub(crate) const OUTCOME_GRACE: Duration = Duration::from_secs(10);

/// メディアが止まったとみなす連続サンプル数
pub(crate) const STALL_SAMPLES: u32 = 3;

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
    /// 最後にメディアの増加を観測した時刻
    pub(crate) last_media_activity_at: Option<SystemTime>,
    /// 連続して増加が観測されなかったサンプル数の最大値
    pub(crate) max_idle_samples: u32,
    /// 現在連続している、増加が観測されなかったサンプル数
    idle_samples: u32,
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
            last_media_activity_at: None,
            max_idle_samples: 0,
            idle_samples: 0,
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

    /// 統計サンプル 1 件でメディアの増加を観測できたかを記録する
    ///
    /// 増加が無かったサンプルが連続するほど「メディアが止まっている」可能性が高くなる。
    /// 止まったかどうかの判定は [`Self::is_stalled`] で行う。
    pub(crate) fn on_sample_activity(&mut self, observed_at: SystemTime, has_activity: bool) {
        if has_activity {
            self.idle_samples = 0;
            self.last_media_activity_at = Some(observed_at);
        } else {
            self.idle_samples = self.idle_samples.saturating_add(1);
            self.max_idle_samples = self.max_idle_samples.max(self.idle_samples);
        }
    }

    /// メディアが止まった状態か
    ///
    /// 動いていたメディアが [`STALL_SAMPLES`] 回連続で増加しなくなった場合に true になる。
    /// 一度も流れていない接続は「送受信が無い」失敗として判定されるため、ここでは扱わない。
    pub(crate) fn is_stalled(&self) -> bool {
        self.last_media_activity_at.is_some() && self.max_idle_samples >= STALL_SAMPLES
    }

    /// 接続の判定結果を返す
    ///
    /// 判定は次の優先順で行う。最初に当てはまったものを結果とする。
    ///
    /// 1. 接続を開始できなかった場合は失敗
    /// 2. 確立できなかった場合は失敗
    /// 3. 判定に使えるメディアが無い、統計サンプルが無い、確立後の猶予が足りない場合は判定不能
    /// 4. 送るはずの種別が流れていない場合は失敗
    /// 5. SFU からのレポートが届いていない場合は失敗
    /// 6. 受けるはずの種別が届いていない場合は失敗
    /// 7. 想定外の切断が起きた場合は失敗
    /// 8. それ以外は成功
    ///
    /// 時刻は呼び出し側から渡さず、記録済みの時刻だけで判定する。
    pub(crate) fn judge(&self, settings: &OutcomeSettings) -> ConnectionOutcome {
        if self.end == Some(LifecycleEnd::BuildFailed) {
            return ConnectionOutcome::Failure(ConnectionFailure::BuildFailed);
        }
        let Some(connected_at) = self.webrtc_connected_at else {
            return ConnectionOutcome::Failure(ConnectionFailure::ConnectFailed);
        };
        // 映像も音声も無効な接続は、観測できるメディアが無いため判定できない
        if !settings.video_enabled && !settings.audio_enabled {
            return ConnectionOutcome::Unjudged;
        }
        // 統計サンプルが 1 件も無い場合は、送受信の有無を判定できない
        if self.samples == 0 {
            return ConnectionOutcome::Unjudged;
        }
        let ended_at = self.disconnected_at.unwrap_or(connected_at);
        if ended_at.duration_since(connected_at).unwrap_or_default() < settings.grace {
            return ConnectionOutcome::Unjudged;
        }

        if settings.expects_send {
            if !self.has_sent_every_enabled_kind(settings) {
                return ConnectionOutcome::Failure(ConnectionFailure::NoMediaSent);
            }
            if self.first_delivery_report_at.is_none() {
                return ConnectionOutcome::Failure(ConnectionFailure::NoDeliveryReport);
            }
        }
        if settings.expects_receive && !self.has_received_every_enabled_kind(settings) {
            return ConnectionOutcome::Failure(ConnectionFailure::NoMediaReceived);
        }
        if self.end == Some(LifecycleEnd::Unexpected) {
            return ConnectionOutcome::Failure(ConnectionFailure::UnexpectedDisconnect);
        }
        ConnectionOutcome::Success
    }

    /// 有効な種別すべての送信を観測できたか
    fn has_sent_every_enabled_kind(&self, settings: &OutcomeSettings) -> bool {
        (!settings.video_enabled || self.first_video_sent_at.is_some())
            && (!settings.audio_enabled || self.first_audio_sent_at.is_some())
    }

    /// 有効な種別すべての受信を観測できたか
    fn has_received_every_enabled_kind(&self, settings: &OutcomeSettings) -> bool {
        (!settings.video_enabled || self.first_video_received_at.is_some())
            && (!settings.audio_enabled || self.first_audio_received_at.is_some())
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

    /// 判定に使う設定を作る (映像と音声は有効)
    fn settings(expects_send: bool, expects_receive: bool) -> OutcomeSettings {
        OutcomeSettings {
            grace: OUTCOME_GRACE,
            expects_send,
            expects_receive,
            video_enabled: true,
            audio_enabled: true,
        }
    }

    /// 確立済みで猶予を超えて生きているライフサイクルを作る
    ///
    /// 統計サンプルを 1 件取り込んだ状態にするため、判定不能にはならない。
    fn established_lifecycle() -> ConnectionLifecycle {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_offer_received(at(1));
        lifecycle.on_peer_connection_state(PeerConnectionState::Connected, at(2));
        lifecycle.on_sample_observed();
        lifecycle.on_sample_activity(at(30), true);
        lifecycle.mark_disconnected(LifecycleEnd::DurationExpired, at(30));
        lifecycle
    }

    /// 送信の観測を満たす (映像と音声の送信 + SFU からのレポート)
    fn fill_sent(lifecycle: &mut ConnectionLifecycle) {
        lifecycle.on_media_sent(MediaKind::Video, at(5));
        lifecycle.on_media_sent(MediaKind::Audio, at(5));
        lifecycle.on_delivery_report(at(6));
    }

    /// 接続を開始できなかった場合は build-failed になること
    #[test]
    fn judge_reports_build_failure() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.mark_disconnected(LifecycleEnd::BuildFailed, at(1));
        assert_eq!(
            lifecycle.judge(&settings(true, false)),
            ConnectionOutcome::Failure(ConnectionFailure::BuildFailed),
            "構築に失敗した接続は build-failed と判定すること"
        );
    }

    /// 確立できなかった場合は connect-failed になること
    #[test]
    fn judge_reports_connect_failure() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_offer_received(at(1));
        lifecycle.mark_disconnected(LifecycleEnd::DurationExpired, at(30));
        assert_eq!(
            lifecycle.judge(&settings(true, false)),
            ConnectionOutcome::Failure(ConnectionFailure::ConnectFailed),
            "確立できなかった接続は connect-failed と判定すること"
        );
    }

    /// 送信とレポート到着を観測した接続は成功と判定すること
    #[test]
    fn judge_reports_success_for_sendonly() {
        let mut lifecycle = established_lifecycle();
        fill_sent(&mut lifecycle);
        assert_eq!(
            lifecycle.judge(&settings(true, false)),
            ConnectionOutcome::Success,
            "送信とレポート到着を観測した接続は成功と判定すること"
        );
    }

    /// 送るはずの種別が流れていない場合は no-media-sent になること
    #[test]
    fn judge_reports_no_media_sent() {
        let mut lifecycle = established_lifecycle();
        lifecycle.on_media_sent(MediaKind::Audio, at(5));
        lifecycle.on_delivery_report(at(6));
        assert_eq!(
            lifecycle.judge(&settings(true, false)),
            ConnectionOutcome::Failure(ConnectionFailure::NoMediaSent),
            "有効な種別の送信が欠けていれば no-media-sent と判定すること"
        );
    }

    /// 送信はあったがレポートが届かない場合は no-delivery-report になること
    #[test]
    fn judge_reports_no_delivery_report() {
        let mut lifecycle = established_lifecycle();
        lifecycle.on_media_sent(MediaKind::Video, at(5));
        lifecycle.on_media_sent(MediaKind::Audio, at(5));
        assert_eq!(
            lifecycle.judge(&settings(true, false)),
            ConnectionOutcome::Failure(ConnectionFailure::NoDeliveryReport),
            "SFU からのレポートが無ければ no-delivery-report と判定すること"
        );
    }

    /// 受信を期待するのに届かない場合は no-media-received になること
    #[test]
    fn judge_reports_no_media_received() {
        let lifecycle = established_lifecycle();
        assert_eq!(
            lifecycle.judge(&settings(false, true)),
            ConnectionOutcome::Failure(ConnectionFailure::NoMediaReceived),
            "有効な種別の受信が欠けていれば no-media-received と判定すること"
        );
    }

    /// 送受信の観測を満たしていても想定外の切断なら失敗と判定すること
    #[test]
    fn judge_reports_unexpected_disconnect() {
        let mut lifecycle = established_lifecycle();
        fill_sent(&mut lifecycle);
        lifecycle.on_media_received(MediaKind::Video, at(5));
        lifecycle.on_media_received(MediaKind::Audio, at(5));
        lifecycle.mark_disconnected(LifecycleEnd::DurationExpired, at(30));
        assert_eq!(
            lifecycle.judge(&settings(true, true)),
            ConnectionOutcome::Success,
            "予定どおりの切断なら成功と判定すること"
        );

        // 同じ接続のまま終了理由だけを想定外の切断に変えて判定する
        lifecycle.end = Some(LifecycleEnd::Unexpected);
        assert_eq!(
            lifecycle.judge(&settings(true, true)),
            ConnectionOutcome::Failure(ConnectionFailure::UnexpectedDisconnect),
            "想定外の切断は unexpected-disconnect と判定すること"
        );
    }

    /// 統計サンプルが無い接続は判定不能になること
    #[test]
    fn judge_is_unjudged_without_samples() {
        let mut lifecycle = established_lifecycle();
        lifecycle.samples = 0;
        assert_eq!(
            lifecycle.judge(&settings(true, false)),
            ConnectionOutcome::Unjudged,
            "統計サンプルが無い接続は判定不能とすること"
        );
    }

    /// 確立から猶予以内に終了した接続は判定不能になること
    #[test]
    fn judge_is_unjudged_within_grace() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        lifecycle.on_peer_connection_state(PeerConnectionState::Connected, at(100));
        lifecycle.on_sample_observed();
        lifecycle.on_sample_activity(at(101), false);
        lifecycle.mark_disconnected(LifecycleEnd::DurationExpired, at(105));
        assert_eq!(
            lifecycle.judge(&settings(true, false)),
            ConnectionOutcome::Unjudged,
            "猶予の間に終了した接続は判定不能とすること"
        );
    }

    /// 映像を無効にした接続では音声だけで成功と判定すること
    #[test]
    fn judge_accepts_audio_only() {
        let mut lifecycle = established_lifecycle();
        lifecycle.on_media_sent(MediaKind::Audio, at(5));
        lifecycle.on_delivery_report(at(6));
        let settings = OutcomeSettings {
            video_enabled: false,
            ..settings(true, false)
        };
        assert_eq!(
            lifecycle.judge(&settings),
            ConnectionOutcome::Success,
            "映像を無効にした接続では音声の送信だけで成功と判定すること"
        );
    }

    /// 映像も音声も無効な接続は判定不能になること
    #[test]
    fn judge_is_unjudged_when_no_media_is_enabled() {
        let lifecycle = established_lifecycle();
        let settings = OutcomeSettings {
            video_enabled: false,
            audio_enabled: false,
            ..settings(true, false)
        };
        assert_eq!(
            lifecycle.judge(&settings),
            ConnectionOutcome::Unjudged,
            "観測できるメディアが無い接続は判定不能とすること"
        );
    }

    /// メディアが止まった状態を検出すること
    #[test]
    fn stall_is_detected_after_consecutive_idle_samples() {
        let mut lifecycle = established_lifecycle();
        assert!(
            !lifecycle.is_stalled(),
            "動き続けている接続を止まった状態としないこと"
        );

        lifecycle.on_sample_activity(at(40), false);
        lifecycle.on_sample_activity(at(50), false);
        assert!(
            !lifecycle.is_stalled(),
            "連続 2 サンプルでは止まった状態としないこと"
        );

        lifecycle.on_sample_activity(at(60), false);
        assert!(
            lifecycle.is_stalled(),
            "連続 3 サンプルで止まった状態とすること"
        );
        assert_eq!(
            lifecycle.last_media_activity_at,
            Some(at(30)),
            "最後に動いた時刻を保持すること"
        );

        lifecycle.on_sample_activity(at(70), true);
        assert_eq!(
            lifecycle.max_idle_samples, 3,
            "最大の連続サンプル数を保持すること"
        );
    }

    /// 一度も流れていない接続は止まった状態としないこと
    #[test]
    fn stall_is_not_detected_without_activity() {
        let mut lifecycle = ConnectionLifecycle::new(at(0));
        for observed_at in [10, 20, 30, 40] {
            lifecycle.on_sample_activity(at(observed_at), false);
        }
        assert!(
            !lifecycle.is_stalled(),
            "一度も流れていない接続は no-media-sent として扱うこと"
        );
        assert_eq!(
            lifecycle.max_idle_samples, 4,
            "連続サンプル数は記録されること"
        );
    }
}
