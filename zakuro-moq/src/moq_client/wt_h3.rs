//! tobi-webtransport を使った WebTransport over HTTP/3
//!
//! tobi_webtransport の `Session` は、ストリームの受け付けと datagram の受信に
//! `&mut self` を要求する。zakuro は MOQT セッションのメインタスクと data stream の
//! 読み取りタスクから同じ接続を同時に操作するため、`Session` を専用タスクが所有し、
//! コマンドとチャネルでやり取りする。

use std::net::SocketAddr;
use std::sync::Arc;

use tobi_quic::Connect;
use tobi_webtransport::client::{Client, ClientConfig, Limits};
use tobi_webtransport::session::{Incoming, Session};
use tobi_webtransport::stream::{WtReceiveStream, WtSendStream};
use tokio::sync::{mpsc, oneshot};

use super::StreamReset;
use super::quic;
use super::transport::ALPN;
use crate::error::{ErrorMessage, Result};

/// ピアが開ける単方向ストリームの上限
///
/// relay は購読ごとに data stream を開くため、既定値の 100 本では不足する。
const INITIAL_MAX_STREAMS_UNI: u64 = 10_000;

/// ピアが開ける双方向ストリームの上限
const INITIAL_MAX_STREAMS_BIDI: u64 = 1_000;

/// WebTransport のフロー制御で広告する受信データ量 (バイト)
const INITIAL_MAX_DATA: u64 = 64 * 1024 * 1024;

/// アプリケーションへ渡す受信ストリームのチャネル容量
const INCOMING_STREAM_CAPACITY: usize = 32;

/// アプリケーションへ渡す受信 datagram のチャネル容量
///
/// QUIC DATAGRAM は再送されない (RFC 9221 §3) ため、あふれた分は失われる。
/// 受信ループが一時的に遅れても取りこぼしを抑えるため、ある程度の深さを持たせる。
const INCOMING_DATAGRAM_CAPACITY: usize = 1_024;

/// WebTransport over HTTP/3 用のクライアントを構築する
///
/// 渡す TLS 設定の ALPN は `h3` に上書きされる (RFC 9114 §3.1)。
///
/// # Errors
///
/// クライアントの起動に失敗した場合はエラーになる。
pub(crate) fn build_client(tls: Arc<rustls::ClientConfig>, is_ipv6: bool) -> Result<Client> {
    Client::builder()
        .with_tls_config(tls)
        .with_io(tobi_quic::IoConfig::new(quic::local_bind_addr(is_ipv6)))
        .start()
        .map_err(|e| {
            ErrorMessage::new(format!(
                "WebTransport over HTTP/3 クライアントの起動に失敗しました: {e}"
            ))
            .into()
        })
}

/// 1 つの接続先へ接続し、WebTransport セッションを開設する
///
/// 戻り値の [`Connection`] は、セッションを所有する駆動タスクとのチャネルである。
///
/// # Errors
///
/// QUIC のハンドシェイク、HTTP/3 の SETTINGS 交換、拡張 CONNECT のいずれかに
/// 失敗した場合はエラーになる。
pub(crate) async fn connect(
    client: &Client,
    remote: SocketAddr,
    server_name: &str,
    authority: &str,
    path: &str,
) -> Result<Connection> {
    // 拡張 CONNECT で MOQT を WebTransport のサブプロトコルとして提示する。
    // draft-ietf-moq-transport-23 §6.2 は WebTransport でも MOQT のプロトコル
    // 識別子を使うと定めている
    let config = ClientConfig::new(authority, path)
        .with_available_protocols(vec![String::from_utf8_lossy(ALPN).into_owned()])
        .with_limits(Limits {
            // 1 接続 1 セッションだけを使う
            max_sessions: 1,
            initial_max_streams_uni: INITIAL_MAX_STREAMS_UNI,
            initial_max_streams_bidi: INITIAL_MAX_STREAMS_BIDI,
            initial_max_data: INITIAL_MAX_DATA,
        });
    let session = client
        .connect(Connect::new(remote, server_name), config)
        .await
        .map_err(|e| {
            ErrorMessage::new(format!(
                "MOQ relay への WebTransport over HTTP/3 接続に失敗しました: {e}"
            ))
        })?;

    let (commands, command_rx) = mpsc::channel(16);
    let (uni_tx, uni_rx) = mpsc::channel(INCOMING_STREAM_CAPACITY);
    let (bidi_tx, bidi_rx) = mpsc::channel(INCOMING_STREAM_CAPACITY);
    let (datagram_tx, datagram_rx) = mpsc::channel(INCOMING_DATAGRAM_CAPACITY);
    tokio::spawn(run(session, command_rx, uni_tx, bidi_tx, datagram_tx));
    Ok(Connection {
        handle: Handle { commands },
        incoming_uni: uni_rx,
        incoming_bidi: bidi_rx,
        datagrams: datagram_rx,
    })
}

/// 開設済みの WebTransport セッション
///
/// `session.rs` は経路を意識せずにこの型を分割し、ストリームの送受信と
/// 接続の終了を行う。
pub(crate) struct Connection {
    /// ストリームの open と datagram の送信、セッションのクローズに使う
    handle: Handle,
    /// ピアが開いた単方向ストリーム
    incoming_uni: mpsc::Receiver<RecvStream>,
    /// ピアが開いた双方向ストリーム
    incoming_bidi: mpsc::Receiver<(RecvStream, SendStream)>,
    /// 受信した datagram
    datagrams: mpsc::Receiver<Vec<u8>>,
}

impl Connection {
    /// 接続ハンドル、accept 用の口、datagram の受け口へ分割する
    pub(crate) fn split(self) -> (Handle, RecvAcceptor, BidiAcceptor, DatagramReceiver) {
        (
            self.handle,
            RecvAcceptor {
                incoming: self.incoming_uni,
            },
            BidiAcceptor {
                incoming: self.incoming_bidi,
            },
            DatagramReceiver {
                incoming: self.datagrams,
            },
        )
    }
}

/// セッションへの操作依頼
enum Command {
    /// 単方向ストリームを開く
    OpenSendStream(oneshot::Sender<std::io::Result<SendStream>>),
    /// 双方向ストリームを開く
    OpenBidirectionalStream(oneshot::Sender<std::io::Result<(RecvStream, SendStream)>>),
    /// datagram を送信する
    SendDatagram {
        /// 送信する payload
        data: Vec<u8>,
        /// 送信結果の通知先
        reply: oneshot::Sender<std::io::Result<()>>,
    },
    /// セッションをクローズする
    Close {
        /// MOQT のセッション終了コード
        error_code: u64,
        /// クローズ完了の通知先
        reply: oneshot::Sender<()>,
    },
}

/// セッションを所有する駆動タスク
///
/// `Session` を 1 つのタスクに閉じ込め、コマンドと受信の配送を直列化する。
/// 送信側のストリーム (`WtSendStream`) はセッションを経由せずに書けるため、
/// ペイロードの送信がこのタスクの滞留に影響されることはない。
async fn run(
    mut session: Session,
    mut commands: mpsc::Receiver<Command>,
    uni_tx: mpsc::Sender<RecvStream>,
    bidi_tx: mpsc::Sender<(RecvStream, SendStream)>,
    datagram_tx: mpsc::Sender<Vec<u8>>,
) {
    loop {
        // 受信の分岐では session を使わない (チャネルへの配送だけ行う)。コマンドの
        // 処理は select! の外で行い、session の可変借用が分岐の本体まで続かない
        // ようにする
        let command = tokio::select! {
            command = commands.recv() => command,
            incoming = session.receive() => {
                match incoming {
                    Ok(Incoming::UnidirectionalStream(stream)) => {
                        if uni_tx.send(RecvStream::new(stream)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Incoming::BidirectionalStream(stream)) => {
                        let (send, recv) = stream.split();
                        if bidi_tx
                            .send((RecvStream::new(recv), SendStream::new(send)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Incoming::Datagram(data)) => {
                        if datagram_tx.send(data).await.is_err() {
                            break;
                        }
                    }
                    // セッションの終了。すべての受信キューが閉じている
                    Err(e) => {
                        tracing::debug!("WebTransport session receive ended: {e}");
                        break;
                    }
                }
                continue;
            }
        };

        // アプリケーションが接続ハンドルを全て手放した。Session の drop が
        // クローズコード 0 の後始末を行う
        let Some(command) = command else {
            break;
        };
        match command {
            Command::OpenSendStream(reply) => {
                let stream = session
                    .open_unidirectional_stream()
                    .await
                    .map(SendStream::new)
                    .map_err(io_error);
                let _ = reply.send(stream);
            }
            Command::OpenBidirectionalStream(reply) => {
                let stream = session
                    .open_bidirectional_stream()
                    .await
                    .map(|stream| {
                        let (send, recv) = stream.split();
                        (RecvStream::new(recv), SendStream::new(send))
                    })
                    .map_err(io_error);
                let _ = reply.send(stream);
            }
            Command::SendDatagram { data, reply } => {
                let result = session.send_datagram(&data).await.map_err(io_error);
                let _ = reply.send(result);
            }
            Command::Close { error_code, reply } => {
                // WebTransport のアプリケーションエラーコードは u32 である
                // (draft-ietf-webtrans-http3-15 §4.4)。MOQT のセッション終了コードは
                // u32 に収まるため、収まらない値は u32::MAX に丸める
                let code = u32::try_from(error_code).unwrap_or(u32::MAX);
                if let Err(e) = session.close(code, "MOQT session closed").await {
                    tracing::debug!("WebTransport session close failed: {e}");
                }
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// WebTransport セッションの操作ハンドル
#[derive(Clone)]
pub(crate) struct Handle {
    /// 駆動タスクへの操作依頼
    commands: mpsc::Sender<Command>,
}

impl Handle {
    /// 新しい単方向送信ストリームを開く
    ///
    /// # Errors
    ///
    /// セッションが終了している場合、またはピアのストリーム数上限に達している
    /// 場合はエラーになる。
    pub(crate) async fn open_send_stream(&self) -> std::io::Result<SendStream> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::OpenSendStream(reply))
            .await
            .map_err(|_| closed_error())?;
        response.await.map_err(|_| closed_error())?
    }

    /// 新しい双方向ストリームを開き、受信方向と送信方向を返す
    ///
    /// # Errors
    ///
    /// セッションが終了している場合、またはピアのストリーム数上限に達している
    /// 場合はエラーになる。
    pub(crate) async fn open_bidirectional_stream(
        &self,
    ) -> std::io::Result<(RecvStream, SendStream)> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::OpenBidirectionalStream(reply))
            .await
            .map_err(|_| closed_error())?;
        response.await.map_err(|_| closed_error())?
    }

    /// OBJECT_DATAGRAM を 1 つ送信する
    ///
    /// # Errors
    ///
    /// セッションが終了している場合、ピアが datagram に対応していない場合、
    /// またはサイズが上限を超える場合はエラーになる。
    pub(crate) async fn send_datagram(&self, data: &[u8]) -> std::io::Result<()> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SendDatagram {
                data: data.to_vec(),
                reply,
            })
            .await
            .map_err(|_| closed_error())?;
        response.await.map_err(|_| closed_error())?
    }

    /// セッションを閉じる
    ///
    /// セッションの終了コードは WebTransport のアプリケーションエラーコードとして
    /// 送る。QUIC の接続終了は tobi-webtransport が後始末する。
    pub(crate) async fn close(&self, error_code: u64) {
        let (reply, response) = oneshot::channel();
        if self
            .commands
            .send(Command::Close { error_code, reply })
            .await
            .is_err()
        {
            return;
        }
        let _ = response.await;
    }
}

/// ピア起動の単方向ストリームを受け入れる口
pub(crate) struct RecvAcceptor {
    /// 駆動タスクが配送するストリーム
    incoming: mpsc::Receiver<RecvStream>,
}

impl RecvAcceptor {
    /// 単方向ストリームを 1 本受け入れる
    ///
    /// セッションが終了した場合は `Ok(None)` を返す。
    pub(crate) async fn accept(&mut self) -> std::io::Result<Option<RecvStream>> {
        Ok(self.incoming.recv().await)
    }
}

/// ピア起動の双方向ストリームを受け入れる口
pub(crate) struct BidiAcceptor {
    /// 駆動タスクが配送するストリーム
    incoming: mpsc::Receiver<(RecvStream, SendStream)>,
}

impl BidiAcceptor {
    /// 双方向ストリームを 1 本受け入れる
    ///
    /// 戻り値の順序は `(受信, 送信)` に揃える。セッションが終了した場合は
    /// `Ok(None)` を返す。
    pub(crate) async fn accept(&mut self) -> std::io::Result<Option<(RecvStream, SendStream)>> {
        Ok(self.incoming.recv().await)
    }
}

/// ピアから届く datagram の受け口
pub(crate) struct DatagramReceiver {
    /// 駆動タスクが配送する datagram
    incoming: mpsc::Receiver<Vec<u8>>,
}

impl DatagramReceiver {
    /// 到着済みの datagram をまとめて受け取る
    ///
    /// 1 件以上届くまで待ち、その時点で受け取れる分をすべてまとめて返す。
    /// 返る `Vec` は空にならない。セッションが終了した場合は `Ok(None)` を返す。
    pub(crate) async fn recv_datagrams(&mut self) -> std::io::Result<Option<Vec<Vec<u8>>>> {
        let Some(first) = self.incoming.recv().await else {
            return Ok(None);
        };
        let mut datagrams = vec![first];
        while let Ok(next) = self.incoming.try_recv() {
            datagrams.push(next);
        }
        Ok(Some(datagrams))
    }
}

/// WebTransport の送信ストリーム
pub(crate) struct SendStream {
    /// tobi-webtransport の送信ストリーム
    stream: WtSendStream,
}

impl SendStream {
    /// 送信ストリームを包む
    fn new(stream: WtSendStream) -> Self {
        Self { stream }
    }

    /// ストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        self.stream.stream_id()
    }

    /// すべてのバイト列を書き込む
    ///
    /// # Errors
    ///
    /// ストリームの書き込みに失敗した場合はエラーになる。
    pub(crate) async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.stream.send(buf).await.map_err(io_error)
    }

    /// ストリームの送信方向を FIN で閉じる
    ///
    /// # Errors
    ///
    /// FIN の送信に失敗した場合はエラーになる。
    pub(crate) async fn finish(&mut self) -> std::io::Result<()> {
        self.stream.finish().await.map_err(io_error)
    }

    /// ストリームの送信方向を reset する
    ///
    /// WebTransport のアプリケーションエラーコードは u32 である
    /// (draft-ietf-webtrans-http3-15 §4.4)。
    ///
    /// # Errors
    ///
    /// エラーコードが u32 に収まらない場合、または reset の送信に失敗した場合は
    /// エラーになる。
    pub(crate) async fn reset(&mut self, error_code: u64) -> std::io::Result<()> {
        let code = u32::try_from(error_code).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("WebTransport のエラーコードは u32 までです: {error_code}"),
            )
        })?;
        self.stream.reset(code).await.map_err(io_error)
    }
}

impl std::fmt::Debug for SendStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WtH3SendStream")
            .field(&self.stream.stream_id())
            .finish()
    }
}

/// WebTransport の受信ストリーム
///
/// tobi-webtransport の受信 API はチャンク単位なので、呼び出し側が要求した
/// バッファへ詰め替えるための残りを保持する。
pub(crate) struct RecvStream {
    /// tobi-webtransport の受信ストリーム
    stream: WtReceiveStream,
    /// まだ呼び出し側へ返していない受信データ
    pending: Vec<u8>,
    /// `pending` 内の返却位置
    offset: usize,
}

impl RecvStream {
    /// 受信ストリームを包む
    fn new(stream: WtReceiveStream) -> Self {
        Self {
            stream,
            pending: Vec::new(),
            offset: 0,
        }
    }

    /// ストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        self.stream.stream_id()
    }

    /// 読み込んだバイト数を返す (0 は FIN)
    ///
    /// # Errors
    ///
    /// 読み込みとストリームの reset はエラーになる。reset のエラーコードは
    /// [`StreamReset`] としてエラーに載せる。
    pub(crate) async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // 前回受け取ったチャンクの残りを先に返す
        if self.offset < self.pending.len() {
            let len = (self.pending.len() - self.offset).min(buf.len());
            buf[..len].copy_from_slice(&self.pending[self.offset..self.offset + len]);
            self.offset += len;
            if self.offset == self.pending.len() {
                self.pending.clear();
                self.offset = 0;
            }
            return Ok(len);
        }
        match self.stream.recv().await {
            Ok(Some(data)) => {
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
                if len < data.len() {
                    self.pending = data;
                    self.offset = len;
                }
                Ok(len)
            }
            // FIN
            Ok(None) => Ok(0),
            Err(e) => Err(stream_error(e)),
        }
    }
}

impl std::fmt::Debug for RecvStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WtH3RecvStream")
            .field(&self.stream.stream_id())
            .finish()
    }
}

/// ストリームの reset を [`StreamReset`] として `io::Error` に載せる
///
/// セッションの終了 (WT_SESSION_GONE) も `StreamReset` として届くため、
/// 呼び出し側は reset として扱う。ピアが `WT_APPLICATION_ERROR` の範囲外の
/// コードでリセットした場合はアプリケーションエラーコードを観測できないため
/// (draft-ietf-webtrans-http3-15 §4.4)、通常の読み込みエラーとして返す。
fn stream_error(error: tobi_webtransport::error::Error) -> std::io::Error {
    match &error {
        tobi_webtransport::error::Error::StreamReset {
            error_code: Some(error_code),
            ..
        } => std::io::Error::other(StreamReset(u64::from(*error_code))),
        _ => io_error(error),
    }
}

/// 経路ごとのエラーを `std::io::Error` へ包む
fn io_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("{error}"))
}

/// ハンドルが閉じているときのエラー
fn closed_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotConnected,
        "WebTransport session is closed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use tobi_webtransport::server::{ServerLimits, WtSessionRequest};

    /// loopback で WebTransport over HTTP/3 のサーバーを起動する
    ///
    /// 自己署名証明書を生成し、ALPN `h3`、QUIC DATAGRAM、`reset_stream_at`
    /// transport parameter を有効にする。モックやスタブではなく、実際の
    /// QUIC と HTTP/3 のハンドシェイクを行なうサーバーである。
    fn start_wt_server() -> (SocketAddr, tobi_quic::Server, CertificateDer<'static>) {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("自己署名証明書を生成できること");
        let certificate = certified.cert.der().clone();
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 を設定できること")
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], private_key)
        .expect("証明書と秘密鍵を設定できること");
        // HTTP/3 の ALPN は `h3` である (RFC 9114 §3.1)
        tls.alpn_protocols = vec![b"h3".to_vec()];

        // draft-14 / 15 の WebTransport は RESET_STREAM_AT の広告を前提とする
        // (draft-ietf-webtrans-http3-15 §4.4)
        let mut transport = tobi_quic::core::connection::ConnectionConfig::default();
        transport.transport_params.reset_stream_at = true;
        let server = tobi_quic::Server::builder()
            .with_tls(tobi_quic::ServerTlsConfig::new(Arc::new(tls)))
            .with_io(tobi_quic::IoConfig::new(
                "127.0.0.1:0".parse().expect("アドレスを解釈できること"),
            ))
            .with_transport(transport)
            .with_datagram(tobi_quic::DatagramConfig::default())
            .start()
            .expect("QUIC サーバーを起動できること");
        (server.local_addr(), server, certificate)
    }

    /// サーバー側で WebTransport セッションを開設する
    async fn accept_wt_session(
        server: &mut tobi_quic::Server,
    ) -> tobi_webtransport::server::WtSession {
        let connection = server.accept().await.expect("接続を受け入れられること");
        WtSessionRequest::from_connection(connection, ServerLimits::default(), false)
            .await
            .expect("WebTransport セッション要求を作れること")
            // クライアントは `moqt-23` を提示するため、選択結果を返す
            .with_protocol("moqt-23")
            .accept()
            .await
            .expect("WebTransport セッションを開設できること")
    }

    /// クライアント側の TLS 設定を構築する
    fn build_client_tls(certificate: CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(certificate)
            .expect("自己署名証明書を信頼ストアへ追加できること");
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 を設定できること")
        .with_root_certificates(roots)
        .with_no_client_auth();
        Arc::new(tls)
    }

    /// 起動済みのサーバーへ WebTransport over HTTP/3 で接続する
    async fn connect_wt(address: SocketAddr, certificate: CertificateDer<'static>) -> Connection {
        let tls = build_client_tls(certificate);
        let client = build_client(tls, false).expect("クライアントを構築できること");
        connect(&client, address, "localhost", "localhost", "/")
            .await
            .expect("WebTransport over HTTP/3 で接続できること")
    }

    /// WebTransport over HTTP/3 で単方向ストリームが往復すること
    ///
    /// MOQT の data stream は単方向ストリームで送る。実際の QUIC + HTTP/3 の
    /// 接続で open / write / finish / accept / read が成立することを確かめる。
    #[tokio::test(flavor = "multi_thread")]
    async fn wt_h3_unidirectional_stream_round_trip() {
        let (address, mut server, certificate) = start_wt_server();
        // サーバーの accept が QUIC のハンドシェイクを進めるため、クライアントの
        // 接続より先に待たせておく
        let server_task = tokio::spawn(async move {
            let mut session = accept_wt_session(&mut server).await;
            let mut stream = session
                .accept_uni_stream()
                .await
                .expect("単方向 stream を受け入れられること");
            let mut received = Vec::new();
            // FIN は StreamClosed エラーで通知される
            while let Ok(data) = stream.recv().await {
                received.extend_from_slice(&data);
            }
            received
        });

        let connection = connect_wt(address, certificate).await;
        let (handle, _, _, _) = connection.split();

        let payload = b"hello tobi-webtransport";
        let mut send = handle
            .open_send_stream()
            .await
            .expect("単方向 stream を開けること");
        send.write_all(payload).await.expect("payload を書けること");
        send.finish().await.expect("FIN を送れること");

        let received = tokio::time::timeout(std::time::Duration::from_secs(10), server_task)
            .await
            .expect("stream の受信がタイムアウトしないこと")
            .expect("サーバータスクが完了すること");
        assert_eq!(received, payload);
    }

    /// WebTransport over HTTP/3 で datagram が送受信できること
    ///
    /// MOQT の OBJECT_DATAGRAM は HTTP Datagram に載せて QUIC DATAGRAM で運ぶ。
    /// loopback の実接続で、送信した datagram が反対側で取り出せることを確かめる。
    #[tokio::test(flavor = "multi_thread")]
    async fn wt_h3_datagram_round_trip() {
        let (address, mut server, certificate) = start_wt_server();
        // サーバーの accept が QUIC のハンドシェイクを進めるため、クライアントの
        // 接続より先に待たせておく
        let server_task = tokio::spawn(async move {
            let mut session = accept_wt_session(&mut server).await;
            session
                .receive_datagram()
                .await
                .expect("datagram を受信できること")
        });

        let connection = connect_wt(address, certificate).await;
        let (handle, _, _, _) = connection.split();

        let payload = b"audio object";
        handle
            .send_datagram(payload)
            .await
            .expect("datagram を送信できること");

        let received = tokio::time::timeout(std::time::Duration::from_secs(10), server_task)
            .await
            .expect("datagram の受信がタイムアウトしないこと")
            .expect("サーバータスクが完了すること");
        assert_eq!(received, payload);
    }
}
