//! QMux (draft-ietf-quic-qmux-02) 経由の MOQT 接続
//!
//! QMux は順序保証された 1 本の双方向バイトストリーム (TLS over TCP) の上で、QUIC version 1 の
//! ストリーム操作と DATAGRAM 操作を多重化する。MOQT (draft-ietf-moq-transport-22) は TCP など
//! 他のトランスポート上で直接動かす方法を定義しておらず、QMux 上の MOQT は individual draft
//! (`draft-nandakumar-moq-qmux-moqt-00`) が扱う。本モジュールは QMux の接続確立と接続維持だけを
//! 担い、MOQT の状態機械は `session.rs` が駆動する。
//!
//! ALPN は native QUIC と同じ `moqt-22` を使う。QMux 上の MOQT は individual draft Section 3.2 が
//! native QUIC と同じプロトコル識別子を使うと定めている。一方 draft-ietf-quic-qmux-02 Section 8.1
//! は「QUIC マッピングと異なる識別子を MUST」とするが、MOQT 側に QMux 用の ALPN 登録が無いため
//! interop を優先する。両 draft が決着したら見直すこと。節番号・規則は draft 由来であり将来の
//! 改訂で変わる可能性がある。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use shiguredo_qmux::codec::role::Role;
use shiguredo_qmux::codec::time::Duration as QmuxDuration;
use shiguredo_qmux::config::Config;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

use crate::error::{ErrorMessage, Result};

use super::transport::ALPN;

/// QMux 接続で広告する `max_datagram_frame_size`
///
/// QMux の 1 Record の上限 (`max_record_size` の既定値) に合わせる。QMux は DATAGRAM を
/// Record に載せるため、ピアが Record に収まる DATAGRAM を受け入れられる値を広告する。
/// zakuro-moq は datagram 配送を指定したトラックの object をこの上限内で送受信する。
const MAX_DATAGRAM_FRAME_SIZE: u64 = shiguredo_qmux::record::DEFAULT_MAX_RECORD_SIZE;

/// QMux 接続で広告する `max_idle_timeout` (ミリ秒)
///
/// MOQT は Forward State 0 の間 object を送らないため、無通信でも接続を維持できる長さにする。
/// 実効値はピアの広告値との最小値になる (RFC 9000 Section 10.1) ため、ピアが短い値を
/// 広告した場合は [`spawn_keep_alive`] の PING で維持する。
const MAX_IDLE_TIMEOUT_MS: u64 = 30_000;

/// keep-alive の PING 間隔の下限
///
/// ピアが極端に短い idle timeout を広告した場合でも、PING の連投で帯域を圧迫しない範囲に
/// とどめる。実効 idle timeout の 1/3 を基本にするため、通常はこの下限より長くなる。
const MIN_KEEP_ALIVE_INTERVAL: Duration = Duration::from_millis(100);

/// keep-alive の PING 間隔の上限
///
/// 実効 idle timeout が長い場合に PING を送り過ぎないための上限である。
const MAX_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// ピアのトランスポートパラメータを待つ上限
///
/// QMux のトランスポートパラメータは接続後の最初の Record で届く。実効 idle timeout は
/// ピアの広告値との最小値なので、届く前に PING 間隔を決めるとピアの短い値を無視してしまう。
const PEER_PARAMETERS_WAIT: Duration = Duration::from_secs(5);

/// ピアのトランスポートパラメータを待つ間のポーリング間隔
const PEER_PARAMETERS_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// QMux 接続を確立する
///
/// 1 接続 = 1 TCP + TLS であり、QUIC 直接接続のようにインスタンス内でソケットを共有できない。
/// アドレスのフォールバックとタイムアウトは呼び出し側 (`transport.rs`) が行う。
///
/// # Errors
///
/// TCP 接続、TLS ハンドシェイク、ALPN の交渉、QMux 接続の生成のいずれかに失敗した場合は
/// エラーになる。
pub(crate) async fn connect(
    socket_addr: SocketAddr,
    server_name: &str,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<Arc<tokio_qmux::Connection>> {
    // QMux は 1 本のバイトストリームを多重化するため、Nagle による遅延が全ストリームに乗る。
    // 負荷試験では小さな object を高頻度に送るため無効化する
    let tcp = TcpStream::connect(socket_addr)
        .await
        .map_err(|e| ErrorMessage::new(format!("QMux の TCP 接続に失敗しました: {e}")))?;
    tcp.set_nodelay(true)
        .map_err(|e| ErrorMessage::new(format!("QMux の TCP_NODELAY 設定に失敗しました: {e}")))?;

    let server_name = rustls::pki_types::ServerName::try_from(server_name.to_string())
        .map_err(|e| ErrorMessage::new(format!("QMux の TLS サーバー名が不正です: {e}")))?;
    let tls = TlsConnector::from(tls_config)
        .connect(server_name, tcp)
        .await
        .map_err(|e| ErrorMessage::new(format!("QMux の TLS ハンドシェイクに失敗しました: {e}")))?;

    // draft-ietf-quic-qmux-02 Section 8.1: ALPN の交渉に失敗した場合はハンドシェイクを中断する
    // MUST。rustls が ALPN 未交渉でハンドシェイクを失敗させるのは QUIC のときだけであり、
    // TCP + TLS では成功してしまうため、ここで明示的に確認する
    if tls.get_ref().1.alpn_protocol() != Some(ALPN) {
        return Err(ErrorMessage::new(format!(
            "QMux のピアが MOQT の ALPN ({}) を交渉しませんでした",
            String::from_utf8_lossy(ALPN),
        ))
        .into());
    }

    let connection = tokio_qmux::Connection::new(Role::Client, tls, build_config())
        .map_err(|e| ErrorMessage::new(format!("QMux 接続の生成に失敗しました: {e}")))?;
    Ok(Arc::new(connection))
}

/// QMux 接続の設定を組み立てる
///
/// `Config::default()` はフロー制御のクレジットがすべて 0 で DATAGRAM も無効なため、ストリームと
/// DATAGRAM を使うには明示する。値は QUIC 直接接続の `transport.rs` の `build_limits` に相当する
/// 量を写す。MOQT の購読ではピアが多数の subgroup stream を開くため、QMux の既定値 (0) では
/// 足りない。
pub(crate) fn build_config() -> Config {
    Config {
        max_idle_timeout: QmuxDuration::from_millis(MAX_IDLE_TIMEOUT_MS),
        initial_max_data: 64 * 1024 * 1024,
        initial_max_stream_data_bidi_local: 16 * 1024 * 1024,
        initial_max_stream_data_bidi_remote: 16 * 1024 * 1024,
        initial_max_stream_data_uni: 16 * 1024 * 1024,
        initial_max_streams_bidi: 1_000,
        initial_max_streams_uni: 10_000,
        max_datagram_frame_size: Some(MAX_DATAGRAM_FRAME_SIZE),
        // max_record_size は既定値 (16382) のまま使う。QMux は大きな STREAM フレームを
        // Record 境界で分割するため、object サイズの上限 (MAX_OBJECT_SIZE) には影響しない
        ..Config::default()
    }
}

/// QMux 接続の keep-alive を起動する
///
/// QMux には QUIC の keep-alive に相当する API が無いため、QX_PING を定期的に送って
/// Record の送受信を発生させ、アイドルタイムアウトをリセットする
/// (draft-ietf-quic-qmux-02 Section 7.1)。ピアの idle timeout が 0 (無効) の場合は送らない。
///
/// タスクは `token` のキャンセルか、接続の終了 (PING の送信失敗) で終了する。
pub(crate) fn spawn_keep_alive(
    connection: &Arc<tokio_qmux::Connection>,
    token: &CancellationToken,
) {
    let connection = Arc::clone(connection);
    let token = token.child_token();
    tokio::spawn(async move {
        // ピアのトランスポートパラメータが届くまで待つ。届かないまま上限に達した場合は
        // ローカルの idle timeout だけで間隔を決める (接続が確立していない可能性がある)
        let deadline = tokio::time::Instant::now() + PEER_PARAMETERS_WAIT;
        while connection.peer_transport_parameters().is_none() {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::select! {
                biased;
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(PEER_PARAMETERS_POLL_INTERVAL) => {}
            }
        }

        let Some(interval) = keep_alive_interval(&connection) else {
            return;
        };
        let start = tokio::time::Instant::now() + interval;
        let mut ticker = tokio::time::interval_at(start, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = ticker.tick() => {
                    if connection.ping().await.is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// keep-alive の PING 間隔を求める (送る必要が無ければ `None`)
///
/// QMux の実効 idle timeout は、両端が広告した値のうち 0 でない値の最小値である
/// (RFC 9000 Section 10.1)。0 は無効を表す。PING の間隔は実効値の 1/3 を基本とし、
/// 下限と上限で挟む。
fn keep_alive_interval(connection: &tokio_qmux::Connection) -> Option<Duration> {
    let peer_timeout_ms = connection
        .peer_transport_parameters()
        .map(|parameters| parameters.max_idle_timeout);
    keep_alive_interval_for(MAX_IDLE_TIMEOUT_MS, peer_timeout_ms)
}

/// 実効 idle timeout から keep-alive の PING 間隔を求める
///
/// `local_timeout_ms` と `peer_timeout_ms` のうち 0 でない値の最小値を実効値とする。
/// 両方が 0 の場合は `None` (アイドルタイムアウトが無効なので送る必要が無い)。
fn keep_alive_interval_for(
    local_timeout_ms: u64,
    peer_timeout_ms: Option<u64>,
) -> Option<Duration> {
    let effective_ms = match peer_timeout_ms {
        None => local_timeout_ms,
        Some(peer) => match (local_timeout_ms, peer) {
            (0, peer) => peer,
            (local, 0) => local,
            (local, peer) => local.min(peer),
        },
    };
    if effective_ms == 0 {
        return None;
    }
    let interval_ms = (effective_ms / 3).clamp(
        MIN_KEEP_ALIVE_INTERVAL.as_millis() as u64,
        MAX_KEEP_ALIVE_INTERVAL.as_millis() as u64,
    );
    Some(Duration::from_millis(interval_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QMux の設定がストリームと DATAGRAM のクレジットを持つこと
    ///
    /// `Config::default()` のままでは全クレジットが 0 で、MOQT の購読で必要な stream を
    /// 開けない。QUIC 直接接続の limits と同等の値を設定していることを固定する。
    #[test]
    fn build_config_has_stream_and_datagram_credits() {
        let config = build_config();
        assert_eq!(config.max_idle_timeout.as_millis(), MAX_IDLE_TIMEOUT_MS);
        assert_eq!(config.initial_max_data, 64 * 1024 * 1024);
        assert_eq!(config.initial_max_stream_data_uni, 16 * 1024 * 1024);
        assert_eq!(config.initial_max_streams_bidi, 1_000);
        assert_eq!(config.initial_max_streams_uni, 10_000);
        assert_eq!(
            config.max_datagram_frame_size,
            Some(MAX_DATAGRAM_FRAME_SIZE)
        );
    }

    /// 実効 idle timeout の 1/3 を PING 間隔にすること
    #[test]
    fn keep_alive_interval_uses_one_third_of_effective_timeout() {
        assert_eq!(
            keep_alive_interval_for(30_000, None),
            Some(Duration::from_secs(10)),
            "ピアの広告が無い場合は自分の 30 秒の 1/3 にする"
        );
        assert_eq!(
            keep_alive_interval_for(30_000, Some(6_000)),
            Some(Duration::from_secs(2)),
            "ピアが短い idle timeout を広告した場合は最小値を実効値にする"
        );
        assert_eq!(
            keep_alive_interval_for(30_000, Some(0)),
            Some(Duration::from_secs(10)),
            "0 は無効を表すため、無効でない値だけを比較する"
        );
    }

    /// PING 間隔が下限と上限に収まること
    ///
    /// ピアが極端に短い値を広告しても PING を連投せず、長い値を広告しても送信を怠らない。
    #[test]
    fn keep_alive_interval_is_clamped() {
        assert_eq!(
            keep_alive_interval_for(30_000, Some(1)),
            Some(MIN_KEEP_ALIVE_INTERVAL),
            "下限より短くしない"
        );
        assert_eq!(
            keep_alive_interval_for(30_000, Some(3_600_000)),
            Some(MAX_KEEP_ALIVE_INTERVAL),
            "上限より長くしない"
        );
    }

    /// アイドルタイムアウトが無効なら PING を送らないこと
    #[test]
    fn keep_alive_interval_is_none_when_idle_timeout_is_disabled() {
        assert_eq!(keep_alive_interval_for(0, None), None);
        assert_eq!(keep_alive_interval_for(0, Some(0)), None);
    }
}
