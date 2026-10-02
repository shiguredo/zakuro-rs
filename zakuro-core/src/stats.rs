//! 仮想クライアントの接続状態の集計
//!
//! 仮想クライアントが送る [`StatsEvent`] を集約し、[`StatsSnapshot`] として保持する。
//! 集計結果は 5 秒ごとにログへ出す。

use std::time::Duration;

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
}

impl StatsSnapshot {
    fn initial(total: u32, instances: u32) -> Self {
        Self {
            total,
            instances,
            connected: 0,
            retrying: 0,
            stopped: 0,
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
        }
    }
}

/// 仮想クライアントの状態変化を集約する
///
/// `new` を呼ぶと集約タスクと定期レポートタスクを起動する。`token` がキャンセルされると
/// 両タスクは終了する。
pub struct StatsCollector {
    /// 仮想クライアントが状態変化を送るチャネル
    event_tx: mpsc::Sender<StatsEvent>,
    /// 集約タスクが生きている間だけ保持する (`new` の戻り値で受信側を保持する)
    _snapshot_rx: watch::Receiver<StatsSnapshot>,
}

impl StatsCollector {
    /// 集約タスクとレポータータスクを起動する
    pub fn new(total: u32, instances: u32, token: CancellationToken) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        let (snapshot_tx, snapshot_rx) = watch::channel(StatsSnapshot::initial(total, instances));

        tokio::spawn(Self::aggregator(
            event_rx,
            snapshot_tx,
            total,
            instances,
            token.clone(),
        ));
        tokio::spawn(Self::reporter(snapshot_rx.clone(), token));

        Self {
            event_tx,
            _snapshot_rx: snapshot_rx,
        }
    }

    /// 仮想クライアントが状態変化を送るためのチャネルを返す
    pub fn event_tx(&self) -> mpsc::Sender<StatsEvent> {
        self.event_tx.clone()
    }

    async fn aggregator(
        event_rx: mpsc::Receiver<StatsEvent>,
        snapshot_tx: watch::Sender<StatsSnapshot>,
        total: u32,
        instances: u32,
        token: CancellationToken,
    ) {
        let mut snapshot = StatsSnapshot::initial(total, instances);
        let mut events = ReceiverStream::new(event_rx);
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                maybe_event = events.next() => {
                    let Some(event) = maybe_event else { break };
                    snapshot.apply(event);
                    let _ = snapshot_tx.send(snapshot.clone());
                }
            }
        }
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
                        "[stats] instances={} total={} connected={} retrying={} stopped={}",
                        snap.instances,
                        snap.total,
                        snap.connected,
                        snap.retrying,
                        snap.stopped,
                    );
                }
            }
        }
    }
}
