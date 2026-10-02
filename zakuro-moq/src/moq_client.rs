//! MOQT (Media over QUIC Transport) で relay へ publish する仮想クライアント
//!
//! 1 仮想クライアント = 1 QUIC 接続 = 1 MOQT セッションで、仮想クライアントごとに一意な
//! Track Name へ object を送り続ける。接続・切断・リトライのライフサイクルは
//! `crate::virtual_client` の Sora 版と同じ規則に従う。

pub(crate) mod session;
mod transport;

use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use zakuro_core::stats::StatsEvent;

pub(crate) use session::ReceiveCounters;
pub(crate) use transport::{MoqClientContext, MoqEndpoint, MoqTlsOptions};

/// subscribe するトラック名を解決する
///
/// `{instance}` / `{vc}` を仮想クライアントの値で置換する。プレースホルダが無い場合は
/// publish と同じ `<名前>-<instance>-<vc>` にする。例えば `video-{instance}-0` と書くと
/// 全仮想クライアントが同じトラック (instance 0 の仮想クライアント 0 のトラック) を購読する。
fn resolve_subscribe_track_name(name: &str, instance_id: u32, vc_id: u32) -> String {
    if name.contains("{instance}") || name.contains("{vc}") {
        name.replace("{instance}", &instance_id.to_string())
            .replace("{vc}", &vc_id.to_string())
    } else {
        format!("{name}-{instance_id}-{vc_id}")
    }
}

/// 切断後に終了処理 (FIN / GOAWAY) を待つ上限
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// 仮想クライアントの統計カウンタ
///
/// 引数を増やしすぎないよう、共有カウンタとイベント送信チャネルをまとめて渡す。
#[derive(Clone)]
pub(crate) struct VcStats {
    /// 接続状態の変化を送るチャネル
    pub(crate) events: mpsc::Sender<StatsEvent>,
    /// 送信した object 数
    pub(crate) objects_sent: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// 受信した object 数・バイト数
    pub(crate) receive: ReceiveCounters,
}

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
    /// subscribe するトラック名 (Full Track Name は仮想クライアントごとに一意化する)
    pub(crate) subscribe_tracks: Vec<String>,
    /// 受信 payload が zakuro-moq の publisher のパターンと一致するかを検査する
    pub(crate) verify_payload: bool,
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
    stats: VcStats,
) {
    let stats_tx = stats.events.clone();
    let objects_sent = stats.objects_sent.clone();
    let receive_counters = stats.receive.clone();
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
    // subscribe するトラック名を解決する。publish 側と同じ `--vcs` を指定すれば同じ番号の
    // 仮想クライアントのトラックを購読でき、`{instance}` / `{vc}` を書けば複数の購読者で
    // 同じトラックを共有できる (1 publish に対して N subscribe の負荷をかける場合)
    let subscribe_tracks: Vec<String> = config
        .subscribe_tracks
        .iter()
        .map(|name| resolve_subscribe_track_name(name, instance_id, vc_id))
        .collect();
    let session_config = session::SessionConfig {
        namespace: &config.namespace,
        tracks: &tracks,
        authority: &config.endpoint.authority,
        path: &config.endpoint.path,
        payload: std::sync::Arc::clone(&config.payload),
        subscribe_tracks: &subscribe_tracks,
        verify_payload: config.verify_payload,
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
            receive_counters.clone(),
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

    /// プレースホルダが無い場合は publish と同じ規則で suffix を付けること
    #[test]
    fn resolve_subscribe_track_name_appends_suffix() {
        assert_eq!(resolve_subscribe_track_name("video", 0, 3), "video-0-3");
        assert_eq!(resolve_subscribe_track_name("audio", 1, 0), "audio-1-0");
    }

    /// `{instance}` / `{vc}` を置換すること
    #[test]
    fn resolve_subscribe_track_name_replaces_placeholders() {
        assert_eq!(
            resolve_subscribe_track_name("video-{instance}-{vc}", 0, 3),
            "video-0-3"
        );
        // vc を固定すると全仮想クライアントが同じトラックを購読する
        assert_eq!(
            resolve_subscribe_track_name("video-{instance}-0", 0, 3),
            "video-0-0"
        );
        assert_eq!(
            resolve_subscribe_track_name("video-{instance}-0", 0, 9),
            "video-0-0"
        );
        // 片方だけの置換もできる
        assert_eq!(resolve_subscribe_track_name("video-{vc}", 2, 5), "video-5");
    }
}
