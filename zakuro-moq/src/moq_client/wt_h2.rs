//! WebTransport over HTTP/2 (TCP + TLS + shiguredo_http2) の接続とストリーム
//!
//! draft-ietf-webtrans-http2-15 のクライアント実装であり、TCP + TLS (ALPN `h2`) の上で
//! shiguredo_http2 の Sans I/O `Connection` と `webtransport::WtSession` を駆動して MOQT を運ぶ。
//!
//! WebTransport over HTTP/2 は HTTP/2 の 1 本の Extended CONNECT ストリーム上に Capsule
//! Protocol でストリーム (WT_STREAM) と DATAGRAM を多重化する。セッションの状態は CONNECT
//! ストリーム 1 本に閉じるため、このモジュールの駆動タスクが `Connection` と `WtSession` を
//! 所有し、公開する [`Handle`] / [`SendStream`] / [`RecvStream`] / acceptor はチャネルで
//! 操作を依頼する。この構成は ngtcp2 経路 (`ngtcp2.rs`) と揃えている。
//!
//! TLS は呼び出し側が `h2` を ALPN に設定した [`rustls::ClientConfig`] を渡す。MOQT の
//! プロトコル識別子 (`moqt-22`) は TLS の ALPN ではなく、Extended CONNECT の
//! WT-Available-Protocols / WT-Protocol ヘッダーで交渉する
//! (draft-ietf-moq-transport-22 §6.2 (Session establishment))。
//!
//! zakuro-moq は object を subgroup stream で送り、datagram 配送を指定したトラックは
//! `WT_DATAGRAM` capsule で送る。受信した datagram はアプリへ渡す
//! (draft-ietf-webtrans-http2-15 §6.13)。
//!
//! 節番号・要求項目は IETF draft 由来であり、draft の改訂や RFC 化で変わりうる。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Duration;

use shiguredo_http2::settings::DEFAULT_INITIAL_WINDOW_SIZE;
use shiguredo_http2::webtransport::{
    WtAvailableProtocols, WtConfig, WtError, WtErrorKind, WtEvent, WtSession, WtStreamId,
};
use shiguredo_http2::{ErrorCode, Event, HeaderField, Limits, StreamId};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Interval, MissedTickBehavior};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

use super::StreamReset;
use super::transport::ALPN;
use crate::error::{ErrorMessage, Result};

/// HTTP/2 の ALPN プロトコル識別子 (RFC 9113 §3.3)
const H2_ALPN: &[u8] = b"h2";

/// Extended CONNECT の `:protocol` 値 (draft-ietf-webtrans-http2-15 §3.1)
const WEBTRANSPORT_PROTOCOL: &[u8] = b"webtransport";

/// WT 出力を `Connection::send_data` に 1 回で渡せる上限
///
/// shiguredo_http2 のストリーム送信バッファの固定容量 (`DEFAULT_INITIAL_WINDOW_SIZE`) と
/// 同じ値である。1 回の `send_data` にそれ以上を渡すと全量拒否されるため、WT の capsule
/// は capsule 境界と無関係に分割して送る (draft-ietf-webtrans-http2-15 §2 / RFC 9297 §3.1)。
const WT_SEND_CHUNK_SIZE: usize = DEFAULT_INITIAL_WINDOW_SIZE as usize;

/// 1 回の write コマンドで送るバイト数
///
/// ngtcp2 経路と同じ値にする。WT の送信ウィンドウはセッション単位であり、1 本の
/// ストリームへ巨大なデータを一度に積むと他のストリームの送信がウィンドウ解放まで
/// 待たされるため、チャンクに区切って他のストリームと交互に送る。
const WRITE_CHUNK_SIZE: usize = 16 * 1024;

/// 受信バッファのサイズ
///
/// shiguredo_http2 の既定の最大フレームサイズ (16384) 以上にして、1 フレームを
/// 複数回の read に分割しすぎないようにする。
const READ_BUFFER_SIZE: usize = 16 * 1024;

/// 接続終了時にピアへ終了を伝える猶予
///
/// ハンドルがすべて破棄された場合と接続失敗時に、WT_CLOSE_SESSION と END_STREAM を
/// 送る試みをこの時間で打ち切る。ピアが読まない TCP で flush が固まるのを避ける。
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// keep-alive の HTTP/2 PING 送信間隔
///
/// ngtcp2 経路の `keep_alive_timeout=5s` と揃える。MOQT は Forward State 0 の間
/// object を送らないため、無通信でも接続を維持できる必要がある。
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// WT のセッションレベル初期受信ウィンドウ (bytes)
///
/// QUIC 直接接続 (`transport.rs` の `build_limits`) と同じ値にし、経路の違いで
/// フロー制御の性質が変わらないようにする。
const WT_INITIAL_MAX_DATA: u64 = 64 * 1024 * 1024;

/// WT のストリームレベル初期受信ウィンドウ (bytes)
const WT_INITIAL_MAX_STREAM_DATA: u64 = 16 * 1024 * 1024;

/// WT のピア起動単方向ストリーム数の初期上限
const WT_INITIAL_MAX_STREAMS_UNI: u64 = 10_000;

/// WT のピア起動双方向ストリーム数の初期上限
const WT_INITIAL_MAX_STREAMS_BIDI: u64 = 1_000;

/// HTTP/2 の接続レベル受信ウィンドウ (bytes)
///
/// WT のデータはすべて 1 本の CONNECT ストリームに載るため、HTTP/2 の接続ウィンドウが
/// そのままスループットの上限になる。既定値 (65535) のままだと WINDOW_UPDATE の往復で
/// 帯域が頭打ちになるため広げる。WT のセッションウィンドウ以下にして、HTTP/2 層で
/// 未読データを抱え込みすぎないようにする。
const H2_CONNECTION_WINDOW_SIZE: u32 = 16 * 1024 * 1024;

/// HTTP/2 のストリームレベル受信ウィンドウ (bytes)
///
/// CONNECT ストリームは 1 本しか使わないが、SETTINGS_INITIAL_WINDOW_SIZE はストリーム
/// 単位の受信上限になるため接続ウィンドウと揃える。
const H2_INITIAL_WINDOW_SIZE: u32 = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 接続
// ---------------------------------------------------------------------------

/// TLS (ALPN `h2`) と HTTP/2 で接続し、WebTransport セッションを確立して駆動タスクを起動する
///
/// 1. TCP 接続と TLS ハンドシェイク (ALPN `h2` は `tls` が広告済み)
/// 2. HTTP/2 の接続プリフェイスと SETTINGS (WT の初期フロー制御値を広告する)
/// 3. ピア SETTINGS の待機 (`SETTINGS_ENABLE_CONNECT_PROTOCOL` と `SETTINGS_WT_ENABLED`
///    の確認。draft-ietf-webtrans-http2-15 §3.1)
/// 4. Extended CONNECT (`:protocol=webtransport`) の送信と 2xx 応答の待機 (§3.2)
/// 5. 応答の `WT-Protocol` が `moqt-22` であることの確認と駆動タスクの起動 (§3.3)
///
/// タイムアウトは呼び出し側 (`transport.rs`) が設定する。ピアが SETTINGS を送らない間に
/// 永久に待つことはない。
///
/// # Errors
///
/// - TCP / TLS / HTTP/2 の初期化に失敗した場合
/// - ピアが WebTransport over HTTP/2 に対応しない SETTINGS を送った場合 (§3.1)
/// - CONNECT が 2xx 以外で拒否された場合 (§3.2)
/// - 2xx 応答に `WT-Protocol` が無い、または `moqt-22` でない場合 (§3.3)
pub(crate) async fn connect(
    remote: SocketAddr,
    server_name: &str,
    authority: &str,
    path: &str,
    tls: &Arc<rustls::ClientConfig>,
) -> Result<Connection> {
    connect_inner(
        remote,
        server_name,
        authority,
        path,
        tls,
        &build_wt_config(),
    )
    .await
}

/// テスト用: WebTransport の初期フロー制御値を差し替えて接続する
///
/// 本番の [`connect`] は [`build_wt_config`] の値を使う。セッション受信ウィンドウの
/// クレジット返却は初期ウィンドウが小さいほど短時間で観測できるため、テストから
/// 小さい広告値で同じ駆動タスクを動かせるよう、この入口だけを追加している。
///
/// # Errors
///
/// [`connect`] と同じ。
#[cfg(test)]
pub(crate) async fn connect_with_wt_config(
    remote: SocketAddr,
    server_name: &str,
    authority: &str,
    path: &str,
    tls: &Arc<rustls::ClientConfig>,
    wt_config: &WtConfig,
) -> Result<Connection> {
    connect_inner(remote, server_name, authority, path, tls, wt_config).await
}

/// 接続を確立する共通処理
///
/// # Errors
///
/// TCP / TLS / HTTP/2 / WebTransport のいずれかの初期化に失敗した場合はエラーになる。
async fn connect_inner(
    remote: SocketAddr,
    server_name: &str,
    authority: &str,
    path: &str,
    tls: &Arc<rustls::ClientConfig>,
    wt_config: &WtConfig,
) -> Result<Connection> {
    let tcp = TcpStream::connect(remote).await.map_err(|e| {
        ErrorMessage::new(format!(
            "WebTransport over HTTP/2 の TCP 接続に失敗しました ({remote}): {e}"
        ))
    })?;
    // QMux 経路と同じく Nagle を無効化する。負荷試験では小さな object を高頻度に送るため、
    // 1 接続に集約される WT over HTTP/2 では TCP の遅延がそのまま転送遅延になる
    tcp.set_nodelay(true).map_err(|e| {
        ErrorMessage::new(format!(
            "WebTransport over HTTP/2 の TCP_NODELAY 設定に失敗しました ({remote}): {e}"
        ))
    })?;

    let server_name =
        rustls::pki_types::ServerName::try_from(server_name.to_string()).map_err(|e| {
            ErrorMessage::new(format!(
                "WebTransport over HTTP/2 の TLS サーバー名が不正です: {e}"
            ))
        })?;
    let stream = TlsConnector::from(Arc::clone(tls))
        .connect(server_name, tcp)
        .await
        .map_err(|e| {
            ErrorMessage::new(format!(
                "WebTransport over HTTP/2 の TLS ハンドシェイクに失敗しました ({remote}): {e}"
            ))
        })?;

    // RFC 9113 §3.3: HTTP/2 は ALPN `h2` で識別する。rustls は TCP + TLS では ALPN 未交渉
    // でもハンドシェイクを成功させるため、ここで明示的に確認する
    if stream.get_ref().1.alpn_protocol() != Some(H2_ALPN) {
        return Err(
            ErrorMessage::new("MOQ relay が HTTP/2 の ALPN (h2) を選択しませんでした").into(),
        );
    }

    let (io, connect_stream, wt_session) = establish(stream, authority, path, wt_config)
        .await
        .map_err(|e| {
            ErrorMessage::new(format!(
                "WebTransport over HTTP/2 のセッション確立に失敗しました ({remote}): {e}"
            ))
        })?;

    tracing::info!("Connected to {} over TLS (HTTP/2) as {}", remote, authority);
    Ok(Connection::start(io, connect_stream, wt_session))
}

/// HTTP/2 を初期化して WebTransport セッションを確立する
///
/// 戻り値は駆動タスクへ引き渡す HTTP/2 の I/O、CONNECT ストリーム、WT セッションである。
async fn establish(
    stream: TlsStream,
    authority: &str,
    path: &str,
    wt_config: &WtConfig,
) -> io::Result<(H2Io<TlsStream>, StreamId, WtSession)> {
    // WebTransport の初期フロー制御値は SETTINGS で広告する (§4.3.1)。呼び出し側が渡した
    // 設定と広告値が一致するよう同じ値から `Limits` を組み立てる
    let wt_config = wt_config.clone();
    let limits = build_limits(&wt_config)?;
    let mut io = H2Io::client(stream, limits);
    // RFC 9113 §3.4: クライアントの接続プリフェイスと初期 SETTINGS を送る
    io.initiate().await?;

    // draft-ietf-webtrans-http2-15 §3.1: ピアが WebTransport に対応するまで CONNECT を
    // 送ってはならない
    wait_for_peer_settings(&mut io).await?;

    // draft-ietf-webtrans-http2-15 §3.2: Extended CONNECT で target URI を指定する
    let headers = build_connect_headers(authority, path)?;
    let connect_stream = io.conn.start_stream(headers, false).map_err(io_error)?;
    io.flush().await?;
    wait_for_connect_response(&mut io, connect_stream).await?;
    tracing::debug!(
        "WebTransport session established: connect_stream={}",
        connect_stream.as_u32()
    );

    // セッション確立時に適用する初期値は、ACK 済みの自広告 SETTINGS とピアの SETTINGS で
    // ある (§4.3.1)。未広告項目の Initial Value は 0 のため、ピア設定は `peer_default`
    // から始める (§11.2)
    let mut local_config = wt_config;
    local_config.overlay_settings(io.conn.local_settings());
    let mut peer_config = WtConfig::peer_default();
    peer_config.overlay_settings(io.conn.remote_settings());
    let mut wt_session = WtSession::client(local_config, peer_config);
    wt_session.initiate().map_err(io_error)?;

    Ok((io, connect_stream, wt_session))
}

/// WebTransport の初期フロー制御値を広告するローカル設定を作る
///
/// QUIC 直接接続のトランスポートパラメータと同じ値にし、経路の違いでフロー制御の
/// 性質が変わらないようにする。
fn build_wt_config() -> WtConfig {
    WtConfig {
        initial_max_data: WT_INITIAL_MAX_DATA,
        initial_max_stream_data_bidi_local: WT_INITIAL_MAX_STREAM_DATA,
        initial_max_stream_data_bidi_remote: WT_INITIAL_MAX_STREAM_DATA,
        initial_max_stream_data_uni: WT_INITIAL_MAX_STREAM_DATA,
        initial_max_streams_bidi: WT_INITIAL_MAX_STREAMS_BIDI,
        initial_max_streams_uni: WT_INITIAL_MAX_STREAMS_UNI,
    }
}

/// WebTransport の初期フロー制御値を含む HTTP/2 の `Limits` を構築する
///
/// draft-ietf-webtrans-http2-15 §4.3.1: 初期フロー制御値は SETTINGS で広告する。
/// §3.1: WebTransport を使うには `SETTINGS_ENABLE_CONNECT_PROTOCOL` と
/// `SETTINGS_WT_ENABLED` の両方が必要である。
/// 初期値を SETTINGS で広告するため、任意の `WebTransport-Init` ヘッダーは送らない
/// (§4.3.2 のヘッダーは SETTINGS と併用する場合に大きい方が採用される)。
fn build_limits(wt_config: &WtConfig) -> io::Result<Limits> {
    let setting = |value: u64| -> io::Result<u32> {
        u32::try_from(value).map_err(|_| {
            io::Error::other(format!(
                "WebTransport flow control limit {value} does not fit in a SETTINGS value (u32)"
            ))
        })
    };
    Limits::builder()
        .initial_window_size(
            shiguredo_http2::WindowSize::new(H2_INITIAL_WINDOW_SIZE).map_err(io_error)?,
        )
        .connection_window_size(
            shiguredo_http2::WindowSize::new(H2_CONNECTION_WINDOW_SIZE).map_err(io_error)?,
        )
        .enable_connect_protocol(true)
        .wt_enabled(true)
        .webtransport(
            Some(setting(wt_config.initial_max_data)?),
            Some(setting(wt_config.initial_max_stream_data_uni)?),
            Some(setting(wt_config.initial_max_stream_data_bidi_local)?),
            Some(setting(wt_config.initial_max_streams_uni)?),
            Some(setting(wt_config.initial_max_streams_bidi)?),
            Some(setting(wt_config.initial_max_stream_data_bidi_remote)?),
        )
        .build()
        .map_err(io_error)
}

/// WebTransport CONNECT のリクエストヘッダーを構築する
///
/// draft-ietf-webtrans-http2-15 §3.1 / §3.2: `:method=CONNECT`、`:protocol=webtransport`、
/// `:scheme=https`、`:authority`、`:path` を設定する。
/// draft-ietf-moq-transport-22 §6.2: MOQT は `WT-Available-Protocols` でプロトコル
/// 識別子を通知する。
fn build_connect_headers(authority: &str, path: &str) -> io::Result<Vec<HeaderField>> {
    let header = |name: &'static [u8], value: &[u8]| -> io::Result<HeaderField> {
        HeaderField::new(name, value).map_err(|e| {
            io::Error::other(format!(
                "failed to build WebTransport CONNECT header {}: {e}",
                String::from_utf8_lossy(name)
            ))
        })
    };
    let mut headers = vec![
        HeaderField::from_static(b":method", b"CONNECT"),
        HeaderField::from_static(b":protocol", WEBTRANSPORT_PROTOCOL),
        HeaderField::from_static(b":scheme", b"https"),
        header(b":authority", authority.as_bytes())?,
        header(b":path", path.as_bytes())?,
    ];
    // RFC 8941 §4.1.6 の sf-string を 1 要素の List として送る
    let available = shiguredo_http2::webtransport::serialize_wt_protocol(ALPN)
        .map_err(|e| io::Error::other(format!("failed to build WT-Available-Protocols: {e}")))?;
    headers.push(header(b"wt-available-protocols", &available)?);
    Ok(headers)
}

/// ピアの SETTINGS を受信し、WebTransport に対応していることを確認する
///
/// draft-ietf-webtrans-http2-15 §3.1: クライアントはサーバーが
/// `SETTINGS_ENABLE_CONNECT_PROTOCOL=1` と `SETTINGS_WT_ENABLED=1` を送るまで
/// WebTransport の CONNECT を送ってはならない (MUST NOT)。最初の SETTINGS で両方が
/// 無い場合はこの接続では WebTransport を使えないため、待ち続けずに失敗させる。
async fn wait_for_peer_settings<S>(io: &mut H2Io<S>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        if peer_supports_webtransport(io) {
            return Ok(());
        }
        match io.next_event().await? {
            Event::SettingsReceived { ack: false } => {
                if !peer_supports_webtransport(io) {
                    return Err(io::Error::other(format!(
                        "MOQ relay does not support WebTransport over HTTP/2 \
                         (enable_connect_protocol={}, wt_enabled={})",
                        io.conn.remote_settings().enable_connect_protocol(),
                        io.conn.remote_settings().wt_enabled()
                    )));
                }
            }
            Event::ConnectionError { error_code, reason } => {
                return Err(io::Error::other(format!(
                    "HTTP/2 connection error before peer SETTINGS: {error_code}: {reason}"
                )));
            }
            // 接続開始前に届くその他のイベントは読み飛ばす
            _ => {}
        }
    }
}

/// ピアが WebTransport over HTTP/2 を使える SETTINGS を広告済みかを返す
fn peer_supports_webtransport<S>(io: &H2Io<S>) -> bool {
    io.conn.remote_settings().enable_connect_protocol() && io.conn.remote_settings().wt_enabled()
}

/// CONNECT の 2xx 応答を待ち、`WT-Protocol` を検証する
///
/// draft-ietf-webtrans-http2-15 §3.2: 2xx でセッションが確立する。
/// §3.3: クライアントが `WT-Available-Protocols` を送った場合、サーバーが
/// `WT-Protocol` を含めるなら候補の中から 1 つを選ばなければならない (MUST)。
/// MOQT はプロトコル識別子の交渉に使うため、2xx に `WT-Protocol` が無い場合も失敗させる。
async fn wait_for_connect_response<S>(io: &mut H2Io<S>, connect_stream: StreamId) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match io.next_event().await? {
            Event::HeadersReceived {
                stream_id,
                headers,
                end_stream,
                ..
            } if stream_id == connect_stream => {
                let status = headers
                    .iter()
                    .find(|h| h.name() == b":status")
                    .and_then(|h| std::str::from_utf8(h.value()).ok())
                    .and_then(|value| value.parse::<u16>().ok());
                let Some(status) = status else {
                    return Err(io::Error::other(
                        "WebTransport CONNECT response has no valid :status",
                    ));
                };
                if !(200..300).contains(&status) {
                    return Err(io::Error::other(format!(
                        "WebTransport CONNECT was rejected: status={status}"
                    )));
                }
                if end_stream {
                    // 2xx + END_STREAM はセッション確立直後の終了であり、使えるセッションに
                    // ならない
                    return Err(io::Error::other(
                        "WebTransport CONNECT response ended the stream",
                    ));
                }
                verify_wt_protocol(&headers)?;
                return Ok(());
            }
            Event::StreamReset {
                stream_id,
                error_code,
                ..
            } if stream_id == connect_stream => {
                return Err(io::Error::other(format!(
                    "WebTransport CONNECT stream was reset before the response: {error_code}"
                )));
            }
            Event::StreamClosed { stream_id } if stream_id == connect_stream => {
                return Err(io::Error::other(
                    "WebTransport CONNECT stream was closed before the response",
                ));
            }
            Event::ConnectionError { error_code, reason } => {
                return Err(io::Error::other(format!(
                    "HTTP/2 connection error while waiting for the WebTransport CONNECT response: \
                     {error_code}: {reason}"
                )));
            }
            _ => {}
        }
    }
}

/// 応答の `WT-Protocol` が送った候補に一致するかを検証する
///
/// draft-ietf-webtrans-http2-15 §3.3: 2xx 応答の `WT-Protocol` は
/// `WT-Available-Protocols` に含めた値でなければならない (MUST)。String 以外の型は
/// 無視され、パラメータは読み飛ばされる (同節)。パラメータの読み飛ばしは
/// [`WtAvailableProtocols::parse`] の List パーサーに任せ、要素数を 1 に限定して
/// Item of String として扱う。
fn verify_wt_protocol(headers: &[HeaderField]) -> io::Result<()> {
    let negotiation_failed = || {
        io::Error::other(
            "WebTransport protocol negotiation failed: the response does not select moqt-22",
        )
    };
    let Some(value) = headers
        .iter()
        .find(|h| h.name() == b"wt-protocol")
        .map(HeaderField::value)
    else {
        return Err(negotiation_failed());
    };
    let parsed = WtAvailableProtocols::parse(value).map_err(|_| negotiation_failed())?;
    if parsed.protocols.len() != 1 || parsed.protocols[0].as_bytes() != ALPN {
        return Err(negotiation_failed());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP/2 の非同期 I/O ラッパー
// ---------------------------------------------------------------------------

/// クライアントの TLS ストリーム型
type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

/// shiguredo_http2 の Sans I/O `Connection` を TCP + TLS ストリーム上で駆動する
///
/// `poll_output` の書き出し (`flush`) と受信 (`read_event`) を提供する。ストリーム型を
/// 汎用にしているのは、結合テストでサーバー側の `tokio_rustls::server::TlsStream` も
/// 同じ手順で駆動するためである。
struct H2Io<S> {
    /// TCP + TLS のストリーム
    stream: S,
    /// Sans I/O の HTTP/2 接続
    conn: shiguredo_http2::Connection,
    /// 受信バッファ
    recv_buf: Vec<u8>,
    /// `poll_output` から取り出してまだストリームへ書き出していないバイト列
    ///
    /// `write` のキャンセル (select! による中断) で未書き出しのデータを失わないため、
    /// 接続の出力バッファとは別に保持する。
    out_buf: Vec<u8>,
    /// `out_buf` のうち書き出し済みの先頭バイト数
    ///
    /// 書き出しのたびに `Vec` の先頭を詰めると O(n²) になるため、カーソルで進めて
    /// バッファが空になったときと未書き出し分が半分未満になったときにまとめて詰める。
    out_pos: usize,
}

impl<S> H2Io<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// クライアントの HTTP/2 接続を作る
    fn client(stream: S, limits: Limits) -> Self {
        Self {
            stream,
            conn: shiguredo_http2::Connection::client(limits),
            recv_buf: vec![0u8; READ_BUFFER_SIZE],
            out_buf: Vec::new(),
            out_pos: 0,
        }
    }

    /// サーバーの HTTP/2 接続を作る (結合テスト用)
    #[cfg(test)]
    fn server(stream: S, limits: Limits) -> Self {
        Self {
            stream,
            conn: shiguredo_http2::Connection::server(limits),
            recv_buf: vec![0u8; READ_BUFFER_SIZE],
            out_buf: Vec::new(),
            out_pos: 0,
        }
    }

    /// 接続プリフェイス (クライアント) と初期 SETTINGS を送信する (RFC 9113 §3.4)
    async fn initiate(&mut self) -> io::Result<()> {
        self.conn.initiate().map_err(io_error)?;
        self.flush().await
    }

    /// 接続の出力をすべてストリームへ書き出す
    ///
    /// 書き出しの途中で select! に中断されても未書き出し分は `out_buf` に残るため、
    /// 次回の呼び出しで続きから送れる。
    async fn flush(&mut self) -> io::Result<()> {
        while let Some(output) = self.conn.poll_output() {
            self.push_output(&output);
        }
        while self.out_pos < self.out_buf.len() {
            // `AsyncWriteExt::write` はキャンセル安全であり、中断時は 0 バイトも
            // 書き出していないことが保証される
            let size = self.stream.write(&self.out_buf[self.out_pos..]).await?;
            if size == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "HTTP/2 stream write returned zero",
                ));
            }
            self.out_pos += size;
        }
        // すべて書き出したら空にして、次回以降のインデックスを先頭に戻す
        self.out_buf.clear();
        self.out_pos = 0;
        self.stream.flush().await
    }

    /// 送信バッファへバイト列を積む
    ///
    /// 書き出し済みの先頭部分は、バッファが空になったときと未書き出し分が半分未満に
    /// なったときにまとめて詰める。書き出しのたびに `Vec::drain` で先頭を詰めると
    /// バッファ長に対して O(n²) になるため、カーソル (`out_pos`) で進める。
    fn push_output(&mut self, data: &[u8]) {
        if self.out_pos == self.out_buf.len() {
            self.out_buf.clear();
            self.out_pos = 0;
        } else if self.out_pos > 0 && self.out_pos * 2 >= self.out_buf.len() {
            self.out_buf.drain(..self.out_pos);
            self.out_pos = 0;
        }
        self.out_buf.extend_from_slice(data);
    }

    /// イベントが得られるまで読み進め、1 件返す
    ///
    /// `AsyncReadExt::read` はキャンセル安全であるため、select! で中断されても
    /// 受信データを失わない。
    async fn read_event(&mut self) -> io::Result<Event> {
        loop {
            if let Some(event) = self.conn.poll_event() {
                return Ok(event);
            }
            let size = self.stream.read(&mut self.recv_buf).await?;
            if size == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the peer closed the HTTP/2 connection",
                ));
            }
            self.conn.feed(&self.recv_buf[..size]).map_err(io_error)?;
            self.conn.process().map_err(io_error)?;
        }
    }

    /// 出力を流してからイベントを 1 件返す (接続確立時の逐次処理用)
    async fn next_event(&mut self) -> io::Result<Event> {
        self.flush().await?;
        self.read_event().await
    }
}

// ---------------------------------------------------------------------------
// 接続ハンドルとストリーム
// ---------------------------------------------------------------------------

/// 駆動タスクへの操作要求
enum Command {
    /// ストリームを開く
    Open(OpenRequest),
    /// ストリームへデータ (または FIN) を送る
    Send {
        stream_id: WtStreamId,
        data: Vec<u8>,
        fin: bool,
        reply: oneshot::Sender<io::Result<()>>,
    },
    /// OBJECT_DATAGRAM を送る
    SendDatagram {
        data: Vec<u8>,
        reply: oneshot::Sender<io::Result<()>>,
    },
    /// ストリームの送信方向を reset する
    Reset {
        stream_id: WtStreamId,
        error_code: u64,
        reply: oneshot::Sender<io::Result<()>>,
    },
    /// ストリームの送信方向を破棄した (Drop)
    DropSend {
        stream_id: WtStreamId,
        finished: bool,
    },
    /// アプリが受信データを消費した
    Consumed { stream_id: WtStreamId, size: u64 },
    /// ストリームの受信方向を破棄した (Drop)
    ///
    /// `unread` はアプリが読まなかったバイト数であり、接続全体の受信クレジットとして
    /// 返す。`Consumed` と分けているのは、`WT_STOP_SENDING` を送った後にストリーム単位の
    /// `WT_MAX_STREAM_DATA` を送ってはならないためである
    /// (draft-ietf-webtrans-http2-15 §6.6 の MUST NOT)。
    StopRead { stream_id: WtStreamId, unread: u64 },
    /// 接続の keep-alive (HTTP/2 PING) を有効にする
    EnableKeepAlive {
        /// PING を送る間隔
        interval: Duration,
    },
    /// セッションを閉じる
    Close {
        code: u64,
        reply: oneshot::Sender<()>,
    },
}

/// ストリームを開く要求
///
/// ピアが広告したストリーム数の上限に達している場合は、空きができるまで駆動タスクが
/// 保持して再試行する。
struct OpenRequest {
    /// 双方向ストリームを開くか
    bidirectional: bool,
    /// 開いたストリーム (単方向の場合は受信方向が `None`) またはエラー
    reply: oneshot::Sender<io::Result<(Option<RecvStream>, SendStream)>>,
}

/// 送信待ちのデータ
struct PendingSend {
    /// 未送信のデータ
    data: Vec<u8>,
    /// このデータを送り終えたら FIN を送るか
    fin: bool,
    /// 送信完了 (WT の capsule への変換完了) を知らせる reply
    reply: oneshot::Sender<io::Result<()>>,
}

/// 終端済みのストリームへの操作に対するエラー
#[derive(Clone, Copy)]
enum SendFailure {
    /// ピアの WT_STOP_SENDING で終端した (MOQT のエラーコード付き)
    Reset(u64),
    /// FIN / reset 済みなどの終端
    Closed,
}

impl SendFailure {
    /// `io::Error` を作る
    ///
    /// reset の場合は [`StreamReset`] を保持し、`session.rs` が downcast して MOQT の
    /// ストリーム reset として扱えるようにする。
    fn error(self) -> io::Error {
        match self {
            Self::Reset(error_code) => {
                io::Error::new(io::ErrorKind::BrokenPipe, StreamReset(error_code))
            }
            Self::Closed => io::Error::new(
                io::ErrorKind::BrokenPipe,
                "WebTransport stream send direction is closed",
            ),
        }
    }
}

/// 受信ストリームへ流すメッセージ
enum Received {
    /// データ (FIN 付きの場合がある)
    Data { data: Vec<u8>, fin: bool },
    /// ピアからの reset
    Reset { error_code: u64 },
}

/// ストリームごとの受信状態
struct RecvState {
    /// アプリへデータを渡すチャネル
    tx: mpsc::UnboundedSender<Received>,
    /// このストリームに広告した初期受信ウィンドウ
    initial: u64,
    /// 受信したバイト数
    received: u64,
    /// アプリが消費したバイト数
    consumed: u64,
    /// これまでに追加広告した受信ウィンドウの合計
    grown: u64,
}

/// ピア起動ストリームの終端状態
///
/// ピアが新しくストリームを開けるよう、受信方向と送信方向の両方が閉じた時点で
/// `WT_MAX_STREAMS` を 1 増やす (draft-ietf-webtrans-http2-15 §6.7)。
struct PeerStream {
    /// 双方向ストリームか
    bidirectional: bool,
    /// 受信方向が終端したか (FIN または reset)
    recv_closed: bool,
    /// 送信方向が終端したか (FIN / reset / Drop。単方向の受信専用ストリームは最初から真)
    send_closed: bool,
}

/// 最後のハンドルが破棄されたら駆動タスクも終了させる
struct Owner {
    /// 駆動タスクへの操作チャネル
    commands: mpsc::UnboundedSender<Command>,
    /// ハンドル全破棄の通知
    cancel: CancellationToken,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// 接続操作のハンドル
///
/// 複数タスクから clone して使う。最後のハンドルが破棄されると駆動タスクが接続を
/// 閉じて終了する。
#[derive(Clone)]
pub(crate) struct Handle(Arc<Owner>);

impl Handle {
    /// 単方向の送信ストリームを開く
    ///
    /// # Errors
    ///
    /// 接続が終了している場合、またはストリームを開けない場合はエラーになる。
    pub(crate) async fn open_send_stream(&self) -> io::Result<SendStream> {
        let (_, send) = self.open(false).await?;
        Ok(send)
    }

    /// 双方向ストリームを開き、受信方向と送信方向を返す
    ///
    /// # Errors
    ///
    /// 接続が終了している場合、またはストリームを開けない場合はエラーになる。
    pub(crate) async fn open_bidirectional_stream(&self) -> io::Result<(RecvStream, SendStream)> {
        let (recv, send) = self.open(true).await?;
        Ok((
            recv.expect("logical invariant: bidirectional stream has a receive half"),
            send,
        ))
    }

    /// ストリームを開き、駆動タスクの応答を待つ
    async fn open(&self, bidirectional: bool) -> io::Result<(Option<RecvStream>, SendStream)> {
        let (reply, result) = oneshot::channel();
        self.0
            .commands
            .send(Command::Open(OpenRequest {
                bidirectional,
                reply,
            }))
            .map_err(|_| connection_closed())?;
        result.await.map_err(|_| connection_closed())?
    }

    /// OBJECT_DATAGRAM を 1 つ送る
    ///
    /// WebTransport over HTTP/2 の DATAGRAM は capsule で運ばれ、HTTP/2 の送信ウィンドウに
    /// 応じて駆動タスクが書き出す。この future は capsule を HTTP/2 の出力へ流し終えた
    /// 時点で完了する (TCP まで書き終えたことは保証しない)。
    ///
    /// # Errors
    ///
    /// 接続が終了している場合、セッションが datagram を送れない状態の場合はエラーになる。
    pub(crate) async fn send_datagram(&self, data: &[u8]) -> io::Result<()> {
        let (reply, result) = oneshot::channel();
        self.0
            .commands
            .send(Command::SendDatagram {
                data: data.to_vec(),
                reply,
            })
            .map_err(|_| connection_closed())?;
        result.await.map_err(|_| connection_closed())?
    }

    /// 接続をアイドルタイムアウトで切られないよう、HTTP/2 PING を定期的に送る
    ///
    /// MOQT は Forward State 0 の間 object を送らないため、無通信でも接続を維持する
    /// 必要がある。PING の送信は駆動タスクが行うため、この呼び出しは待たない。
    pub(crate) fn enable_keep_alive(&self) {
        let _ = self.0.commands.send(Command::EnableKeepAlive {
            interval: KEEP_ALIVE_INTERVAL,
        });
    }

    /// テスト用: PING の送信間隔を差し替えて keep-alive を有効にする
    ///
    /// 本番の間隔 (5 秒) のままではテストが遅くなるため、同じ経路を短い間隔で
    /// 検証できるようにする。
    #[cfg(test)]
    fn enable_keep_alive_with_interval(&self, interval: Duration) {
        let _ = self.0.commands.send(Command::EnableKeepAlive { interval });
    }

    /// MOQT のエラーコードで接続を閉じる
    ///
    /// `draft-ietf-moq-transport-22` §12.2 のセッション終了コードを渡す。
    /// WT_CLOSE_SESSION の Application Error Code が 32 ビットのため、収まらない
    /// ローカル専用コードは NO_ERROR として送る。
    pub(crate) async fn close(&self, error_code: u64) {
        let (reply, result) = oneshot::channel();
        if self
            .0
            .commands
            .send(Command::Close {
                code: error_code,
                reply,
            })
            .is_ok()
        {
            let _ = result.await;
        }
    }
}

/// 送信ストリーム
pub(crate) struct SendStream {
    /// WT のストリーム ID
    stream_id: WtStreamId,
    /// 駆動タスクのハンドル
    handle: Handle,
    /// 送信方向を閉じたか
    finished: bool,
}

impl SendStream {
    /// WebTransport のストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        self.stream_id
    }

    /// すべてのバイト列を書き込む
    ///
    /// WT の送信ウィンドウが枯渇している間は待つ。ピアの STOP_SENDING で終端した
    /// ストリームではエラーの中に [`StreamReset`] が入る。
    ///
    /// # Errors
    ///
    /// 接続が終了している場合、ストリームが終端している場合、または書き込みに
    /// 失敗した場合はエラーになる。
    pub(crate) async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        for chunk in buf.chunks(WRITE_CHUNK_SIZE) {
            self.write(chunk, false).await?;
        }
        Ok(())
    }

    /// データ (または FIN) を送り、駆動タスクが WT の capsule へ変換するまで待つ
    async fn write(&mut self, data: &[u8], fin: bool) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "WebTransport stream send direction is already finished",
            ));
        }
        let (reply, result) = oneshot::channel();
        self.handle
            .0
            .commands
            .send(Command::Send {
                stream_id: self.stream_id,
                data: data.to_vec(),
                fin,
                reply,
            })
            .map_err(|_| connection_closed())?;
        result.await.map_err(|_| connection_closed())?
    }

    /// ストリームの送信方向を FIN で閉じる
    ///
    /// # Errors
    ///
    /// 接続が終了している場合、またはストリームが終端している場合はエラーになる。
    pub(crate) async fn finish(&mut self) -> io::Result<()> {
        self.write(&[], true).await?;
        self.finished = true;
        Ok(())
    }

    /// ストリームの送信方向を reset する
    ///
    /// `error_code` は MOQT のストリームリセットエラーコード
    /// (draft-ietf-moq-transport-22 §12.5) であり、WT_RESET_STREAM の
    /// Application Error Code としてそのまま送る (draft-ietf-webtrans-http2-15 §6.2)。
    ///
    /// # Errors
    ///
    /// 接続が終了している場合、またはストリームが終端している場合はエラーになる。
    pub(crate) async fn reset(&mut self, error_code: u64) -> io::Result<()> {
        let (reply, result) = oneshot::channel();
        self.handle
            .0
            .commands
            .send(Command::Reset {
                stream_id: self.stream_id,
                error_code,
                reply,
            })
            .map_err(|_| connection_closed())?;
        result.await.map_err(|_| connection_closed())??;
        self.finished = true;
        Ok(())
    }
}

impl Drop for SendStream {
    fn drop(&mut self) {
        // FIN を送らずに破棄された送信ストリームは、駆動タスクが reset して終端する。
        // ここで待たない (Drop は非同期にできない) ため、後始末は駆動タスクに任せる
        let _ = self.handle.0.commands.send(Command::DropSend {
            stream_id: self.stream_id,
            finished: self.finished,
        });
    }
}

/// 受信ストリーム
///
/// 読み込みはキャンセルされても未読データを失わない。ピアの reset は
/// [`StreamReset`] を内包した [`io::Error`] で返す。
pub(crate) struct RecvStream {
    /// WT のストリーム ID
    stream_id: WtStreamId,
    /// 駆動タスクのハンドル
    handle: Handle,
    /// 駆動タスクからの受信メッセージ
    incoming: mpsc::UnboundedReceiver<Received>,
    /// アプリへまだ返していない受信データ
    buffer: Vec<u8>,
    /// `buffer` の中で次に返す位置
    offset: usize,
    /// データに付いていた FIN をまだアプリへ返していない
    pending_fin: bool,
    /// 観測した終端 (`None` は FIN、`Some` は reset のエラーコード)
    terminal: Option<Option<u64>>,
}

impl RecvStream {
    /// WebTransport のストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        self.stream_id
    }

    /// 読み込んだバイト数を返す (0 は FIN)
    ///
    /// ピアが reset したストリームでは [`StreamReset`] を内包したエラーを返す。
    ///
    /// # Errors
    ///
    /// 接続が終了している場合、またはストリームが reset された場合はエラーになる。
    pub(crate) async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.offset < self.buffer.len() {
                let size = buf.len().min(self.buffer.len() - self.offset);
                buf[..size].copy_from_slice(&self.buffer[self.offset..self.offset + size]);
                self.offset += size;
                // await を挟まずに消費通知を出し、select! によるキャンセルで欠落させない
                let _ = self.handle.0.commands.send(Command::Consumed {
                    stream_id: self.stream_id,
                    size: size as u64,
                });
                return Ok(size);
            }
            self.buffer.clear();
            self.offset = 0;
            if self.pending_fin {
                self.pending_fin = false;
                self.terminal = Some(None);
            }
            if let Some(terminal) = self.terminal {
                return match terminal {
                    None => Ok(0),
                    Some(error_code) => Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        StreamReset(error_code),
                    )),
                };
            }
            match self.incoming.recv().await {
                Some(Received::Data { data, fin }) => {
                    self.buffer = data;
                    self.offset = 0;
                    self.pending_fin = fin;
                }
                Some(Received::Reset { error_code }) => {
                    self.terminal = Some(Some(error_code));
                }
                // 駆動タスクが終了した = 接続が閉じた
                None => return Err(connection_closed()),
            }
        }
    }
}

impl Drop for RecvStream {
    fn drop(&mut self) {
        // 未読データを破棄する。消費しなかった分の接続全体の受信枠は駆動タスクが返す。
        // `Command::Consumed` を別に送らないのは、その直後に送る WT_STOP_SENDING の後で
        // ストリーム単位の WT_MAX_STREAM_DATA を広告しないためである (§6.6 の MUST NOT)。
        self.incoming.close();
        let mut unread = (self.buffer.len() - self.offset) as u64;
        while let Ok(message) = self.incoming.try_recv() {
            if let Received::Data { data, .. } = message {
                unread += data.len() as u64;
            }
        }
        // 受信方向を放棄したことをピアへ伝える (WT_STOP_SENDING)。エラーコードに 0 を
        // 使う根拠は Command::StopRead のコメントを参照する
        let _ = self.handle.0.commands.send(Command::StopRead {
            stream_id: self.stream_id,
            unread,
        });
    }
}

/// ピア起動の単方向ストリームを受け入れる口
pub(crate) struct RecvAcceptor {
    /// 駆動タスクから届く受信ストリーム
    incoming: mpsc::UnboundedReceiver<RecvStream>,
    /// 駆動タスクを生かしておくためのハンドル
    _handle: Handle,
}

impl RecvAcceptor {
    /// 単方向ストリームを 1 本受け入れる
    ///
    /// 接続が終了した場合は `Ok(None)` を返す。
    ///
    /// # Errors
    ///
    /// accept に失敗した場合はエラーになる。
    pub(crate) async fn accept(&mut self) -> io::Result<Option<RecvStream>> {
        Ok(self.incoming.recv().await)
    }
}

/// ピア起動の双方向ストリームを受け入れる口
pub(crate) struct BidiAcceptor {
    /// 駆動タスクから届く `(受信方向, 送信方向)` の組
    incoming: mpsc::UnboundedReceiver<(RecvStream, SendStream)>,
    /// 駆動タスクを生かしておくためのハンドル
    _handle: Handle,
}

impl BidiAcceptor {
    /// 双方向ストリームを 1 本受け入れる
    ///
    /// 接続が終了した場合は `Ok(None)` を返す。
    ///
    /// # Errors
    ///
    /// accept に失敗した場合はエラーになる。
    pub(crate) async fn accept(&mut self) -> io::Result<Option<(RecvStream, SendStream)>> {
        Ok(self.incoming.recv().await)
    }
}

/// 確立済みの WebTransport over HTTP/2 接続
pub(crate) struct Connection {
    /// 接続操作のハンドル
    handle: Handle,
    /// ピア起動の単方向ストリームの受け入れ口
    recv: RecvAcceptor,
    /// ピア起動の双方向ストリームの受け入れ口
    bidi: BidiAcceptor,
    /// 駆動タスクが転送する受信 datagram
    datagram: mpsc::UnboundedReceiver<Vec<u8>>,
}

impl Connection {
    /// 駆動タスクを起動し、分割可能な接続を組み立てる
    fn start(io: H2Io<TlsStream>, connect_stream: StreamId, wt_session: WtSession) -> Self {
        let (commands, requests) = mpsc::unbounded_channel();
        let (uni_tx, uni_rx) = mpsc::unbounded_channel();
        let (bidi_tx, bidi_rx) = mpsc::unbounded_channel();
        let (datagram_tx, datagram_rx) = mpsc::unbounded_channel();
        let owner = Arc::new(Owner {
            commands,
            cancel: CancellationToken::new(),
        });
        let handle = Handle(Arc::clone(&owner));
        let driver = Driver {
            io,
            connect_stream,
            wt: wt_session,
            requests,
            owner: Arc::downgrade(&owner),
            cancel: owner.cancel.clone(),
            uni_tx,
            bidi_tx,
            datagram_tx,
            recv_streams: HashMap::new(),
            peer_streams: HashMap::new(),
            pending_sends: HashMap::new(),
            opened: VecDeque::new(),
            stopped: HashMap::new(),
            stop_sent: HashSet::new(),
            wt_out_pending: Vec::new(),
            after_flush: Vec::new(),
            session_received: 0,
            session_consumed: 0,
            session_grown: 0,
            draining: false,
            keep_alive: None,
        };
        tokio::spawn(async move {
            driver.run().await;
        });
        Self {
            recv: RecvAcceptor {
                incoming: uni_rx,
                _handle: handle.clone(),
            },
            bidi: BidiAcceptor {
                incoming: bidi_rx,
                _handle: handle.clone(),
            },
            datagram: datagram_rx,
            handle,
        }
    }

    /// 接続ハンドル、単方向 acceptor、双方向 acceptor、datagram の受け口へ分割する
    pub(crate) fn split(
        self,
    ) -> (
        Handle,
        RecvAcceptor,
        BidiAcceptor,
        mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        (self.handle, self.recv, self.bidi, self.datagram)
    }
}

// ---------------------------------------------------------------------------
// 駆動タスク
// ---------------------------------------------------------------------------

/// 接続を単独で所有する駆動タスクの状態
///
/// HTTP/2 のイベントとアプリからのコマンドを単一のタスクで直列に処理する。これにより
/// `WtSession` の状態をロックなしで安全に扱える。
struct Driver {
    /// HTTP/2 + TLS + TCP の I/O
    io: H2Io<TlsStream>,
    /// WebTransport の CONNECT ストリーム
    connect_stream: StreamId,
    /// Sans I/O の WT セッション
    wt: WtSession,
    /// アプリからの操作要求
    requests: mpsc::UnboundedReceiver<Command>,
    /// ハンドルの Weak 参照 (新しいストリームのハンドルを作るため)
    owner: Weak<Owner>,
    /// ハンドル全破棄の通知
    cancel: CancellationToken,
    /// ピア起動の単方向受信ストリームをアプリへ渡すチャネル
    uni_tx: mpsc::UnboundedSender<RecvStream>,
    /// ピア起動の双方向ストリームをアプリへ渡すチャネル
    bidi_tx: mpsc::UnboundedSender<(RecvStream, SendStream)>,
    /// 受信した datagram をアプリへ渡すチャネル
    datagram_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// 受信中のストリームの状態
    recv_streams: HashMap<WtStreamId, RecvState>,
    /// ピア起動ストリームの終端状態 (WT_MAX_STREAMS を返すため)
    peer_streams: HashMap<WtStreamId, PeerStream>,
    /// 送信待ちのデータ (ストリームごと、FIFO)
    pending_sends: HashMap<WtStreamId, VecDeque<PendingSend>>,
    /// ストリーム数の空きを待つ open 要求
    opened: VecDeque<OpenRequest>,
    /// ピアから WT_STOP_SENDING を受けたストリーム (ストリーム ID → エラーコード)
    ///
    /// エントリは削除せず、上限も設けない。WebTransport のストリーム ID は QUIC の
    /// 識別子を写したもので再利用されないため (draft-ietf-webtrans-http2-15 §5.2 が
    /// RFC 9000 §2.1 の識別子を写すと定める) 誤判定は起きない。削除すると、遅延した
    /// 送信が `StreamClosed` に化けてアプリが該当ストリームの終端と判別できなくなる。
    /// したがって増加量はピアが cancel したストリーム数に比例する。
    stopped: HashMap<WtStreamId, u64>,
    /// WT_STOP_SENDING を送ったストリーム (以降の受信データを破棄する)
    stop_sent: HashSet<WtStreamId>,
    /// `poll_output` から取り出してまだ HTTP/2 へ流していない WT 出力
    ///
    /// HTTP/2 の送信ウィンドウが枯渇している間はここに溜まる。上限は WT の送信
    /// ウィンドウ (セッションとストリーム) と、アプリが完了を待っている書き込み量で
    /// 決まり、ピアが受信を進めない限り無制限には増えない。
    wt_out_pending: Vec<u8>,
    /// フラッシュ成功後に `Ok` を返す reply (reset の完了通知)
    after_flush: Vec<oneshot::Sender<io::Result<()>>>,
    /// セッション全体で受信したバイト数
    session_received: u64,
    /// アプリが消費したバイト数 (セッション全体)
    session_consumed: u64,
    /// これまでに追加広告した受信ウィンドウの合計 (セッション全体)
    session_grown: u64,
    /// ピアの drain / 終了を観測したか
    draining: bool,
    /// keep-alive の PING 送信タイマー (`Command::EnableKeepAlive` で有効になる)
    keep_alive: Option<Interval>,
}

impl Driver {
    /// 駆動タスクのメインループ
    ///
    /// 終了時は必ず接続を閉じるか、閉じたことをチャネルの閉鎖でアプリへ伝える。
    async fn run(mut self) {
        let cancel = self.cancel.clone();
        loop {
            // ストリーム数の空き待ちを再試行してから、送信待ちと WT 出力を捌く
            self.pump_opens();
            if !self.flush_cycle().await {
                break;
            }
            // biased は付けない。コマンドが絶えず届く状況で HTTP/2 の受信が飢えると、
            // WINDOW_UPDATE を読めずに送信が止まったままになるため、tokio の公平な
            // 選択に任せる
            tokio::select! {
                request = self.requests.recv() => {
                    match request {
                        Some(request) => {
                            if !self.handle_command(request).await {
                                break;
                            }
                        }
                        // すべての Handle が破棄された
                        None => {
                            self.close_gracefully(0).await;
                            break;
                        }
                    }
                }
                _ = cancel.cancelled() => {
                    self.close_gracefully(0).await;
                    break;
                }
                _ = next_keep_alive_tick(&mut self.keep_alive), if self.keep_alive.is_some() => {
                    if !self.send_keep_alive_ping() {
                        break;
                    }
                }
                event = self.io.read_event() => {
                    match event {
                        Ok(event) => {
                            if !self.handle_event(event).await {
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::debug!("HTTP/2 connection ended: {error}");
                            break;
                        }
                    }
                }
            }
        }
        // 未処理のコマンドと送信待ちはここで破棄される。reply の送信側が drop されると
        // 呼び出し側は接続終了としてエラーを受け取る
    }

    /// 送信待ち・WT 出力・reply の順に捌き、TCP まで書き出す
    ///
    /// 戻り値は接続を継続できるかどうかである。書き出しに失敗した場合は、待っている
    /// reply をすべてエラーにして false を返す。
    async fn flush_cycle(&mut self) -> bool {
        let completed = self.flush_pending_sends();
        let result = match self.pump_wt_output() {
            Ok(()) => self.io.flush().await,
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => {
                for (reply, result) in completed {
                    let _ = reply.send(result);
                }
                for reply in self.after_flush.drain(..) {
                    let _ = reply.send(Ok(()));
                }
                true
            }
            Err(error) => {
                let message = error.to_string();
                for (reply, _) in completed {
                    let _ = reply.send(Err(io_error(&message)));
                }
                self.fail_all_requests(&message);
                false
            }
        }
    }

    /// ストリームを 1 本開き、両方向のハンドルを返す
    ///
    /// # Errors
    ///
    /// ピアが広告したストリーム数の上限に達している場合、またはセッションが
    /// 終了している場合は [`WtError`] を返す。呼び出し側は種類を見て再試行を決める。
    fn open_streams(
        &mut self,
        bidirectional: bool,
        handle: Handle,
    ) -> std::result::Result<(Option<RecvStream>, SendStream), WtError> {
        let stream_id = if bidirectional {
            self.wt.open_bidi_stream()?
        } else {
            self.wt.open_uni_stream()?
        };
        let send = SendStream {
            stream_id,
            handle: handle.clone(),
            finished: false,
        };
        let recv = if bidirectional {
            // ローカル起動の双方向ストリームの受信上限は BIDI_LOCAL
            let initial = self.wt.config().initial_max_stream_data_bidi_local;
            Some(self.recv_stream(stream_id, initial, handle))
        } else {
            None
        };
        Ok((recv, send))
    }

    /// 受信ストリームのハンドルと受信状態を作る
    fn recv_stream(&mut self, stream_id: WtStreamId, initial: u64, handle: Handle) -> RecvStream {
        let (tx, incoming) = mpsc::unbounded_channel();
        self.recv_streams.insert(
            stream_id,
            RecvState {
                tx,
                initial,
                received: 0,
                consumed: 0,
                grown: 0,
            },
        );
        RecvStream {
            stream_id,
            handle,
            incoming,
            buffer: Vec::new(),
            offset: 0,
            pending_fin: false,
            terminal: None,
        }
    }

    /// ストリーム数の空きを待つ open 要求を再試行する
    ///
    /// ピアが WT_MAX_STREAMS を送っても `WtEvent` は発火しないため、駆動タスクの
    /// ループごとに再試行する。上限に達している場合は先頭の要求を残して打ち切る。
    fn pump_opens(&mut self) {
        let mut attempts = self.opened.len();
        while attempts > 0 {
            attempts -= 1;
            let Some(request) = self.opened.pop_front() else {
                return;
            };
            // 待っている呼び出し側が既に drop された要求は捨てる
            if request.reply.is_closed() {
                continue;
            }
            if self.draining || !self.wt.is_active() {
                let _ = request.reply.send(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "WebTransport session is closing",
                )));
                continue;
            }
            let Some(handle) = self.owner.upgrade().map(Handle) else {
                let _ = request.reply.send(Err(connection_closed()));
                continue;
            };
            match self.open_streams(request.bidirectional, handle) {
                Ok(result) => {
                    let _ = request.reply.send(Ok(result));
                }
                Err(error) if error.kind() == WtErrorKind::FlowControlError => {
                    // WT_MAX_STREAMS の空き待ち。他の要求も同じ制限に当たるため、
                    // 先頭へ戻して打ち切る
                    self.opened.push_back(request);
                    return;
                }
                Err(error) => {
                    let _ = request.reply.send(Err(io_error(error)));
                }
            }
        }
    }

    /// コマンドを処理し、ループを継続するかどうかを返す
    async fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::Open(request) => {
                self.opened.push_back(request);
                true
            }
            Command::Send {
                stream_id,
                data,
                fin,
                reply,
            } => {
                if !self.can_send(stream_id) {
                    let _ = reply.send(Err(self.closed_stream_error(stream_id).error()));
                    return true;
                }
                self.pending_sends
                    .entry(stream_id)
                    .or_default()
                    .push_back(PendingSend { data, fin, reply });
                true
            }
            Command::SendDatagram { data, reply } => {
                // DATAGRAM capsule を WT セッションの出力へ積み、次の flush_cycle で
                // HTTP/2 へ流す。reply は flush 後に返す
                match self.wt.send_datagram(&data) {
                    Ok(()) => self.after_flush.push(reply),
                    Err(error) => {
                        let _ = reply.send(Err(io_error(error)));
                    }
                }
                true
            }
            Command::Reset {
                stream_id,
                error_code,
                reply,
            } => {
                if !self.can_send(stream_id) {
                    let _ = reply.send(Err(self.closed_stream_error(stream_id).error()));
                    return true;
                }
                // 未送信のデータは app が reset した時点で不要である
                self.fail_pending_sends(stream_id, SendFailure::Closed);
                match self.wt.reset_stream(stream_id, error_code) {
                    Ok(()) => {
                        self.after_flush.push(reply);
                        self.mark_peer_send_closed(stream_id);
                    }
                    Err(error) => {
                        let _ = reply.send(Err(io_error(error)));
                    }
                }
                true
            }
            Command::DropSend {
                stream_id,
                finished,
            } => {
                self.fail_pending_sends(stream_id, SendFailure::Closed);
                if !finished {
                    // FIN を送らずに破棄されたストリームは reset で終端する。ピアは
                    // データの途切れを検出でき、セッションは継続する
                    if let Err(error) = self.wt.reset_stream(stream_id, 0) {
                        tracing::debug!(
                            "Failed to reset the dropped WebTransport stream {stream_id}: {error}"
                        );
                    }
                }
                self.mark_peer_send_closed(stream_id);
                true
            }
            Command::Consumed { stream_id, size } => {
                self.session_consumed = self.session_consumed.saturating_add(size);
                if let Some(state) = self.recv_streams.get_mut(&stream_id) {
                    state.consumed = state.consumed.saturating_add(size);
                }
                self.grow_session_recv_window();
                self.grow_stream_recv_window(stream_id);
                true
            }
            Command::StopRead { stream_id, unread } => {
                // 受信方向を放棄した。以降の受信データは破棄し、ピアへ WT_STOP_SENDING を
                // 送る。未読分はアプリが読まなかったデータとして扱い、接続全体の受信
                // クレジットだけ返す。ストリーム単位のウィンドウは STOP_SENDING を送った
                // 後に広告してはならない (§6.6 の MUST NOT) ため拡張しない
                self.recv_streams.remove(&stream_id);
                if unread > 0 {
                    self.session_consumed = self.session_consumed.saturating_add(unread);
                    self.grow_session_recv_window();
                }
                // トランスポート層は MOQT のストリームエラーコードの意味を知らないため、
                // 既定の 0 を送る (MOQT §12.5 では 0 は INTERNAL_ERROR)。QUIC 経路の
                // StopRead も同じ値を使う
                if self.wt.stop_sending(stream_id, 0).is_ok() {
                    self.stop_sent.insert(stream_id);
                }
                self.mark_peer_recv_closed(stream_id);
                true
            }
            Command::EnableKeepAlive { interval } => {
                // 最初の PING は接続直後ではなく interval の経過後に送る
                let mut interval = time::interval_at(time::Instant::now() + interval, interval);
                interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
                self.keep_alive = Some(interval);
                true
            }
            Command::Close { code, reply } => {
                if let Err(error) = self.close_session(code).await {
                    // 終了処理は best-effort である。送信ウィンドウの枯渇などで
                    // END_STREAM を送れない場合もあり、接続はこの後破棄される
                    tracing::debug!(
                        "WebTransport over HTTP/2 close was not completed cleanly: {error}"
                    );
                }
                let _ = reply.send(());
                false
            }
        }
    }

    /// H2 のイベントを処理し、ループを継続するかどうかを返す
    async fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::DataReceived {
                stream_id,
                data,
                end_stream,
            } if stream_id == self.connect_stream => {
                if let Err(error) = self.wt.feed(&data).and_then(|_| self.wt.process()) {
                    return self.fail_session(error).await;
                }
                while let Some(event) = self.wt.poll_event() {
                    if !self.dispatch_wt_event(event).await {
                        return false;
                    }
                }
                // RFC 9113 §6.9.1: 受信したデータの分だけウィンドウを戻し、単一の
                // CONNECT ストリーム上でピアが送り続けられるようにする
                let size = u32::try_from(data.len())
                    .expect("logical invariant: HTTP/2 DATA payload fits in u32");
                if size > 0 {
                    if let Err(error) = self.io.conn.send_window_update(StreamId::Connection, size)
                    {
                        tracing::warn!(
                            "Failed to send an HTTP/2 connection WINDOW_UPDATE: {error}"
                        );
                        return false;
                    }
                    if !end_stream
                        && let Err(error) =
                            self.io.conn.send_window_update(self.connect_stream, size)
                    {
                        tracing::warn!("Failed to send an HTTP/2 stream WINDOW_UPDATE: {error}");
                        return false;
                    }
                }
                if end_stream {
                    // CONNECT ストリームの終了はセッション終了である (§3.4)
                    tracing::info!("WebTransport session closed by peer: CONNECT stream ended");
                    return false;
                }
                true
            }
            Event::DataDiscarded {
                stream_id,
                connection_window_consumed,
            } if stream_id == self.connect_stream && connection_window_consumed > 0 => {
                // RFC 9113 §6.9.1: 破棄した DATA の分も接続ウィンドウを補充する
                let size = u32::try_from(connection_window_consumed)
                    .expect("logical invariant: discarded HTTP/2 DATA fits in u32");
                if let Err(error) = self.io.conn.send_window_update(StreamId::Connection, size) {
                    tracing::warn!("Failed to replenish the HTTP/2 connection window: {error}");
                    return false;
                }
                true
            }
            Event::StreamReset {
                stream_id,
                error_code,
                connection_window_consumed,
            } if stream_id == self.connect_stream => {
                // draft-ietf-webtrans-http2-15 §3.4: CONNECT ストリームの reset は
                // アプリケーション層の通知を伴わないセッション終了である
                tracing::warn!(
                    "WebTransport session terminated by a CONNECT stream reset: {error_code}"
                );
                if connection_window_consumed > 0
                    && let Ok(size) = u32::try_from(connection_window_consumed)
                {
                    let _ = self.io.conn.send_window_update(StreamId::Connection, size);
                }
                false
            }
            Event::StreamClosed { stream_id } if stream_id == self.connect_stream => {
                tracing::info!("WebTransport session closed: CONNECT stream closed");
                false
            }
            Event::ConnectionError { error_code, reason } => {
                tracing::warn!("HTTP/2 connection error: {error_code}: {reason}");
                false
            }
            Event::GoawayReceived { error_code, .. } => {
                // draft-ietf-webtrans-http2-15 §6.13: GOAWAY は新しいセッションを作れなく
                // するが、既存セッションは継続できる
                tracing::info!("Received HTTP/2 GOAWAY: {error_code}");
                true
            }
            other => {
                tracing::trace!("Ignoring HTTP/2 event: {other:?}");
                true
            }
        }
    }

    /// WT イベントを処理し、ループを継続するかどうかを返す
    async fn dispatch_wt_event(&mut self, event: WtEvent) -> bool {
        match event {
            WtEvent::StreamOpened {
                stream_id,
                bidirectional,
            } => {
                let Some(handle) = self.owner.upgrade().map(Handle) else {
                    return false;
                };
                // 終端状態を記録する。単方向の受信専用ストリームは最初から送信方向が無い
                self.peer_streams.insert(
                    stream_id,
                    PeerStream {
                        bidirectional,
                        recv_closed: false,
                        send_closed: !bidirectional,
                    },
                );
                if bidirectional {
                    // ピア起動の双方向ストリームの受信上限は BIDI_REMOTE
                    let initial = self.wt.config().initial_max_stream_data_bidi_remote;
                    let recv = self.recv_stream(stream_id, initial, handle.clone());
                    let send = SendStream {
                        stream_id,
                        handle,
                        finished: false,
                    };
                    let _ = self.bidi_tx.send((recv, send));
                } else {
                    let initial = self.wt.config().initial_max_stream_data_uni;
                    let recv = self.recv_stream(stream_id, initial, handle);
                    let _ = self.uni_tx.send(recv);
                }
                true
            }
            WtEvent::StreamData {
                stream_id,
                data,
                fin,
            } => {
                if self.stop_sent.contains(&stream_id) {
                    // draft-ietf-moq-transport-22 §6.4.2.3: STOP_SENDING 後のデータは
                    // 破棄する。ストリームウィンドウは拡張しない (§6.6 の MUST NOT) が、
                    // 破棄した分の接続全体の受信クレジットは返す (§6.5)。返さないと、
                    // 飛行中のデータを破棄し続けたときにセッションの受信ウィンドウが
                    // 枯渇して接続が恒久的に停止する
                    let size = data.len() as u64;
                    self.session_received = self.session_received.saturating_add(size);
                    self.session_consumed = self.session_consumed.saturating_add(size);
                    self.grow_session_recv_window();
                    if fin {
                        self.stop_sent.remove(&stream_id);
                    }
                    return true;
                }
                let size = data.len() as u64;
                self.session_received = self.session_received.saturating_add(size);
                let delivered = match self.recv_streams.get_mut(&stream_id) {
                    Some(state) => {
                        state.received = state.received.saturating_add(size);
                        state.tx.send(Received::Data { data, fin }).is_ok()
                    }
                    None => false,
                };
                if !delivered {
                    // アプリが受信方向を放棄済みのストリームである。未読データとして
                    // 扱い、接続全体の受信枠だけ戻す
                    tracing::debug!(
                        "Received data for a WebTransport stream without a reader: {stream_id}"
                    );
                    self.session_consumed = self.session_consumed.saturating_add(size);
                    self.recv_streams.remove(&stream_id);
                    self.grow_session_recv_window();
                }
                if fin {
                    self.mark_peer_recv_closed(stream_id);
                }
                true
            }
            WtEvent::StreamReset {
                stream_id,
                error_code,
            } => {
                if let Some(state) = self.recv_streams.remove(&stream_id) {
                    let _ = state.tx.send(Received::Reset { error_code });
                }
                self.stop_sent.remove(&stream_id);
                self.mark_peer_recv_closed(stream_id);
                true
            }
            WtEvent::StopSending {
                stream_id,
                error_code,
            } => {
                // draft-ietf-webtrans-http2-15 §6.3: ピアが送信方向の停止を要求した。
                // 以降の write / finish を失敗させ、未送信データも該当ストリームの終端として
                // 失敗させる。capsule は MOQT §12.5 のコードをそのまま運ぶ
                tracing::debug!(
                    "Received WT_STOP_SENDING for stream {stream_id}: error_code={error_code:#x}"
                );
                self.stopped.insert(stream_id, error_code);
                self.fail_pending_sends(stream_id, SendFailure::Reset(error_code));
                self.mark_peer_send_closed(stream_id);
                true
            }
            WtEvent::DatagramReceived { data } => {
                // MOQT の datagram の解釈は session 層が行う。ここでは受信キューへ渡す
                let _ = self.datagram_tx.send(data);
                true
            }
            WtEvent::SessionDraining => {
                tracing::info!("WebTransport session draining");
                self.draining = true;
                true
            }
            WtEvent::SessionClosed { error_code, reason } => {
                // draft-ietf-webtrans-http2-15 §6.12: WT_CLOSE_SESSION を受信したら
                // CONNECT ストリームを END_STREAM で閉じる (MUST)
                tracing::info!(
                    "WebTransport session closed by peer: error_code={error_code:#x}, reason={reason:?}"
                );
                let _ = self
                    .io
                    .conn
                    .send_data(self.connect_stream, Vec::new(), true);
                let _ = self.io.flush().await;
                false
            }
        }
    }

    /// WT のセッションエラーで接続を中断する
    ///
    /// draft-ietf-webtrans-http2-15 §3.4 / §11.3: セッションエラーは CONNECT ストリームの
    /// RST_STREAM で伝える。エラーコードが特定できる場合は RST_STREAM を送ってから
    /// 接続を終了する。
    async fn fail_session(&mut self, error: WtError) -> bool {
        tracing::warn!("WebTransport over HTTP/2 session failed: {error}");
        if let Some(error_code) = wt_http2_error_code(error.kind())
            && self
                .io
                .conn
                .reset_stream(self.connect_stream, error_code)
                .is_ok()
        {
            let _ = self.io.flush().await;
        }
        false
    }

    /// HTTP/2 の PING を送り、ループを継続するかどうかを返す
    ///
    /// PING への ACK は HTTP/2 層が自動で返すため、応答は待たない。送信に失敗した
    /// 場合は接続が壊れているため false を返す。
    fn send_keep_alive_ping(&mut self) -> bool {
        if let Err(error) = self.io.conn.send_ping([0u8; 8]) {
            tracing::debug!("Failed to send an HTTP/2 keep-alive PING: {error}");
            return false;
        }
        true
    }

    /// 送信待ちのデータを送信ウィンドウの空きに応じて送る
    ///
    /// 完了した reply は戻り値で返す。呼び出し側は WT の capsule への変換と現在の
    /// HTTP/2 出力の書き出しが終わった時点で reply を返す。HTTP/2 の送信ウィンドウが
    /// 枯渇している場合、WT の capsule は駆動タスクの送信バッファ (`wt_out_pending`) に
    /// 残るため、`write_all` が戻った時点で全バイトが TCP に渡っているとは限らない
    /// (接続が継続する限り後で送られる)。
    fn flush_pending_sends(&mut self) -> Vec<(oneshot::Sender<io::Result<()>>, io::Result<()>)> {
        let stream_ids: Vec<WtStreamId> = self.pending_sends.keys().copied().collect();
        let mut completed = Vec::new();
        for stream_id in stream_ids {
            if !self.can_send(stream_id) {
                let failure = self.closed_stream_error(stream_id);
                self.fail_pending_sends(stream_id, failure);
                continue;
            }
            loop {
                let session_available = self.wt.flow_control().send_available();
                let stream_available = self
                    .wt
                    .stream(stream_id)
                    .map_or(0, |stream| stream.send_available());
                let available = session_available.min(stream_available);
                let Some(front) = self
                    .pending_sends
                    .get(&stream_id)
                    .and_then(|queue| queue.front())
                else {
                    break;
                };
                // FIN のみ (空データ) はフロー制御ウィンドウの空きが無くても送れる
                // (RFC 9000 §19.8 の空 STREAM フレームと同じ扱い)
                let empty_fin = front.data.is_empty() && front.fin;
                if available == 0 && !empty_fin {
                    break;
                }
                let take = available.min(front.data.len() as u64) as usize;
                let (chunk, is_last, send_fin) = {
                    let front = self
                        .pending_sends
                        .get_mut(&stream_id)
                        .and_then(|queue| queue.front_mut())
                        .expect("logical invariant: front was checked above");
                    let chunk: Vec<u8> = front.data.drain(..take).collect();
                    let is_last = front.data.is_empty();
                    (chunk, is_last, is_last && front.fin)
                };
                match self.wt.send_stream_data(stream_id, &chunk, send_fin) {
                    Ok(()) => {
                        if is_last {
                            let pending = self
                                .pending_sends
                                .get_mut(&stream_id)
                                .and_then(VecDeque::pop_front)
                                .expect("logical invariant: front was checked above");
                            completed.push((pending.reply, Ok(())));
                            if send_fin {
                                self.mark_peer_send_closed(stream_id);
                            }
                        }
                    }
                    Err(error) => {
                        let pending = self
                            .pending_sends
                            .get_mut(&stream_id)
                            .and_then(VecDeque::pop_front)
                            .expect("logical invariant: front was checked above");
                        completed.push((pending.reply, Err(io_error(error))));
                        break;
                    }
                }
                if self
                    .pending_sends
                    .get(&stream_id)
                    .is_some_and(VecDeque::is_empty)
                {
                    self.pending_sends.remove(&stream_id);
                    break;
                }
            }
        }
        completed
    }

    /// 未送信データを、そのストリームの終端理由で失敗させる
    fn fail_pending_sends(&mut self, stream_id: WtStreamId, failure: SendFailure) {
        if let Some(queue) = self.pending_sends.remove(&stream_id) {
            for pending in queue {
                let _ = pending.reply.send(Err(failure.error()));
            }
        }
    }

    /// 接続終了時に、待っているすべての要求をエラーにする
    fn fail_all_requests(&mut self, message: &str) {
        for (_, queue) in self.pending_sends.drain() {
            for pending in queue {
                let _ = pending.reply.send(Err(io_error(message)));
            }
        }
        for reply in self.after_flush.drain(..) {
            let _ = reply.send(Err(io_error(message)));
        }
        for request in self.opened.drain(..) {
            let _ = request.reply.send(Err(io_error(message)));
        }
    }

    /// 送信可能なストリームかを返す
    fn can_send(&self, stream_id: WtStreamId) -> bool {
        !self.stopped.contains_key(&stream_id)
            && self
                .wt
                .stream(stream_id)
                .is_some_and(|stream| stream.can_send())
    }

    /// 終端済みストリームへの操作に対するエラーを返す
    fn closed_stream_error(&self, stream_id: WtStreamId) -> SendFailure {
        match self.stopped.get(&stream_id).copied() {
            Some(error_code) => SendFailure::Reset(error_code),
            None => SendFailure::Closed,
        }
    }

    /// WT の出力を HTTP/2 の DATA として送信可能な分だけ流す
    fn pump_wt_output(&mut self) -> io::Result<()> {
        while let Some(output) = self.wt.poll_output() {
            self.wt_out_pending.extend_from_slice(&output);
        }
        // 送信バッファに滞留データがある間は追加しない (固定容量 65535 の超過を避ける)
        while !self.wt_out_pending.is_empty()
            && !self.io.conn.has_pending_send_data(self.connect_stream)
        {
            let size = self.wt_out_pending.len().min(WT_SEND_CHUNK_SIZE);
            let chunk: Vec<u8> = self.wt_out_pending.drain(..size).collect();
            self.io
                .conn
                .send_data(self.connect_stream, chunk, false)
                .map_err(io_error)?;
        }
        Ok(())
    }

    /// セッションをクローズする (draft-ietf-webtrans-http2-15 §6.12)
    ///
    /// WT_CLOSE_SESSION を送り、直後に CONNECT ストリームを END_STREAM で half-close する
    /// (MUST)。`code` は MOQT のセッション終了コードである
    /// (draft-ietf-moq-transport-22 §12.2)。
    async fn close_session(&mut self, code: u64) -> io::Result<()> {
        let wire_code = session_close_code(code);
        if !self.wt.is_closed() {
            self.wt
                .close(wire_code, "MOQT session closed")
                .map_err(io_error)?;
        }
        self.pump_wt_output()?;
        self.io.flush().await?;
        // RFC 9113 §6.9.1: 空 DATA + END_STREAM はフロー制御ウィンドウの空きが無くても
        // 送信できる
        self.io
            .conn
            .send_data(self.connect_stream, Vec::new(), true)
            .map_err(io_error)?;
        self.io.flush().await?;
        if self.io.conn.has_pending_send_data(self.connect_stream) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "failed to send END_STREAM on the WebTransport CONNECT stream",
            ));
        }
        Ok(())
    }

    /// ハンドル全破棄時に、ピアへ終了を伝える試みを行う
    ///
    /// ピアが読まない TCP で flush が固まらないよう、[`SHUTDOWN_TIMEOUT`] で打ち切る。
    async fn close_gracefully(&mut self, code: u64) {
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, self.close_session(code)).await;
    }

    /// 受信ウィンドウを消費の進み具合に応じて拡張する (セッション全体)
    ///
    /// draft-ietf-webtrans-http2-15 §6.5: アプリが読んだ分だけ `WT_MAX_DATA` を送る。
    /// 読んでいない分を超えてウィンドウを広げないため、アプリの停滞がメモリの増加に
    /// ならない。
    fn grow_session_recv_window(&mut self) {
        let initial = self.wt.config().initial_max_data;
        let Some(increment) = receive_window_increment(
            initial,
            self.session_received,
            self.session_consumed,
            self.session_grown,
        ) else {
            return;
        };
        match self.wt.grow_recv_window(increment) {
            Ok(()) => self.session_grown = self.session_grown.saturating_add(increment),
            Err(error) => {
                tracing::debug!("Failed to grow the WebTransport session receive window: {error}");
            }
        }
    }

    /// 受信ウィンドウを消費の進み具合に応じて拡張する (ストリーム単位)
    ///
    /// draft-ietf-webtrans-http2-15 §6.6: アプリが読んだ分だけ `WT_MAX_STREAM_DATA` を
    /// 送る。終端済みのストリームでは意味が無いため、存在と受信可否を確認してから送る。
    fn grow_stream_recv_window(&mut self, stream_id: WtStreamId) {
        let Some(state) = self.recv_streams.get(&stream_id) else {
            return;
        };
        let (initial, received, consumed, grown) =
            (state.initial, state.received, state.consumed, state.grown);
        let Some(increment) = receive_window_increment(initial, received, consumed, grown) else {
            return;
        };
        if !self
            .wt
            .stream(stream_id)
            .is_some_and(|stream| stream.can_recv())
        {
            return;
        }
        match self.wt.grow_stream_recv_window(stream_id, increment) {
            Ok(()) => {
                if let Some(state) = self.recv_streams.get_mut(&stream_id) {
                    state.grown = state.grown.saturating_add(increment);
                }
            }
            Err(error) => {
                tracing::debug!(
                    "Failed to grow the receive window of WebTransport stream {stream_id}: {error}"
                );
            }
        }
    }

    /// ピア起動ストリームの受信方向が終端したことを記録する
    fn mark_peer_recv_closed(&mut self, stream_id: WtStreamId) {
        let Some(peer) = self.peer_streams.get_mut(&stream_id) else {
            return;
        };
        peer.recv_closed = true;
        self.credit_peer_stream_if_closed(stream_id);
    }

    /// ピア起動ストリームの送信方向が終端したことを記録する
    fn mark_peer_send_closed(&mut self, stream_id: WtStreamId) {
        let Some(peer) = self.peer_streams.get_mut(&stream_id) else {
            return;
        };
        peer.send_closed = true;
        self.credit_peer_stream_if_closed(stream_id);
    }

    /// ピア起動ストリームが完全に閉じたら、ピアが次のストリームを開けるよう上限を増やす
    ///
    /// draft-ietf-webtrans-http2-15 §6.7: `WT_MAX_STREAMS` でピアが開ける累計ストリーム数を
    /// 通知する。QUIC の MAX_STREAMS と同じく、閉じた 1 本につき 1 増やす。
    /// `WtEvent` は発火しないため、ここで明示的に送る。
    fn credit_peer_stream_if_closed(&mut self, stream_id: WtStreamId) {
        let Some(peer) = self.peer_streams.get(&stream_id) else {
            return;
        };
        if !(peer.recv_closed && peer.send_closed) {
            return;
        }
        let bidirectional = peer.bidirectional;
        self.peer_streams.remove(&stream_id);
        if let Err(error) = self.wt.grow_max_streams(1, bidirectional) {
            // セッションが既に閉じている場合などに起きる。接続全体を落とす必要はない
            tracing::debug!(
                "Failed to raise the WebTransport stream limit after closing {stream_id}: {error}"
            );
        }
    }
}

/// keep-alive の次の PING 送信時刻まで待つ
///
/// 有効になっていない場合は永久に待機し、select! の他の分岐だけを動かす。
async fn next_keep_alive_tick(keep_alive: &mut Option<Interval>) {
    match keep_alive {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending::<()>().await,
    }
}

/// 受信ウィンドウを拡張する量を計算する
///
/// 広告済みの受信上限から見た残りの受信枠 (credit) が初期ウィンドウの半分を切ったら、
/// アプリが消費済みでまだ再広告していない分を返す。初期ウィンドウが 0 の場合は
/// ウィンドウ制御が無効なので拡張しない。
fn receive_window_increment(initial: u64, received: u64, consumed: u64, grown: u64) -> Option<u64> {
    if initial == 0 {
        return None;
    }
    // 現在の受信枠 = 初期ウィンドウ + 追加広告分 - 受信済み
    let credit = initial.saturating_add(grown).saturating_sub(received);
    if credit >= initial.div_ceil(2) {
        return None;
    }
    // アプリが消費した分のうち、まだ再広告していない分を返す
    let increment = consumed.saturating_sub(grown);
    if increment == 0 {
        None
    } else {
        Some(increment)
    }
}

/// MOQT のセッション終了コードを WT_CLOSE_SESSION の 32 ビットコードへ変換する
///
/// draft-ietf-webtrans-http2-15 §6.12: WT_CLOSE_SESSION の Application Error Code は
/// 32 ビットである。MOQT のローカル専用コード (`SESSION_LOCAL_*` =
/// `0xFFFF_FFFF_FFFF_FFxx`) は wire に載せられないため 0 (NO_ERROR) にして警告する。
fn session_close_code(code: u64) -> u32 {
    match u32::try_from(code) {
        Ok(code) => code,
        Err(_) => {
            tracing::warn!(
                "MOQT close code {code} does not fit in a WebTransport over HTTP/2 close code; \
                 sending NO_ERROR"
            );
            0
        }
    }
}

/// セッション終了系の `WtErrorKind` を HTTP/2 のエラーコードへ対応付ける
///
/// draft-ietf-webtrans-http2-15 §3.4 / §11.3: セッションエラーは CONNECT ストリームの
/// RST_STREAM で伝える。対応するコードが無い場合は `None` を返し、RST_STREAM を送らない。
fn wt_http2_error_code(kind: WtErrorKind) -> Option<ErrorCode> {
    match kind {
        WtErrorKind::StreamStateError => Some(ErrorCode::WtStreamStateError),
        WtErrorKind::FlowControlError => Some(ErrorCode::WtFlowControlError),
        WtErrorKind::SessionStateError => Some(ErrorCode::WtError),
        _ => None,
    }
}

/// 接続が閉じていることを表すエラーを作る
fn connection_closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "WebTransport over HTTP/2 connection closed",
    )
}

/// 経路ごとのエラーを `std::io::Error` へ包む
fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests;
