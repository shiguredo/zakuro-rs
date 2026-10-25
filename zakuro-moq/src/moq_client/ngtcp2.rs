//! ngtcp2 のイベント駆動 API を MOQT 用の接続とストリームへ変換する
//!
//! 接続は単一のタスクが所有し、操作をチャネルで受け付ける。受信データを読むまで
//! フロー制御クレジットを戻さず、送信もキューが排出されるまで呼び出し側を待たせる。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Duration;

use base64ct::Encoding;
use shiguredo_ngtcp2_tokio::{
    Client, ClientConfig, ClientConnection, ConnectionEvent, Settings, TransportParams,
};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::StreamReset;
use super::transport::{ALPN, MoqTlsOptions};
use crate::error::{ErrorMessage, Result};

pub(super) mod webtransport;

/// 送信要求 1 件の最大サイズ。64 件の同時要求で送信待ちを約 1 MiB に制限する
const WRITE_CHUNK_SIZE: usize = 16 * 1024;

/// TLS / QUIC の接続設定を作る
///
/// CA ファイルを読めない場合、または PEM 証明書が不正な場合はエラーになる。
pub(crate) fn build_config(options: &MoqTlsOptions) -> Result<ClientConfig> {
    build_config_with_alpn(options, ALPN)
}

/// 経路に対応する ALPN を指定して TLS / QUIC 設定を作る
fn build_config_with_alpn(options: &MoqTlsOptions, alpn: &[u8]) -> Result<ClientConfig> {
    // RFC 9000 §18.2: 他の経路と同じ受信ウィンドウとストリーム数を広告する。
    // Reliable Stream Resets の transport parameter は draft 由来であり変更され得る。
    let params = TransportParams::new()
        .with_initial_max_data(64 * 1024 * 1024)
        .with_initial_max_stream_data_uni(16 * 1024 * 1024)
        .with_initial_max_stream_data_bidi_local(16 * 1024 * 1024)
        .with_initial_max_stream_data_bidi_remote(16 * 1024 * 1024)
        .with_max_streams_uni(10_000)
        .with_max_streams_bidi(1_000)
        .with_datagram(65535)
        .with_reset_stream_at(true);
    let mut settings = Settings::new(0);
    settings.keep_alive_timeout = Some(Duration::from_secs(5));
    let mut config = ClientConfig::new(&[alpn])
        .with_verify_peer(!options.insecure)
        .with_datagram(shiguredo_ngtcp2_tokio::DatagramConfig::default())
        .with_settings(settings)
        .with_transport_params(params);
    if !options.insecure
        && let Some(path) = &options.ca_cert
    {
        let pem = std::fs::read_to_string(path).map_err(|e| {
            ErrorMessage::new(format!(
                "--ca-cert の CA 証明書の読み込みに失敗しました ({path}): {e}"
            ))
        })?;
        // ngtcp2 の API は 1 回に 1 枚だけ取り込むため、PEM バンドルを個別に渡す。
        use rustls::pki_types::pem::PemObject;
        let mut count = 0;
        for certificate in rustls::pki_types::CertificateDer::pem_slice_iter(pem.as_bytes()) {
            let certificate = certificate
                .map_err(|e| ErrorMessage::new(format!("CA 証明書の解釈に失敗しました: {e}")))?;
            let encoded = base64ct::Base64::encode_string(certificate.as_ref());
            config = config.with_ca_cert_pem(format!(
                "-----BEGIN CERTIFICATE-----\n{encoded}\n-----END CERTIFICATE-----\n"
            ));
            count += 1;
        }
        if count == 0 {
            return Err(ErrorMessage::new("--ca-cert に証明書が含まれていません").into());
        }
    }
    Ok(config)
}

/// ngtcp2 の接続を確立し、専用タスクで駆動する
///
/// UDP / TLS / QUIC の接続に失敗した場合はエラーになる。
pub(crate) async fn connect(
    remote: SocketAddr,
    local: SocketAddr,
    server_name: &str,
    config: &ClientConfig,
) -> Result<Connection> {
    let connection = Client::connect_with_config(remote, local, server_name, config)
        .await
        .map_err(io_error)?;
    if connection.selected_alpn_protocol().as_deref() != Some(ALPN) {
        return Err(ErrorMessage::new("MOQ relay が moqt-22 ALPN を選択しませんでした").into());
    }
    Ok(Connection::new(connection))
}

/// ストリーム受信の終端
enum Received {
    Data(Vec<u8>),
    Fin,
    Reset(u64),
    Failed(String),
}

/// 接続タスクへの操作要求
enum Command {
    Open(OpenRequest),
    Write {
        id: i64,
        data: Vec<u8>,
        fin: bool,
        reply: oneshot::Sender<io::Result<()>>,
    },
    Reset {
        id: i64,
        code: u64,
        reply: oneshot::Sender<io::Result<()>>,
    },
    Consumed {
        id: i64,
        size: u64,
    },
    StopRead(i64),
    DropSend {
        id: i64,
        finished: bool,
    },
    Close {
        code: u64,
        reply: oneshot::Sender<()>,
    },
}

/// ストリーム数の枠が空くまで保持する open 要求
struct OpenRequest {
    bidi: bool,
    reply: oneshot::Sender<io::Result<(Option<RecvStream>, SendStream)>>,
}

/// 最後のハンドルが破棄されたら接続タスクも終了する
struct Owner {
    commands: mpsc::UnboundedSender<Command>,
    cancel: CancellationToken,
    writes: Semaphore,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// 接続操作のハンドル
#[derive(Clone)]
pub(crate) struct Handle(Arc<Owner>);

impl Handle {
    /// 単方向の送信ストリームを開く。接続終了時はエラーになる
    pub(crate) async fn open_send_stream(&self) -> io::Result<SendStream> {
        let (_, send) = self.open(false).await?;
        Ok(send)
    }

    /// 双方向ストリームを開き、受信方向と送信方向を返す
    pub(crate) async fn open_bidirectional_stream(&self) -> io::Result<(RecvStream, SendStream)> {
        let (recv, send) = self.open(true).await?;
        Ok((
            recv.expect("logical invariant: bidirectional stream has a receive half"),
            send,
        ))
    }

    async fn open(&self, bidi: bool) -> io::Result<(Option<RecvStream>, SendStream)> {
        let (reply, result) = oneshot::channel();
        self.0
            .commands
            .send(Command::Open(OpenRequest { bidi, reply }))
            .map_err(|_| closed())?;
        result.await.map_err(|_| closed())?
    }

    /// MOQT のエラーコードで接続を閉じる
    pub(crate) async fn close(&self, code: u64) {
        let (reply, result) = oneshot::channel();
        if self.0.commands.send(Command::Close { code, reply }).is_ok() {
            let _ = result.await;
        }
    }
}

/// 送信ストリーム
pub(crate) struct SendStream {
    id: i64,
    handle: Handle,
    finished: bool,
}

impl SendStream {
    /// QUIC ストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        self.id as u64
    }

    /// 全データを送信待ちに渡し、QUIC の送信キューが排出されるまで待つ
    pub(crate) async fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        for chunk in data.chunks(WRITE_CHUNK_SIZE) {
            self.write(chunk, false).await?;
        }
        Ok(())
    }

    async fn write(&mut self, data: &[u8], fin: bool) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream already finished",
            ));
        }
        let _permit = self.handle.0.writes.acquire().await.map_err(|_| closed())?;
        let (reply, result) = oneshot::channel();
        self.handle
            .0
            .commands
            .send(Command::Write {
                id: self.id,
                data: data.to_vec(),
                fin,
                reply,
            })
            .map_err(|_| closed())?;
        result.await.map_err(|_| closed())?
    }

    /// FIN を送る。接続終了時はエラーになる
    pub(crate) async fn finish(&mut self) -> io::Result<()> {
        self.write(&[], true).await?;
        self.finished = true;
        Ok(())
    }

    /// 送信済みデータを保証して reset する。未対応ピアには通常の RESET_STREAM を送る
    pub(crate) async fn reset(&mut self, code: u64) -> io::Result<()> {
        let (reply, result) = oneshot::channel();
        self.handle
            .0
            .commands
            .send(Command::Reset {
                id: self.id,
                code,
                reply,
            })
            .map_err(|_| closed())?;
        result.await.map_err(|_| closed())??;
        self.finished = true;
        Ok(())
    }
}

impl Drop for SendStream {
    fn drop(&mut self) {
        let _ = self.handle.0.commands.send(Command::DropSend {
            id: self.id,
            finished: self.finished,
        });
    }
}

/// 受信ストリーム。読み込みはキャンセルされても未読データを失わない
pub(crate) struct RecvStream {
    id: i64,
    handle: Handle,
    incoming: mpsc::UnboundedReceiver<Received>,
    buffer: Vec<u8>,
    offset: usize,
    end: Option<Option<u64>>,
}

impl RecvStream {
    /// QUIC ストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        self.id as u64
    }

    /// 読み込んだサイズを返す (0 は FIN)。reset と接続断はエラーになる
    pub(crate) async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.offset < self.buffer.len() {
                let size = buf.len().min(self.buffer.len() - self.offset);
                buf[..size].copy_from_slice(&self.buffer[self.offset..self.offset + size]);
                self.offset += size;
                // await を挟まずに消費通知を出し、select! によるキャンセルで欠落させない。
                let _ = self.handle.0.commands.send(Command::Consumed {
                    id: self.id,
                    size: size as u64,
                });
                return Ok(size);
            }
            if let Some(end) = self.end {
                return match end {
                    None => Ok(0),
                    Some(code) => Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        StreamReset(code),
                    )),
                };
            }
            match self.incoming.recv().await.ok_or_else(closed)? {
                Received::Data(data) => {
                    self.buffer = data;
                    self.offset = 0;
                }
                Received::Fin => self.end = Some(None),
                Received::Reset(code) => self.end = Some(Some(code)),
                Received::Failed(message) => return Err(io::Error::other(message)),
            }
        }
    }
}

impl Drop for RecvStream {
    fn drop(&mut self) {
        // 放棄したストリームも接続全体の受信枠を消費し続けないよう、未読分を返す。
        self.incoming.close();
        let mut unread = (self.buffer.len() - self.offset) as u64;
        while let Ok(message) = self.incoming.try_recv() {
            if let Received::Data(data) = message {
                unread += data.len() as u64;
            }
        }
        let _ = self.handle.0.commands.send(Command::Consumed {
            id: self.id,
            size: unread,
        });
        let _ = self.handle.0.commands.send(Command::StopRead(self.id));
    }
}

/// ピアが開始した単方向ストリームの受け入れ口
pub(crate) struct RecvAcceptor {
    incoming: mpsc::UnboundedReceiver<RecvStream>,
    _handle: Handle,
}

impl RecvAcceptor {
    /// ストリームを受け入れる。接続終了時は `Ok(None)` を返す
    pub(crate) async fn accept(&mut self) -> io::Result<Option<RecvStream>> {
        Ok(self.incoming.recv().await)
    }
}

/// ピアが開始した双方向ストリームの受け入れ口
pub(crate) struct BidiAcceptor {
    incoming: mpsc::UnboundedReceiver<(RecvStream, SendStream)>,
    _handle: Handle,
}

impl BidiAcceptor {
    /// ストリームを受け入れる。接続終了時は `Ok(None)` を返す
    pub(crate) async fn accept(&mut self) -> io::Result<Option<(RecvStream, SendStream)>> {
        Ok(self.incoming.recv().await)
    }
}

/// 確立済みの ngtcp2 接続
pub(crate) struct Connection {
    handle: Handle,
    recv: RecvAcceptor,
    bidi: BidiAcceptor,
}

impl Connection {
    fn new(connection: ClientConnection) -> Self {
        Self::with_webtransport(connection, None)
    }

    fn with_webtransport(connection: ClientConnection, wt: Option<webtransport::State>) -> Self {
        let (commands, requests) = mpsc::unbounded_channel();
        let (uni_tx, uni_rx) = mpsc::unbounded_channel();
        let (bidi_tx, bidi_rx) = mpsc::unbounded_channel();
        let handle = Handle(Arc::new(Owner {
            commands,
            cancel: CancellationToken::new(),
            writes: Semaphore::new(64),
        }));
        let driver = Driver {
            connection,
            requests,
            owner: Arc::downgrade(&handle.0),
            cancel: handle.0.cancel.clone(),
            uni_tx,
            bidi_tx,
            receivers: HashMap::new(),
            senders: HashSet::new(),
            opens: VecDeque::new(),
            writes: Vec::new(),
            wt,
            blocked_writes: VecDeque::new(),
            deferred_resets: VecDeque::new(),
        };
        tokio::spawn(async move {
            if let Err(e) = driver.run().await {
                tracing::warn!("ngtcp2 connection failed: {e}");
            }
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
            handle,
        }
    }

    /// 接続ハンドル、単方向 acceptor、双方向 acceptor へ分割する
    pub(crate) fn split(self) -> (Handle, RecvAcceptor, BidiAcceptor) {
        (self.handle, self.recv, self.bidi)
    }
}

/// 接続を単独で所有する駆動タスクの状態
struct Driver {
    connection: ClientConnection,
    requests: mpsc::UnboundedReceiver<Command>,
    owner: Weak<Owner>,
    cancel: CancellationToken,
    uni_tx: mpsc::UnboundedSender<RecvStream>,
    bidi_tx: mpsc::UnboundedSender<(RecvStream, SendStream)>,
    // 受信チャネルのバイト数は QUIC の 64 MiB の接続枠で制限される。bounded チャネルで
    // 接続タスクを止めると他ストリームの ACK やフロー制御も止まるため、ここでは待たない。
    receivers: HashMap<i64, mpsc::UnboundedSender<Received>>,
    senders: HashSet<i64>,
    opens: VecDeque<OpenRequest>,
    writes: Vec<(i64, oneshot::Sender<io::Result<()>>)>,
    /// HTTP/3 経路だけが持つ WebTransport のプロトコル状態
    wt: Option<webtransport::State>,
    /// WebTransport のデータ枠を待つ要求。接続タスクは止めない
    blocked_writes: VecDeque<Command>,
    /// WT ヘッダーが QUIC の送信待ちに残っている間は reset を遅延する
    deferred_resets: VecDeque<Command>,
}

impl Driver {
    fn recv_stream(&mut self, id: i64, handle: Handle) -> RecvStream {
        let (tx, rx) = mpsc::unbounded_channel();
        self.receivers.insert(id, tx);
        RecvStream {
            id,
            handle,
            incoming: rx,
            buffer: Vec::new(),
            offset: 0,
            end: None,
        }
    }

    fn send_stream(&mut self, id: i64, handle: Handle) -> SendStream {
        self.senders.insert(id);
        SendStream {
            id,
            handle,
            finished: false,
        }
    }

    async fn run(mut self) -> io::Result<()> {
        let result = self.drive().await;
        if let Err(error) = &result {
            self.close_failed_webtransport(error).await;
        }
        result
    }

    async fn drive(&mut self) -> io::Result<()> {
        loop {
            self.drain_webtransport()?;
            self.open_pending()?;
            self.retry_blocked_writes().await?;
            self.flush_webtransport()?;
            self.connection.flush().await.map_err(io_error)?;
            if !self.connection.has_pending_data() {
                while let Some(command) = self.deferred_resets.pop_front() {
                    self.command(command).await?;
                }
                for (_, reply) in self.writes.drain(..) {
                    let _ = reply.send(Ok(()));
                }
            }
            tokio::select! {
                biased;
                command = self.requests.recv() => {
                    let Some(command) = command else { return Ok(()); };
                    if !self.command(command).await? { return Ok(()); }
                }
                _ = self.cancel.cancelled() => {
                    if self.wt.is_some() {
                        let _ = self.close_webtransport(0).await;
                    } else {
                        let _ = self.connection.close(0, b"MOQT session dropped").await;
                    }
                    return Ok(());
                }
                event = self.connection.recv_event() => {
                    let event = event.map_err(io_error)?;
                    if !self.transport_event(event)? { return Ok(()); }
                }
                // recv_event は ACK だけでは戻らない。送信キューが空になったことを
                // 定期的にも観測し、最後の write / FIN の完了を待ち続けないようにする。
                _ = tokio::time::sleep(Duration::from_millis(5)), if !self.writes.is_empty() || !self.deferred_resets.is_empty() => {}
            }
        }
    }

    fn open_pending(&mut self) -> io::Result<()> {
        let count = self.opens.len();
        for _ in 0..count {
            let OpenRequest { bidi, reply } = self
                .opens
                .pop_front()
                .expect("logical invariant: queued open exists");
            if reply.is_closed() {
                continue;
            }
            let remaining = if bidi {
                self.connection.streams_bidi_left()
            } else {
                self.connection.streams_uni_left()
            };
            if let Some(wt) = &mut self.wt
                && wt.draining
            {
                let _ = reply.send(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "WebTransport session draining",
                )));
                continue;
            }
            if remaining == 0 {
                self.opens.push_back(OpenRequest { bidi, reply });
                continue;
            }
            if let Some(wt) = &mut self.wt
                && !wt.session.try_open_stream(bidi)
            {
                self.opens.push_back(OpenRequest { bidi, reply });
                continue;
            }
            let result = if bidi {
                self.connection.open_bidi_stream()
            } else {
                self.connection.open_uni_stream()
            };
            match result {
                Ok(id) => {
                    if let Some(wt) = &mut self.wt {
                        wt.open_stream(&mut self.connection, id, bidi)?;
                    }
                    let handle = Handle(self.owner.upgrade().ok_or_else(closed)?);
                    let recv = bidi.then(|| self.recv_stream(id, handle.clone()));
                    let send = self.send_stream(id, handle);
                    let _ = reply.send(Ok((recv, send)));
                }
                Err(e) => {
                    let _ = reply.send(Err(io_error(e)));
                }
            }
        }
        Ok(())
    }

    async fn command(&mut self, command: Command) -> io::Result<bool> {
        match command {
            Command::Open(request) => self.opens.push_back(request),
            Command::Write {
                id,
                mut data,
                fin,
                reply,
            } => {
                if !self.senders.contains(&id) {
                    let _ = reply.send(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "stream send direction closed",
                    )));
                } else {
                    if let Some(wt) = &mut self.wt {
                        // WT_MAX_DATA が write のチャンクより小さくても進められるよう、
                        // 現在の許可量まで送り、残りだけを保留する。
                        let available = wt.available_data(data.len());
                        if !data.is_empty() && available == 0 {
                            wt.session.try_send_data(data.len() as u64);
                            self.blocked_writes.push_back(Command::Write {
                                id,
                                data,
                                fin,
                                reply,
                            });
                            return Ok(true);
                        }
                        if available < data.len() {
                            let remainder = data.split_off(available);
                            wt.session.try_send_data(data.len() as u64);
                            self.connection
                                .write_stream(id, &data, false)
                                .map_err(io_error)?;
                            self.blocked_writes.push_back(Command::Write {
                                id,
                                data: remainder,
                                fin,
                                reply,
                            });
                            return Ok(true);
                        }
                        wt.session.try_send_data(data.len() as u64);
                    }
                    match self.connection.write_stream(id, &data, fin) {
                        Ok(_) => self.writes.push((id, reply)),
                        Err(e) => {
                            let _ = reply.send(Err(io_error(e)));
                        }
                    }
                }
            }
            Command::Reset { id, code, reply } => {
                if self.wt.is_some() && self.connection.has_pending_data() {
                    // RESET_STREAM_AT は ngtcp2 が受け付けた範囲だけを保証する。
                    // WT ヘッダーを未送信のまま捨てるとセッションへ関連付けられなくなる。
                    self.deferred_resets
                        .push_back(Command::Reset { id, code, reply });
                    return Ok(true);
                }
                // draft-ietf-quic-reliable-stream-reset: ピアが広告した場合だけ reliable reset
                // が有効になる。write_all の完了後なので未送信データを保証対象から落とさない。
                // この拡張の API とワイヤ形式は draft の改訂で変わる可能性がある。
                let code = self.wire_error_code(code)?;
                let result = self
                    .connection
                    .reset_stream_reliable(id, code)
                    .map_err(io_error);
                self.senders.remove(&id);
                let _ = reply.send(result);
            }
            Command::Consumed { id, size } => {
                if size > 0 {
                    self.connection
                        .extend_max_stream_offset(id, size)
                        .map_err(io_error)?;
                    if let Some(wt) = &mut self.wt {
                        wt.h3.wt_data_consumed(wt.session.session_id(), size);
                    }
                }
            }
            Command::StopRead(id) => {
                self.receivers.remove(&id);
                let code = self.wire_error_code(0)?;
                let _ = self.connection.stop_sending(id, code);
            }
            Command::DropSend { id, finished } => {
                if self.wt.is_some() && !finished && self.connection.has_pending_data() {
                    self.deferred_resets
                        .push_back(Command::DropSend { id, finished });
                    return Ok(true);
                }
                self.senders.remove(&id);
                if !finished {
                    let code = self.wire_error_code(0)?;
                    let _ = self.connection.reset_stream_reliable(id, code);
                }
            }
            Command::Close { code, reply } => {
                if self.wt.is_some() {
                    self.close_webtransport(code).await?;
                    let _ = reply.send(());
                    return Ok(false);
                }
                self.connection
                    .close(code, b"MOQT session closed")
                    .await
                    .map_err(io_error)?;
                let _ = reply.send(());
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn event(&mut self, event: ConnectionEvent) -> io::Result<bool> {
        match event {
            ConnectionEvent::StreamOpened { stream_id: id } => {
                let handle = Handle(self.owner.upgrade().ok_or_else(closed)?);
                let recv = self.recv_stream(id, handle.clone());
                // RFC 9000 §2.1: bit 1 が 0 のストリームは双方向である。
                if id & 2 == 0 {
                    let send = self.send_stream(id, handle);
                    let _ = self.bidi_tx.send((recv, send));
                } else {
                    let _ = self.uni_tx.send(recv);
                }
            }
            ConnectionEvent::StreamData {
                stream_id: id,
                data,
                fin,
            } => {
                if let Some(tx) = self.receivers.get(&id) {
                    let size = data.len() as u64;
                    if !data.is_empty() && tx.send(Received::Data(data)).is_err() {
                        self.connection
                            .extend_max_stream_offset(id, size)
                            .map_err(io_error)?;
                    }
                    if fin {
                        let _ = tx.send(Received::Fin);
                    }
                } else {
                    self.connection
                        .extend_max_stream_offset(id, data.len() as u64)
                        .map_err(io_error)?;
                }
            }
            ConnectionEvent::StreamReset {
                stream_id,
                app_error_code,
                ..
            } => {
                if let Some(tx) = self.receivers.remove(&stream_id) {
                    let _ = tx.send(Received::Reset(app_error_code));
                }
            }
            ConnectionEvent::StreamStopSending {
                stream_id,
                app_error_code,
            } => {
                self.senders.remove(&stream_id);
                let mut pending = Vec::new();
                for (id, reply) in self.writes.drain(..) {
                    if id == stream_id {
                        let _ = reply.send(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            StreamReset(app_error_code),
                        )));
                    } else {
                        pending.push((id, reply));
                    }
                }
                self.writes = pending;
            }
            ConnectionEvent::StreamClosed { stream_id, .. } => {
                self.receivers.remove(&stream_id);
                self.senders.remove(&stream_id);
                // RFC 9000 §19.11: 処理済みのピア起動ストリームの枠を戻す。
                if stream_id & 1 == 1 {
                    if stream_id & 2 == 0 {
                        self.connection.extend_max_streams_bidi(1);
                    } else {
                        self.connection.extend_max_streams_uni(1);
                    }
                }
            }
            ConnectionEvent::ConnectionClosed {
                error_code, reason, ..
            } => {
                if error_code != 0 && !(self.wt.is_some() && error_code == 0x100) {
                    tracing::warn!(
                        "ngtcp2 peer closed connection: code={error_code} reason={reason:?}"
                    );
                }
                return Ok(false);
            }
            _ => {}
        }
        Ok(true)
    }
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "ngtcp2 connection closed")
}
fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests;
