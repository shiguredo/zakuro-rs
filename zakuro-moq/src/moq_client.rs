//! MOQT (Media over QUIC Transport) で relay へ publish する仮想クライアント
//!
//! 1 仮想クライアント = 1 QUIC 接続 = 1 MOQT セッションで、仮想クライアントごとに一意な
//! Track Name へ object を送り続ける。接続・切断・リトライのライフサイクルは
//! `crate::virtual_client` の Sora 版と同じ規則に従う。

mod session;
mod transport;

use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use zakuro_core::stats::StatsEvent;

pub(crate) use transport::{MoqClientContext, MoqEndpoint, MoqTlsOptions};

/// 切断後に終了処理 (FIN / GOAWAY) を待つ上限
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// MOQ 仮想クライアントの設定
#[derive(Clone)]
pub(crate) struct MoqClientConfig {
    /// 接続先 relay の URL を解釈した結果
    pub(crate) endpoint: MoqEndpoint,
    /// TLS 設定
    pub(crate) tls: MoqTlsOptions,
    /// Track Namespace
    pub(crate) namespace: String,
    /// publish するトラック (Track Name は仮想クライアントごとに一意化する)
    pub(crate) tracks: Vec<crate::args::TrackSpec>,
    /// object の payload (最大サイズで 1 度だけ作り、トラック・仮想クライアント間で共有する)
    pub(crate) payload: std::sync::Arc<[u8]>,
    /// 接続維持秒数 (未指定なら無制限)
    pub(crate) duration: Option<f64>,
    /// duration 経過後の再接続間隔 (秒)
    pub(crate) repeat_interval: Option<f64>,
    /// 接続失敗時の最大リトライ回数
    pub(crate) max_retry: u32,
    /// リトライ間隔 (秒)
    pub(crate) retry_interval: f64,
}

/// 仮想クライアント 1 本を実行する
///
/// 接続・切断・リトライを繰り返し、`token` がキャンセルされるか、`--duration` と
/// `--repeat-interval` の指定で終了条件を満たすまで戻らない。
pub(crate) async fn run(
    instance_id: u32,
    vc_id: u32,
    context: MoqClientContext,
    config: MoqClientConfig,
    token: CancellationToken,
    stats_tx: mpsc::Sender<StatsEvent>,
    objects_sent: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    // Track Name は仮想クライアントごとに一意にする。relay は同一 Track への複数 publisher を
    // 区別できないため、負荷試験では「どの仮想クライアントの object か」を分ける必要がある
    let tracks: Vec<session::TrackConfig> = config
        .tracks
        .iter()
        .map(|track| session::TrackConfig {
            name: format!("{}-{}-{}", track.name, instance_id, vc_id),
            object_rate: track.object_rate,
            object_size: track.object_size,
        })
        .collect();
    let track_names: Vec<String> = tracks.iter().map(|t| t.name.clone()).collect();
    let session_config = session::SessionConfig {
        namespace: &config.namespace,
        tracks: &tracks,
        authority: &config.endpoint.authority,
        path: &config.endpoint.path,
        payload: std::sync::Arc::clone(&config.payload),
    };

    let mut retry_count: u32 = 0;

    loop {
        let connection_token = token.child_token();

        let connection = tokio::select! {
            biased;
            _ = token.cancelled() => break,
            result = context.connect() => match result {
                Ok(connection) => connection,
                Err(e) => {
                    tracing::warn!("[i{}/vc-{}] MOQ connect failed: {}", instance_id, vc_id, e);
                    retry_count += 1;
                    if retry_count > config.max_retry {
                        tracing::info!(
                            "[i{}/vc-{}] reached max retry count ({})",
                            instance_id,
                            vc_id,
                            config.max_retry,
                        );
                        break;
                    }
                    let _ = stats_tx
                        .send(StatsEvent::Retrying {
                            instance_id,
                            vc_id,
                            retry_count,
                        })
                        .await;
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs_f64(config.retry_interval)) => continue,
                    }
                }
            },
        };

        // MOQT セッションを開始し、SETUP の交換が完了するまで待つ。
        // 「接続済み」は QUIC のハンドシェイク完了ではなく MOQT セッションの確立とする
        // (確立していない接続は publish できないため、負荷試験の意味では未接続である)
        let (established_tx, established_rx) = tokio::sync::oneshot::channel();
        let mut session_future = Box::pin(session::run(
            connection,
            &session_config,
            &connection_token,
            Some(established_tx),
            objects_sent.clone(),
        ));
        // session future が完了したかを追跡する。async fn の future は完了後に再度 poll
        // すると panic するため、終了処理では未完了のときだけ待つ
        let mut session_finished = false;
        let established = tokio::select! {
            biased;
            _ = token.cancelled() => false,
            result = established_rx => result.is_ok(),
            result = &mut session_future => {
                session_finished = true;
                match result {
                    Ok(()) => tracing::info!("[i{}/vc-{}] MOQ session finished", instance_id, vc_id),
                    Err(e) => tracing::warn!("[i{}/vc-{}] MOQ session failed: {}", instance_id, vc_id, e),
                }
                false
            }
        };

        let mut disconnected_by_duration = false;
        if established {
            let _ = stats_tx
                .send(StatsEvent::Connected { instance_id, vc_id })
                .await;
            tracing::info!(
                "[i{}/vc-{}] MOQ session established: tracks={}",
                instance_id,
                vc_id,
                track_names.join(","),
            );
            disconnected_by_duration = tokio::select! {
                biased;
                _ = token.cancelled() => false,
                _ = duration_timer(config.duration) => true,
                result = &mut session_future => {
                    session_finished = true;
                    match result {
                        Ok(()) => tracing::info!("[i{}/vc-{}] MOQ session finished", instance_id, vc_id),
                        Err(e) => tracing::warn!("[i{}/vc-{}] MOQ session failed: {}", instance_id, vc_id, e),
                    }
                    false
                }
            };
        }

        // session のループを止め、終了処理 (FIN / GOAWAY) を待つ。既に完了している future を
        // 再度 poll してはならない
        connection_token.cancel();
        wait_for_session_shutdown(&mut session_future, session_finished).await;
        drop(session_future);

        if established {
            let _ = stats_tx
                .send(StatsEvent::Disconnected { instance_id, vc_id })
                .await;
        }

        if token.is_cancelled() {
            tracing::info!("[i{}/vc-{}] shutting down", instance_id, vc_id);
            break;
        }

        if disconnected_by_duration {
            tracing::info!("[i{}/vc-{}] duration expired", instance_id, vc_id);
            retry_count = 0;
            match config.repeat_interval {
                Some(interval) if interval > 0.0 => {
                    tracing::info!(
                        "[i{}/vc-{}] reconnecting in {:.1}s",
                        instance_id,
                        vc_id,
                        interval,
                    );
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs_f64(interval)) => continue,
                    }
                }
                _ => break,
            }
        }

        // 想定外の切断はリトライする
        retry_count += 1;
        if retry_count > config.max_retry {
            tracing::info!(
                "[i{}/vc-{}] reached max retry count ({})",
                instance_id,
                vc_id,
                config.max_retry,
            );
            break;
        }
        let _ = stats_tx
            .send(StatsEvent::Retrying {
                instance_id,
                vc_id,
                retry_count,
            })
            .await;
        tracing::info!(
            "[i{}/vc-{}] retrying in {:.1}s ({}/{})",
            instance_id,
            vc_id,
            config.retry_interval,
            retry_count,
            config.max_retry,
        );
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs_f64(config.retry_interval)) => continue,
        }
    }

    let _ = stats_tx
        .send(StatsEvent::Stopped { instance_id, vc_id })
        .await;
}

/// `--duration` 経過を待つ future
///
/// 未指定または 0 以下の場合は完了しない (`pending`)。
async fn duration_timer(duration: Option<f64>) {
    match duration {
        Some(d) if d > 0.0 => tokio::time::sleep(Duration::from_secs_f64(d)).await,
        _ => std::future::pending().await,
    }
}

/// MOQ relay へ接続するためのコンテキストを構築する
///
/// 名前解決と s2n-quic クライアントの構築はインスタンスで 1 回だけ行い、仮想クライアントで
/// 共有する。
///
/// # Errors
///
/// 名前解決、TLS 設定、ソケットの作成に失敗した場合はエラーになる。
pub(crate) async fn build_context(config: &MoqClientConfig) -> Result<MoqClientContext> {
    if config.tls.insecure {
        tracing::warn!("MOQ TLS certificate verification is disabled (--insecure)");
    }
    MoqClientContext::new(&config.endpoint, &config.tls).await
}

/// セッション future の終了処理を待つ
///
/// `finished` が true のとき、その future は既に完了しているため poll しない。`async fn` の
/// future を完了後に poll すると panic するため、この判定は必須である。
async fn wait_for_session_shutdown<F>(session_future: &mut F, finished: bool)
where
    F: std::future::Future<Output = Result<()>> + Unpin,
{
    if finished {
        return;
    }
    let _ = tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, session_future).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 完了済みの future を再 poll しないこと
    ///
    /// `finished` を無視して poll すると、async fn の future は
    /// `` `async fn` resumed after completion `` で panic する。
    #[tokio::test]
    async fn wait_for_session_shutdown_skips_finished_future() {
        let mut future = Box::pin(async { Ok(()) });
        let _ = (&mut future).await;
        wait_for_session_shutdown(&mut future, true).await;
    }
}
