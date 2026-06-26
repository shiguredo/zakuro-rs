use std::time::Duration;

use shiguredo_webrtc::{rtc_log_info, rtc_log_warning};
use tokio::sync::{mpsc, watch};
use tokio::time::MissedTickBehavior;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};
use tokio_util::sync::CancellationToken;

pub(crate) enum StatsEvent {
    Connected {
        instance_id: u32,
        vc_id: u32,
    },
    Disconnected {
        instance_id: u32,
        vc_id: u32,
    },
    Retrying {
        instance_id: u32,
        vc_id: u32,
        retry_count: u32,
    },
    Stopped {
        instance_id: u32,
        vc_id: u32,
    },
}

#[derive(Clone)]
pub(crate) struct StatsSnapshot {
    pub(crate) total: u32,
    pub(crate) instances: u32,
    pub(crate) connected: u32,
    pub(crate) retrying: u32,
    pub(crate) stopped: u32,
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
                rtc_log_info!("[i{}/vc-{}][stats] connected", instance_id, vc_id);
                self.connected += 1;
                if self.retrying > 0 {
                    self.retrying -= 1;
                }
            }
            StatsEvent::Disconnected { instance_id, vc_id } => {
                rtc_log_info!("[i{}/vc-{}][stats] disconnected", instance_id, vc_id);
                if self.connected > 0 {
                    self.connected -= 1;
                }
            }
            StatsEvent::Retrying {
                instance_id,
                vc_id,
                retry_count,
            } => {
                rtc_log_warning!(
                    "[i{}/vc-{}][stats] retrying ({})",
                    instance_id,
                    vc_id,
                    retry_count,
                );
                self.retrying += 1;
            }
            StatsEvent::Stopped { instance_id, vc_id } => {
                rtc_log_info!("[i{}/vc-{}][stats] stopped", instance_id, vc_id);
                self.stopped += 1;
                if self.retrying > 0 {
                    self.retrying -= 1;
                }
            }
        }
    }
}

pub(crate) struct StatsCollector {
    event_tx: mpsc::Sender<StatsEvent>,
    _snapshot_rx: watch::Receiver<StatsSnapshot>,
}

impl StatsCollector {
    pub(crate) fn new(total: u32, instances: u32, token: CancellationToken) -> Self {
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

    pub(crate) fn event_tx(&self) -> mpsc::Sender<StatsEvent> {
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
                    rtc_log_info!(
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
