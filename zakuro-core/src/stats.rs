//! 仮想クライアントの接続状態の集計
//!
//! 仮想クライアントが送る [`StatsEvent`] を集約し、[`StatsSnapshot`] として保持する。
//! 集計結果は 5 秒ごとにログへ出す。

use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::time::MissedTickBehavior;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};
use tokio_util::sync::CancellationToken;

/// 仮想クライアントから集計側へ送る状態変化
#[derive(Debug, Clone, Copy)]
pub enum StatsEvent {
    /// 接続した (MOQ はセッション確立、Sora はシグナリング完了)
    Connected {
        /// インスタンス番号
        instance_id: u32,
        /// 仮想クライアント番号
        vc_id: u32,
    },
    /// 切断した
    Disconnected {
        /// インスタンス番号
        instance_id: u32,
        /// 仮想クライアント番号
        vc_id: u32,
    },
    /// 再接続を待っている
    Retrying {
        /// インスタンス番号
        instance_id: u32,
        /// 仮想クライアント番号
        vc_id: u32,
        /// 通算のリトライ回数
        retry_count: u32,
    },
    /// 再試行せずに終了した
    Stopped {
        /// インスタンス番号
        instance_id: u32,
        /// 仮想クライアント番号
        vc_id: u32,
    },
    /// 接続が終了し、合否が確定した
    ///
    /// 接続単位の合否を判定する実装 (Sora) だけが送る。
    ConnectionEnded {
        /// インスタンス番号
        instance_id: u32,
        /// 仮想クライアント番号
        vc_id: u32,
        /// 判定結果 (`success` / `failure` / `unjudged`)
        outcome: &'static str,
        /// 失敗理由 (成功と判定不能の場合は None)
        failure_reason: Option<&'static str>,
        /// メディアが止まった状態か
        stalled: bool,
        /// 接続の試行から確立までに要した時間
        connect_duration: Option<Duration>,
        /// 接続が終了した時刻 (単調時計)
        ended_at: Instant,
    },
}

/// 集計時点の仮想クライアントの状態
#[derive(Debug, Clone)]
pub struct StatsSnapshot {
    /// 起動予定の仮想クライアント総数
    pub total: u32,
    /// インスタンス数
    pub instances: u32,
    /// 接続中の仮想クライアント数
    pub connected: u32,
    /// 再接続待ちの仮想クライアント数
    pub retrying: u32,
    /// 終了した仮想クライアント数
    pub stopped: u32,
    /// 成功した接続数
    pub success: u32,
    /// 失敗した接続数
    pub failure: u32,
    /// 判定できなかった接続数
    pub unjudged: u32,
    /// メディアが止まった接続数
    pub stalled: u32,
}

impl StatsSnapshot {
    fn initial(total: u32, instances: u32) -> Self {
        Self {
            total,
            instances,
            connected: 0,
            retrying: 0,
            stopped: 0,
            success: 0,
            failure: 0,
            unjudged: 0,
            stalled: 0,
        }
    }

    fn apply(&mut self, event: StatsEvent) {
        match event {
            StatsEvent::Connected { instance_id, vc_id } => {
                log::info!("[i{}/vc-{}][stats] connected", instance_id, vc_id);
                self.connected += 1;
                if self.retrying > 0 {
                    self.retrying -= 1;
                }
            }
            StatsEvent::Disconnected { instance_id, vc_id } => {
                log::info!("[i{}/vc-{}][stats] disconnected", instance_id, vc_id);
                if self.connected > 0 {
                    self.connected -= 1;
                }
            }
            StatsEvent::Retrying {
                instance_id,
                vc_id,
                retry_count,
            } => {
                log::warn!(
                    "[i{}/vc-{}][stats] retrying ({})",
                    instance_id,
                    vc_id,
                    retry_count,
                );
                self.retrying += 1;
            }
            StatsEvent::Stopped { instance_id, vc_id } => {
                log::info!("[i{}/vc-{}][stats] stopped", instance_id, vc_id);
                self.stopped += 1;
                if self.retrying > 0 {
                    self.retrying -= 1;
                }
            }
            StatsEvent::ConnectionEnded {
                outcome, stalled, ..
            } => {
                match outcome {
                    "success" => self.success += 1,
                    "failure" => self.failure += 1,
                    _ => self.unjudged += 1,
                }
                if stalled {
                    self.stalled += 1;
                }
            }
        }
    }
}

/// 試験全体の集計結果
///
/// 集約タスクが終了したときに [`StatsCollector::finalize`] が返す。
#[derive(Debug, Clone, Default)]
pub struct StatsSummary {
    /// 成功した接続数
    pub success: u32,
    /// 失敗した接続数
    pub failure: u32,
    /// 判定できなかった接続数
    pub unjudged: u32,
    /// メディアが止まった接続数
    pub stalled: u32,
    /// 立ち上がり期間のため集計から除外した接続数
    pub warmup_excluded: u32,
    /// 失敗理由ごとの接続数 (接続数の多い順)
    pub failure_reasons: Vec<(&'static str, u32)>,
    /// 確立までの所要時間の p50 / p95 / p99 (ミリ秒)
    pub connect_time_p50_ms: Option<f64>,
    pub connect_time_p95_ms: Option<f64>,
    pub connect_time_p99_ms: Option<f64>,
}

impl StatsSummary {
    /// 合否を判定した接続数 (判定不能を除く)
    pub fn judged(&self) -> u32 {
        self.success + self.failure
    }

    /// 成功した接続の割合
    ///
    /// 判定できなかった接続は分母から外す。判定した接続が 1 本も無い場合は None。
    pub fn success_rate(&self) -> Option<f64> {
        let judged = self.judged();
        if judged == 0 {
            return None;
        }
        Some(f64::from(self.success) / f64::from(judged))
    }
}

/// 合否の集計
///
/// 毎イベント複製される [`StatsSnapshot`] とは別に、集約タスクの中だけで持つ。
#[derive(Debug)]
struct OutcomeTotals {
    /// 判定結果ごとの接続数
    success: u32,
    failure: u32,
    unjudged: u32,
    /// メディアが止まった接続数
    stalled: u32,
    /// 失敗理由ごとの接続数
    failure_reasons: Vec<(&'static str, u32)>,
    /// 確立までの所要時間 (ミリ秒)。パーセンタイルの算出に使う
    connect_times_ms: Vec<f64>,
    /// 立ち上がり期間のため集計から除外した接続数
    warmup_excluded: u32,
}

impl OutcomeTotals {
    fn new() -> Self {
        Self {
            success: 0,
            failure: 0,
            unjudged: 0,
            stalled: 0,
            failure_reasons: Vec::new(),
            connect_times_ms: Vec::new(),
            warmup_excluded: 0,
        }
    }

    /// 立ち上がり期間のため集計から除外したことを記録する
    fn exclude_for_warmup(&mut self) {
        self.warmup_excluded = self.warmup_excluded.saturating_add(1);
    }

    fn apply(&mut self, event: StatsEvent) {
        let StatsEvent::ConnectionEnded {
            outcome,
            failure_reason,
            stalled,
            connect_duration,
            ..
        } = event
        else {
            return;
        };
        match outcome {
            "success" => self.success += 1,
            "failure" => self.failure += 1,
            _ => self.unjudged += 1,
        }
        if stalled {
            self.stalled += 1;
        }
        if let Some(reason) = failure_reason {
            match self
                .failure_reasons
                .iter_mut()
                .find(|(name, _)| *name == reason)
            {
                Some((_, count)) => *count += 1,
                None => self.failure_reasons.push((reason, 1)),
            }
        }
        if let Some(duration) = connect_duration {
            self.connect_times_ms.push(duration.as_secs_f64() * 1000.0);
        }
    }

    /// 集計結果をまとめる
    fn summary(&self) -> StatsSummary {
        let mut failure_reasons = self.failure_reasons.clone();
        // 接続数の多い順に並べる (同数の場合は理由名の昇順で安定させる)
        failure_reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let mut connect_times_ms = self.connect_times_ms.clone();
        connect_times_ms.sort_by(f64::total_cmp);
        StatsSummary {
            success: self.success,
            failure: self.failure,
            unjudged: self.unjudged,
            stalled: self.stalled,
            warmup_excluded: self.warmup_excluded,
            failure_reasons,
            connect_time_p50_ms: percentile(&connect_times_ms, 0.50),
            connect_time_p95_ms: percentile(&connect_times_ms, 0.95),
            connect_time_p99_ms: percentile(&connect_times_ms, 0.99),
        }
    }
}

/// 集計の対象になるイベントか
///
/// 立ち上がり期間 (`warmup`) が経過する前に終了した接続は対象から外す。
/// 接続の合否は終了時点の 1 点で決まるため、測定窓は「終了時刻が `warmup` 以降」と定める。
fn is_counted(event: &StatsEvent, started_at: Instant, warmup: Duration) -> bool {
    if warmup.is_zero() {
        return true;
    }
    let StatsEvent::ConnectionEnded { ended_at, .. } = event else {
        return true;
    };
    ended_at.duration_since(started_at) >= warmup
}

/// 昇順に並んだ値のパーセンタイルを返す
///
/// 値が無い場合は None。最も近い順位の値を返す (補間しない)。
fn percentile(sorted: &[f64], ratio: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    // 最も近い順位 (nearest rank): 順位 = ceil(割合 * 件数)、1 始まり
    let rank = (ratio * sorted.len() as f64).ceil().max(1.0) as usize;
    let index = (rank - 1).min(sorted.len() - 1);
    sorted.get(index).copied()
}

/// 仮想クライアントの状態変化を集約する
///
/// `new` を呼ぶと集約タスクと定期レポートタスクを起動する。`token` がキャンセルされると
/// 両タスクは終了する。
pub struct StatsCollector {
    /// 仮想クライアントが状態変化を送るチャネル
    event_tx: mpsc::Sender<StatsEvent>,
    /// 集約タスクのハンドル (`finalize` で最終結果を受け取る)
    aggregator: tokio::task::JoinHandle<StatsSummary>,
    /// 集約タスクが生きている間だけ保持する (`new` の戻り値で受信側を保持する)
    _snapshot_rx: watch::Receiver<StatsSnapshot>,
}

impl StatsCollector {
    /// 集約タスクとレポータータスクを起動する
    ///
    /// `warmup` は集計から除外する立ち上がり期間。0 を指定すると除外しない。
    /// 試験の開始時刻はこの関数を呼んだ時点とする。
    pub fn new(total: u32, instances: u32, warmup: Duration, token: CancellationToken) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        let (snapshot_tx, snapshot_rx) = watch::channel(StatsSnapshot::initial(total, instances));

        let started_at = Instant::now();
        let aggregator = tokio::spawn(Self::aggregator(
            event_rx,
            snapshot_tx,
            total,
            instances,
            started_at,
            warmup,
            token.clone(),
        ));
        tokio::spawn(Self::reporter(snapshot_rx.clone(), token));

        Self {
            event_tx,
            aggregator,
            _snapshot_rx: snapshot_rx,
        }
    }

    /// 仮想クライアントが状態変化を送るためのチャネルを返す
    pub fn event_tx(&self) -> mpsc::Sender<StatsEvent> {
        self.event_tx.clone()
    }

    /// 集約タスクの終了を待ち、試験全体の集計結果を返す
    ///
    /// 呼び出し側が持つ送信チャネルを閉じ、仮想クライアント側の送信チャネルも
    /// すべて破棄されたあとに呼ぶこと。割り込み (キャンセル) で集約タスクが先に
    /// 終了した場合は、その時点までの集計結果を返す。
    pub async fn finalize(self) -> StatsSummary {
        let Self {
            event_tx,
            aggregator,
            ..
        } = self;
        // 送信側を閉じて、集約タスクが残りのイベントを処理し終えるのを待つ
        drop(event_tx);
        match aggregator.await {
            Ok(summary) => summary,
            Err(e) => {
                log::warn!("[stats] aggregator task failed: {}", e);
                StatsSummary::default()
            }
        }
    }

    async fn aggregator(
        event_rx: mpsc::Receiver<StatsEvent>,
        snapshot_tx: watch::Sender<StatsSnapshot>,
        total: u32,
        instances: u32,
        started_at: Instant,
        warmup: Duration,
        token: CancellationToken,
    ) -> StatsSummary {
        let mut snapshot = StatsSnapshot::initial(total, instances);
        let mut totals = OutcomeTotals::new();
        let mut events = ReceiverStream::new(event_rx);
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                maybe_event = events.next() => {
                    let Some(event) = maybe_event else { break };
                    // 立ち上がり期間に終了した接続は集計から外す (生の記録は残る)
                    if is_counted(&event, started_at, warmup) {
                        snapshot.apply(event);
                        totals.apply(event);
                        let _ = snapshot_tx.send(snapshot.clone());
                    } else {
                        totals.exclude_for_warmup();
                    }
                }
            }
        }
        totals.summary()
    }

    async fn reporter(snapshot_rx: watch::Receiver<StatsSnapshot>, token: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut ticks = IntervalStream::new(interval);
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = ticks.next() => {
                    let snap = snapshot_rx.borrow().clone();
                    log::info!(
                        "[stats] instances={} total={} connected={} retrying={} stopped={} \
                         success={} failure={} unjudged={} stalled={}",
                        snap.instances,
                        snap.total,
                        snap.connected,
                        snap.retrying,
                        snap.stopped,
                        snap.success,
                        snap.failure,
                        snap.unjudged,
                        snap.stalled,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 値が無い場合はパーセンタイルを返さないこと
    #[test]
    fn percentile_returns_none_for_empty_values() {
        assert_eq!(
            percentile(&[], 0.5),
            None,
            "値が 1 つも無い場合は None を返すこと"
        );
    }

    /// 最も近い順位の値を返すこと
    #[test]
    fn percentile_returns_the_nearest_rank() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&values, 0.50), Some(50.0), "p50 を返すこと");
        assert_eq!(percentile(&values, 0.95), Some(95.0), "p95 を返すこと");
        assert_eq!(percentile(&values, 0.99), Some(99.0), "p99 を返すこと");
        assert_eq!(
            percentile(&values, 1.0),
            Some(100.0),
            "最大値は最後の要素を返すこと"
        );
    }

    /// 判定した接続が無い場合は成功接続率を出さないこと
    #[test]
    fn success_rate_is_none_without_judged_connections() {
        let summary = StatsSummary {
            unjudged: 5,
            ..StatsSummary::default()
        };
        assert_eq!(
            summary.success_rate(),
            None,
            "判定不能しかない場合は成功接続率を出さないこと"
        );
    }

    /// 判定不能を分母から外して成功接続率を出すこと
    #[test]
    fn success_rate_excludes_unjudged_connections() {
        let summary = StatsSummary {
            success: 3,
            failure: 1,
            unjudged: 6,
            ..StatsSummary::default()
        };
        assert_eq!(
            summary.success_rate(),
            Some(0.75),
            "判定した接続だけを分母にすること"
        );
    }

    /// 集約タスクが合否を集計してサマリを返すこと
    #[tokio::test]
    async fn collector_summarizes_connection_outcomes() {
        let token = CancellationToken::new();
        let collector = StatsCollector::new(4, 1, Duration::ZERO, token.clone());
        let event_tx = collector.event_tx();

        let events = [
            StatsEvent::ConnectionEnded {
                instance_id: 0,
                vc_id: 0,
                outcome: "success",
                failure_reason: None,
                stalled: false,
                connect_duration: Some(Duration::from_millis(100)),
                ended_at: Instant::now(),
            },
            StatsEvent::ConnectionEnded {
                instance_id: 0,
                vc_id: 1,
                outcome: "success",
                failure_reason: None,
                stalled: true,
                connect_duration: Some(Duration::from_millis(300)),
                ended_at: Instant::now(),
            },
            StatsEvent::ConnectionEnded {
                instance_id: 0,
                vc_id: 2,
                outcome: "failure",
                failure_reason: Some("no-media-sent"),
                stalled: false,
                connect_duration: Some(Duration::from_millis(200)),
                ended_at: Instant::now(),
            },
            StatsEvent::ConnectionEnded {
                instance_id: 0,
                vc_id: 3,
                outcome: "unjudged",
                failure_reason: None,
                stalled: false,
                connect_duration: None,
                ended_at: Instant::now(),
            },
        ];
        for event in events {
            event_tx
                .send(event)
                .await
                .expect("イベントの送信に成功すること");
        }
        drop(event_tx);

        let summary = collector.finalize().await;
        assert_eq!(summary.success, 2, "成功した接続数を数えること");
        assert_eq!(summary.failure, 1, "失敗した接続数を数えること");
        assert_eq!(summary.unjudged, 1, "判定不能の接続数を数えること");
        assert_eq!(summary.judged(), 3, "判定した接続数を数えること");
        assert_eq!(summary.stalled, 1, "停止した接続数を数えること");
        assert_eq!(
            summary.failure_reasons,
            vec![("no-media-sent", 1)],
            "失敗理由ごとの接続数を数えること"
        );
        assert_eq!(
            summary.success_rate(),
            Some(2.0 / 3.0),
            "判定不能を分母から外すこと"
        );
        assert_eq!(
            summary.connect_time_p50_ms,
            Some(200.0),
            "確立までの所要時間の p50 を出すこと"
        );
        assert_eq!(
            summary.connect_time_p95_ms,
            Some(300.0),
            "確立までの所要時間の p95 を出すこと"
        );
        assert_eq!(
            summary.connect_time_p99_ms,
            Some(300.0),
            "確立までの所要時間の p99 を出すこと"
        );
    }

    /// 状態変化のイベントは従来どおり集計されること
    #[tokio::test]
    async fn collector_keeps_counting_state_changes() {
        let token = CancellationToken::new();
        let collector = StatsCollector::new(2, 1, Duration::ZERO, token.clone());
        let event_tx = collector.event_tx();

        event_tx
            .send(StatsEvent::Connected {
                instance_id: 0,
                vc_id: 0,
            })
            .await
            .expect("イベントの送信に成功すること");
        event_tx
            .send(StatsEvent::Stopped {
                instance_id: 0,
                vc_id: 1,
            })
            .await
            .expect("イベントの送信に成功すること");
        drop(event_tx);

        let summary = collector.finalize().await;
        assert_eq!(
            summary.judged(),
            0,
            "合否のイベントが無ければ判定数は 0 であること"
        );
        assert!(
            summary.failure_reasons.is_empty(),
            "失敗理由が無ければ空であること"
        );
        assert_eq!(
            summary.connect_time_p50_ms, None,
            "所要時間が無ければ None であること"
        );
    }

    /// 立ち上がり期間の内側で終了した接続は集計しないこと
    #[test]
    fn connection_ended_within_warmup_is_not_counted() {
        let started_at = Instant::now();
        let warmup = Duration::from_secs(10);
        let event = |ended_at: Instant| StatsEvent::ConnectionEnded {
            instance_id: 0,
            vc_id: 0,
            outcome: "success",
            failure_reason: None,
            stalled: false,
            connect_duration: Some(Duration::from_millis(100)),
            ended_at,
        };

        assert!(
            !is_counted(
                &event(started_at + Duration::from_secs(9)),
                started_at,
                warmup
            ),
            "立ち上がり期間の内側で終了した接続は集計しないこと"
        );
        assert!(
            is_counted(
                &event(started_at + Duration::from_secs(10)),
                started_at,
                warmup
            ),
            "立ち上がり期間ちょうどで終了した接続は集計すること"
        );
        assert!(
            is_counted(
                &event(started_at + Duration::from_secs(11)),
                started_at,
                warmup
            ),
            "立ち上がり期間を過ぎた接続は集計すること"
        );
        assert!(
            is_counted(
                &event(started_at + Duration::from_secs(1)),
                started_at,
                Duration::ZERO
            ),
            "除外期間が 0 の場合は常に集計すること"
        );
    }

    /// 立ち上がり期間に終了した接続は集計から除外されること
    #[tokio::test]
    async fn collector_excludes_warmup_connections() {
        let token = CancellationToken::new();
        // 1 時間を除外期間にすると、この時点で終了した接続は必ず内側になる
        let collector = StatsCollector::new(2, 1, Duration::from_secs(3600), token.clone());
        let event_tx = collector.event_tx();

        event_tx
            .send(StatsEvent::ConnectionEnded {
                instance_id: 0,
                vc_id: 0,
                outcome: "failure",
                failure_reason: Some("connect-failed"),
                stalled: false,
                connect_duration: None,
                ended_at: Instant::now(),
            })
            .await
            .expect("イベントの送信に成功すること");
        drop(event_tx);

        let summary = collector.finalize().await;
        assert_eq!(
            summary.judged(),
            0,
            "立ち上がり期間の接続は合否の集計に含めないこと"
        );
        assert!(
            summary.failure_reasons.is_empty(),
            "立ち上がり期間の接続は失敗理由にも含めないこと"
        );
        assert_eq!(summary.warmup_excluded, 1, "除外した接続数を数えること");
    }
}
