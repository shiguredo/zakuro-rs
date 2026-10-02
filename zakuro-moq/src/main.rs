//! MOQ (Media over QUIC Transport) 負荷試験ツール zakuro-moq
//!
//! 仮想クライアントを複数起動し、MOQ relay へ QUIC 接続して複数トラックを publish する。
//! Sora (WebRTC) 版の `zakuro` とは別バイナリで、libwebrtc に依存しない。

mod args;
mod error;
mod logging;
mod moq_client;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tokio_util::time::DelayQueue;
use zakuro_core::stats::{StatsCollector, StatsEvent};

use crate::args::{CommonArgs, InstanceArgs};
use crate::error::{ErrorMessage, Result};

/// object 送信数の集計をログに出す間隔
const OBJECT_STATS_INTERVAL: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}

/// 非同期ランタイムを起動する
fn run() -> Result<()> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| ErrorMessage::new(format!("tokio ランタイムの起動に失敗しました: {e}")))?;
    rt.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let (common, instance_args, warnings) = args::parse_args()?;
    logging::init(common.log_level);
    for warning in &warnings {
        tracing::warn!("{warning}");
    }

    let total_vcs: u32 = instance_args.iter().map(|i| i.vcs).sum();
    let instances_count = instance_args.len() as u32;
    tracing::info!(
        "zakuro-moq: instances={} instance-hatch-rate={} total-vcs={}",
        instances_count,
        common.instance_hatch_rate,
        total_vcs,
    );

    let token = CancellationToken::new();

    // Ctrl+C ハンドラ: 1 回目は graceful shutdown、2 回目は強制終了
    let shutdown_token = token.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Ctrl+C received, shutting down...");
        shutdown_token.cancel();

        let _ = tokio::signal::ctrl_c().await;
        tracing::warn!("Ctrl+C received again, forcing process exit");
        std::process::exit(130);
    });

    // HTTP API (`GET /.ok` / `POST /rpc`)
    if let (Some(host), Some(port)) = (common.http_host.as_deref(), common.http_port) {
        let server = zakuro_core::http_server::HttpServer::bind(host, port, token.clone())
            .await
            .map_err(|e| ErrorMessage::new(format!("HTTP サーバーの bind に失敗しました: {e}")))?;
        let handler = zakuro_core::http_server::DefaultHandler::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        );
        tokio::spawn(async move {
            server.run(handler).await;
        });
    }

    // 接続数の集計と、object 送信数の集計
    let stats = StatsCollector::new(total_vcs, instances_count, token.clone());
    let stats_tx = stats.event_tx();
    let objects_sent = Arc::new(AtomicU64::new(0));
    spawn_object_stats(objects_sent.clone(), token.clone());

    // instance-hatch-rate 制御の DelayQueue を構築
    let hatch_start = tokio::time::Instant::now();
    let interval = Duration::from_secs_f64(1.0 / common.instance_hatch_rate);
    let mut delay: DelayQueue<usize> = DelayQueue::new();
    for i in 0..instance_args.len() {
        delay.insert(i, interval * i as u32);
    }

    let mut pending: Vec<Option<InstanceArgs>> = instance_args.into_iter().map(Some).collect();
    let mut instances: JoinSet<(usize, Result<()>)> = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            maybe_expired = delay.next() => {
                let Some(expired) = maybe_expired else { break };
                let index = expired.into_inner();
                tracing::info!(
                    "Starting zakuro-moq instance {} at +{:.2}s",
                    index,
                    hatch_start.elapsed().as_secs_f64(),
                );
                let instance = pending[index]
                    .take()
                    .expect("logical invariant: each instance is dispatched once via DelayQueue");
                let common = common.clone();
                let task_token = token.child_token();
                let stats_tx = stats_tx.clone();
                let objects_sent = objects_sent.clone();
                instances.spawn(async move {
                    let result = run_instance(index as u32, common, instance, task_token, stats_tx, objects_sent).await;
                    (index, result)
                });
            }
        }
    }

    drop(stats_tx);
    while let Some(joined) = instances.join_next().await {
        match joined {
            Ok((id, Ok(()))) => tracing::info!("zakuro-moq instance {} finished", id),
            Ok((id, Err(e))) => tracing::warn!("zakuro-moq instance {} failed: {}", id, e),
            Err(e) => tracing::warn!("zakuro-moq instance task panicked: {e}"),
        }
    }
    token.cancel();

    tracing::info!(
        "zakuro-moq: all instances finished (objects-sent={})",
        objects_sent.load(Ordering::Relaxed),
    );
    Ok(())
}

/// 1 つのインスタンスを実行する
///
/// QUIC クライアントと名前解決はインスタンスで 1 回だけ行い、仮想クライアントで共有する。
async fn run_instance(
    instance_id: u32,
    common: CommonArgs,
    instance: InstanceArgs,
    token: CancellationToken,
    stats_tx: mpsc::Sender<StatsEvent>,
    objects_sent: Arc<AtomicU64>,
) -> Result<()> {
    let endpoint = moq_client::MoqEndpoint::parse(&instance.url)?;
    let track_names: Vec<String> = instance.tracks.iter().map(|t| t.name.clone()).collect();
    tracing::info!(
        "Zakuro instance {} (MOQ): vcs={} vcs-hatch-rate={} duration={:?} repeat-interval={:?} namespace={} tracks=[{}]",
        instance_id,
        instance.vcs,
        instance.vcs_hatch_rate,
        instance.duration,
        instance.repeat_interval,
        instance.namespace,
        track_names.join(", "),
    );

    // object の payload は最大サイズで 1 つだけ作り、トラック・仮想クライアント間で共有する。
    // 送信側と受信側で内容を比較しやすいよう、位置に依存するパターンで埋める
    let max_object_size = instance
        .tracks
        .iter()
        .map(|t| t.object_size)
        .max()
        .unwrap_or(0);
    let mut payload = Vec::new();
    for i in 0..max_object_size {
        payload.push((i % 251) as u8);
    }

    let config = moq_client::MoqClientConfig {
        endpoint,
        tls: moq_client::MoqTlsOptions {
            insecure: common.insecure,
            ca_cert: instance.ca_cert.clone(),
        },
        namespace: instance.namespace.clone(),
        tracks: instance.tracks.clone(),
        payload: payload.into(),
        duration: instance.duration,
        repeat_interval: instance.repeat_interval,
        max_retry: instance.max_retry,
        retry_interval: instance.retry_interval,
    };
    let context = moq_client::build_context(&config).await?;

    // vcs-hatch-rate 制御の DelayQueue を構築
    let vc_hatch_start = tokio::time::Instant::now();
    let vc_interval = Duration::from_secs_f64(1.0 / instance.vcs_hatch_rate);
    let mut vc_delay: DelayQueue<u32> = DelayQueue::new();
    for i in 0..instance.vcs {
        vc_delay.insert(i, vc_interval * i);
    }

    let mut clients: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            maybe_expired = vc_delay.next() => {
                let Some(expired) = maybe_expired else { break };
                let vc_id = expired.into_inner();
                tracing::info!(
                    "[i{}/vc-{}] starting MOQ virtual client (+{:.2}s)",
                    instance_id,
                    vc_id,
                    vc_hatch_start.elapsed().as_secs_f64(),
                );
                clients.spawn(moq_client::run(
                    instance_id,
                    vc_id,
                    context.clone(),
                    config.clone(),
                    token.child_token(),
                    stats_tx.clone(),
                    objects_sent.clone(),
                ));
            }
        }
    }

    while let Some(result) = clients.join_next().await {
        if let Err(e) = result {
            tracing::warn!("[i{}] MOQ virtual client task panicked: {}", instance_id, e);
        }
    }
    Ok(())
}

/// object 送信数の集計を定期的にログへ出す
fn spawn_object_stats(objects_sent: Arc<AtomicU64>, token: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(OBJECT_STATS_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = 0u64;
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = interval.tick() => {
                    let current = objects_sent.load(Ordering::Relaxed);
                    let recent = (current.saturating_sub(previous)) as f64
                        / OBJECT_STATS_INTERVAL.as_secs_f64();
                    tracing::info!(
                        "[stats] objects-sent={} recent-rate={:.1}/s",
                        current,
                        recent,
                    );
                    previous = current;
                }
            }
        }
    });
}
