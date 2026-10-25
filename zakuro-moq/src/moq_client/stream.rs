//! MOQT の I/O 層で使うストリーム、datagram、接続ハンドルの抽象化
//!
//! `shiguredo_moqt::Session` は Sans I/O の状態機械であり、ワイヤの読み書きは呼び出し側の
//! 責務である。zakuro-moq は QUIC 直接接続 (s2n-quic / ngtcp2)、WebTransport over HTTP/3、QMux の経路を持ち、
//! `session.rs` は経路を意識せずにセッションを駆動したい。そこでストリーム・接続ハンドル・
//! accept・datagram の送受信を enum で包み、必要な操作だけを提供する。
//!
//! トレイトは作らない (時雨堂 Rust 規約)。各経路が同じ操作を提供する
//! ため、enum の match で十分である。

use std::sync::Arc;
use std::task::Poll;

use bytes::Bytes;
use s2n_quic::connection::{BidirectionalStreamAcceptor, Handle, ReceiveStreamAcceptor};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ngtcp2, qmux, wt_h2};

/// 送信ストリーム
pub(crate) enum MoqSendStream {
    /// QUIC 直接接続の送信ストリーム
    Quic(s2n_quic::stream::SendStream),
    /// ngtcp2 の送信ストリーム
    Ngtcp2(ngtcp2::SendStream),
    /// WebTransport over HTTP/2 の送信ストリーム
    WtH2(wt_h2::SendStream),
    /// QMux の送信ストリーム
    Qmux(tokio_qmux::SendStream),
}

impl MoqSendStream {
    /// ストリーム ID を返す
    ///
    /// MOQT の `DataStreamId` に使う。経路ごとに ID の空間は異なるが、`Session` は値の意味を
    /// 解釈せず、同一ストリームを同一値で通知されることだけを前提にする。
    pub(crate) fn id(&self) -> u64 {
        match self {
            Self::Quic(stream) => stream.id(),
            Self::Ngtcp2(stream) => stream.id(),
            Self::WtH2(stream) => stream.id(),
            Self::Qmux(stream) => stream.id().as_u64(),
        }
    }

    /// すべてのバイト列を書き込む
    ///
    /// # Errors
    ///
    /// ストリームの書き込みに失敗した場合はエラーになる。
    pub(crate) async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            // s2n-quic の SendStream は AsyncWrite を実装している
            Self::Quic(stream) => stream.write_all(buf).await,
            Self::Ngtcp2(stream) => stream.write_all(buf).await,
            Self::WtH2(stream) => stream.write_all(buf).await,
            // tokio-qmux の write は渡したデータをすべて受け付けるまで待つ
            Self::Qmux(stream) => stream.write(buf).await.map(|_| ()).map_err(io_error),
        }
    }

    /// ストリームの送信方向を FIN で閉じる
    ///
    /// # Errors
    ///
    /// FIN の送信に失敗した場合はエラーになる。
    pub(crate) async fn finish(&mut self) -> std::io::Result<()> {
        match self {
            Self::Quic(stream) => stream.finish().map_err(io_error),
            Self::Ngtcp2(stream) => stream.finish().await,
            Self::WtH2(stream) => stream.finish().await,
            Self::Qmux(stream) => stream.finish().await.map_err(io_error),
        }
    }

    /// ストリームの送信方向を reset する
    ///
    /// `error_code` は MOQT のストリームリセットエラーコード
    /// (draft-ietf-moq-transport-22 §12.5) である。QMux は QUIC version 1 のフレームを
    /// 多重化するため、コード空間は native QUIC と共通である
    /// (draft-ietf-quic-qmux-02 Section 4)。
    /// ngtcp2 は対応ピアに RESET_STREAM_AT を送り、送信済みのバイト列を保証する。
    ///
    /// # Errors
    ///
    /// reset の送信に失敗した場合はエラーになる。
    pub(crate) async fn reset(&mut self, error_code: u64) -> std::io::Result<()> {
        match self {
            Self::Quic(stream) => stream
                .reset(application_error(error_code))
                .map_err(io_error),
            Self::Qmux(stream) => stream.reset(error_code).await.map_err(io_error),
            Self::Ngtcp2(stream) => stream.reset(error_code).await,
            Self::WtH2(stream) => stream.reset(error_code).await,
        }
    }
}

/// 受信ストリーム
pub(crate) enum MoqRecvStream {
    /// QUIC 直接接続の受信ストリーム
    Quic(s2n_quic::stream::ReceiveStream),
    /// ngtcp2 の受信ストリーム
    Ngtcp2(ngtcp2::RecvStream),
    /// WebTransport over HTTP/2 の受信ストリーム
    WtH2(wt_h2::RecvStream),
    /// QMux の受信ストリーム
    Qmux(tokio_qmux::RecvStream),
}

impl MoqRecvStream {
    /// ストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        match self {
            Self::Quic(stream) => stream.id(),
            Self::Ngtcp2(stream) => stream.id(),
            Self::WtH2(stream) => stream.id(),
            Self::Qmux(stream) => stream.id().as_u64(),
        }
    }

    /// 読み込んだバイト数を返す (0 は FIN)
    ///
    /// # Errors
    ///
    /// 読み込みとストリームの reset はエラーになる。ngtcp2 の reset エラーは
    /// [`super::StreamReset`] としてエラーコードを保持する。
    pub(crate) async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Quic(stream) => stream.read(buf).await,
            Self::Ngtcp2(stream) => stream.read(buf).await,
            Self::WtH2(stream) => stream.read(buf).await,
            Self::Qmux(stream) => match stream.read(buf).await {
                Ok(Some(size)) => Ok(size),
                // QMux の read は FIN を Ok(None) で表す
                Ok(None) => Ok(0),
                Err(e) => Err(io_error(e)),
            },
        }
    }
}

impl std::fmt::Debug for MoqRecvStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Quic(stream) => f.debug_tuple("Quic").field(stream).finish(),
            Self::Ngtcp2(stream) => f.debug_tuple("Ngtcp2").field(&stream.id()).finish(),
            Self::WtH2(stream) => f.debug_tuple("WtH2").field(&stream.id()).finish(),
            Self::Qmux(stream) => f.debug_tuple("Qmux").field(stream).finish(),
        }
    }
}

/// 接続のハンドル
///
/// 複数タスクから clone して使う (ストリームの open と接続の close)。s2n-quic は
/// `Handle`、ngtcp2 と WebTransport over HTTP/2 は駆動タスクへのチャネル、QMux は
/// `Arc<tokio_qmux::Connection>` を持つ。
#[derive(Clone)]
pub(crate) enum MoqConnectionHandle {
    /// QUIC 直接接続のハンドル
    Quic(Handle),
    /// ngtcp2 の接続ハンドル
    Ngtcp2(ngtcp2::Handle),
    /// WebTransport over HTTP/2 の接続ハンドル
    WtH2(wt_h2::Handle),
    /// QMux 接続のハンドル
    Qmux(Arc<tokio_qmux::Connection>),
}

impl MoqConnectionHandle {
    /// 新しい単方向送信ストリームを開く
    ///
    /// # Errors
    ///
    /// ストリームの open に失敗した場合はエラーになる。
    pub(crate) async fn open_send_stream(&mut self) -> std::io::Result<MoqSendStream> {
        match self {
            Self::Quic(handle) => handle
                .open_send_stream()
                .await
                .map(MoqSendStream::Quic)
                .map_err(io_error),
            Self::Qmux(connection) => connection
                .open_unidirectional()
                .await
                .map(MoqSendStream::Qmux)
                .map_err(io_error),
            Self::Ngtcp2(handle) => handle.open_send_stream().await.map(MoqSendStream::Ngtcp2),
            Self::WtH2(handle) => handle.open_send_stream().await.map(MoqSendStream::WtH2),
        }
    }

    /// 新しい双方向ストリームを開き、受信方向と送信方向を返す
    ///
    /// 戻り値の順序は QUIC / QMux で揃えて `(受信, 送信)` にする。
    ///
    /// # Errors
    ///
    /// ストリームの open に失敗した場合はエラーになる。
    pub(crate) async fn open_bidirectional_stream(
        &mut self,
    ) -> std::io::Result<(MoqRecvStream, MoqSendStream)> {
        match self {
            Self::Ngtcp2(handle) => {
                let (recv, send) = handle.open_bidirectional_stream().await?;
                Ok((MoqRecvStream::Ngtcp2(recv), MoqSendStream::Ngtcp2(send)))
            }
            Self::WtH2(handle) => {
                let (recv, send) = handle.open_bidirectional_stream().await?;
                Ok((MoqRecvStream::WtH2(recv), MoqSendStream::WtH2(send)))
            }
            Self::Quic(handle) => {
                let stream = handle.open_bidirectional_stream().await.map_err(io_error)?;
                let (recv, send) = stream.split();
                Ok((MoqRecvStream::Quic(recv), MoqSendStream::Quic(send)))
            }
            Self::Qmux(connection) => {
                // tokio-qmux は (送信, 受信) の順で返す
                let (send, recv) = connection.open_bidirectional().await.map_err(io_error)?;
                Ok((MoqRecvStream::Qmux(recv), MoqSendStream::Qmux(send)))
            }
        }
    }

    /// 接続を閉じる
    ///
    /// MOQT のセッション終了コード (`error_code`) を接続層へ伝える。QMux は QUIC version 1 の
    /// コード空間を共有するため、native QUIC と同じ値をそのまま載せる
    /// (draft-ietf-quic-qmux-02 Section 4)。
    pub(crate) async fn close(&self, error_code: u64) {
        match self {
            Self::Quic(handle) => handle.close(application_error(error_code)),
            Self::Ngtcp2(handle) => handle.close(error_code).await,
            Self::WtH2(handle) => handle.close(error_code).await,
            Self::Qmux(connection) => {
                // QMux の CONNECTION_CLOSE は理由文字列を要求する。値は可変長整数の範囲に
                // 収まる必要があり、収まらない場合は送信されない (RFC 9000 Section 19.19)
                if let Err(e) = connection.close(error_code, "MOQT session closed") {
                    tracing::warn!("QMux connection close failed: {e}");
                }
            }
        }
    }

    /// 接続をアイドルタイムアウトで切られないように維持する
    ///
    /// s2n-quic の keep-alive を有効にする。ngtcp2 は接続構築時に有効にしている。
    /// QMux には keep-alive の API が無いため、
    /// QX_PING を定期的に送るタスクを起動する ([`qmux::spawn_keep_alive`])。
    /// WebTransport over HTTP/2 は駆動タスクに HTTP/2 PING を定期送信させる
    /// ([`wt_h2::Handle::enable_keep_alive`])。
    ///
    /// # Errors
    ///
    /// QUIC の keep-alive の有効化に失敗した場合はエラーになる。QMux / WebTransport over
    /// HTTP/2 の keep-alive は駆動タスクへの通知のみで失敗しない (接続終了で自ら止まる)。
    pub(crate) fn keep_alive(&mut self, token: &CancellationToken) -> std::io::Result<()> {
        match self {
            Self::Quic(handle) => handle.keep_alive(true).map_err(io_error),
            // ngtcp2 の keep-alive は接続作成時に設定済みである
            Self::Ngtcp2(_) => Ok(()),
            // HTTP/2 の PING を駆動タスクが送る。MOQT には PING が無いため、
            // 無通信の接続 (Forward State 0 など) を relay の idle timeout で切られない
            // ようにする
            Self::WtH2(handle) => {
                handle.enable_keep_alive();
                Ok(())
            }
            Self::Qmux(connection) => {
                qmux::spawn_keep_alive(connection, token);
                Ok(())
            }
        }
    }

    /// OBJECT_DATAGRAM を 1 つ送信する
    ///
    /// datagram は再送されない (RFC 9221 §3) ため、失敗しても再送しない。QUIC の送信キューが
    /// 満杯の場合は空きができるまで待つ。待たずに捨てると音声の欠落として現れるが、
    /// 負荷試験では送信できた object 数を正しく数えたいため待つ。
    ///
    /// # Errors
    ///
    /// datagram を送信できない場合 (接続終了、ピアが datagram 非対応、サイズ超過など) は
    /// エラーになる。
    pub(crate) async fn send_datagram(&self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Quic(handle) => {
                let mut bytes = Bytes::copy_from_slice(data);
                // `poll_send_datagram` は空きが無いとき `Waker` を登録するため、
                // ポーリングで待たずに空きができ次第おきる
                std::future::poll_fn(|cx| {
                    let result = handle.datagram_mut(
                        |sender: &mut s2n_quic::provider::datagram::default::Sender| {
                            sender.poll_send_datagram(&mut bytes, cx)
                        },
                    );
                    match result {
                        Ok(poll) => poll.map_err(|e| io_error(format!("datagram send error: {e}"))),
                        Err(e) => Poll::Ready(Err(io_error(format!("datagram query error: {e}")))),
                    }
                })
                .await
            }
            Self::Ngtcp2(handle) => handle.send_datagram(data).await,
            Self::WtH2(handle) => handle.send_datagram(data).await,
            Self::Qmux(connection) => connection
                .send_datagram(data)
                .await
                .map_err(|e| io_error(format!("QMux datagram send error: {e}"))),
        }
    }
}

/// ピア起動の単方向ストリーム (data stream) を受け入れる口
pub(crate) enum MoqRecvAcceptor {
    /// QUIC 直接接続の単方向ストリーム acceptor
    Quic(ReceiveStreamAcceptor),
    /// ngtcp2 の単方向ストリーム acceptor
    Ngtcp2(ngtcp2::RecvAcceptor),
    /// WebTransport over HTTP/2 の単方向ストリーム acceptor
    WtH2(wt_h2::RecvAcceptor),
    /// QMux 接続 (accept は `&self` で待てる)
    Qmux(Arc<tokio_qmux::Connection>),
}

impl MoqRecvAcceptor {
    /// 単方向ストリームを 1 本受け入れる
    ///
    /// 接続が閉じた場合は `Ok(None)` を返す。
    ///
    /// # Errors
    ///
    /// accept に失敗した場合はエラーになる。
    pub(crate) async fn accept_receive_stream(&mut self) -> std::io::Result<Option<MoqRecvStream>> {
        match self {
            Self::Ngtcp2(acceptor) => acceptor
                .accept()
                .await
                .map(|stream| stream.map(MoqRecvStream::Ngtcp2)),
            Self::WtH2(acceptor) => acceptor
                .accept()
                .await
                .map(|stream| stream.map(MoqRecvStream::WtH2)),
            Self::Quic(acceptor) => acceptor
                .accept_receive_stream()
                .await
                .map(|stream| stream.map(MoqRecvStream::Quic))
                .map_err(io_error),
            Self::Qmux(connection) => connection
                .accept_unidirectional()
                .await
                .map(|stream| stream.map(MoqRecvStream::Qmux))
                .map_err(io_error),
        }
    }
}

/// ピア起動の双方向ストリーム (request stream) を受け入れる口
pub(crate) enum MoqBidiAcceptor {
    /// QUIC 直接接続の双方向ストリーム acceptor
    Quic(BidirectionalStreamAcceptor),
    /// ngtcp2 の双方向ストリーム acceptor
    Ngtcp2(ngtcp2::BidiAcceptor),
    /// WebTransport over HTTP/2 の双方向ストリーム acceptor
    WtH2(wt_h2::BidiAcceptor),
    /// QMux 接続 (accept は `&self` で待てる)
    Qmux(Arc<tokio_qmux::Connection>),
}

impl MoqBidiAcceptor {
    /// 双方向ストリームを 1 本受け入れる
    ///
    /// 戻り値の順序は `(受信, 送信)` に揃える。接続が閉じた場合は `Ok(None)` を返す。
    ///
    /// # Errors
    ///
    /// accept に失敗した場合はエラーになる。
    pub(crate) async fn accept_bidirectional_stream(
        &mut self,
    ) -> std::io::Result<Option<(MoqRecvStream, MoqSendStream)>> {
        match self {
            Self::Ngtcp2(acceptor) => acceptor.accept().await.map(|stream| {
                stream
                    .map(|(recv, send)| (MoqRecvStream::Ngtcp2(recv), MoqSendStream::Ngtcp2(send)))
            }),
            Self::WtH2(acceptor) => acceptor.accept().await.map(|stream| {
                stream.map(|(recv, send)| (MoqRecvStream::WtH2(recv), MoqSendStream::WtH2(send)))
            }),
            Self::Quic(acceptor) => acceptor
                .accept_bidirectional_stream()
                .await
                .map(|stream| {
                    stream.map(|stream| {
                        let (recv, send) = stream.split();
                        (MoqRecvStream::Quic(recv), MoqSendStream::Quic(send))
                    })
                })
                .map_err(io_error),
            Self::Qmux(connection) => connection
                .accept_bidirectional()
                .await
                .map(|stream| {
                    // tokio-qmux は (送信, 受信) の順で返す
                    stream
                        .map(|(send, recv)| (MoqRecvStream::Qmux(recv), MoqSendStream::Qmux(send)))
                })
                .map_err(io_error),
        }
    }
}

/// ピアから届く datagram の受け口
///
/// MOQT の OBJECT_DATAGRAM は QUIC DATAGRAM で運ばれる
/// (draft-ietf-moq-transport-22 §11.2 (Datagrams))。経路ごとに受信の仕組みが異なるため、
/// 駆動タスクのチャネル (ngtcp2 / WebTransport over HTTP/2) と s2n-quic の datagram
/// provider を enum で包む。QMux は接続が受信キューを持つため接続そのものを保持する。
pub(crate) enum MoqDatagramReceiver {
    /// QUIC 直接接続 (s2n-quic) の datagram 受信
    Quic(Handle),
    /// ngtcp2 (QUIC 直接接続 / WebTransport over HTTP/3) の駆動タスクが転送する datagram
    Ngtcp2(mpsc::UnboundedReceiver<Vec<u8>>),
    /// WebTransport over HTTP/2 の駆動タスクが転送する datagram
    WtH2(mpsc::UnboundedReceiver<Vec<u8>>),
    /// QMux 接続の datagram 受信
    Qmux(Arc<tokio_qmux::Connection>),
}

impl MoqDatagramReceiver {
    /// 到着済みの datagram をまとめて受け取る
    ///
    /// 1 件以上届くまで待ち、その時点で受け取れる分をすべてまとめて返す。返る `Vec` は
    /// 空にならない。接続が閉じた場合は `Ok(None)` を返す。
    ///
    /// # Errors
    ///
    /// 接続の datagram 受信に失敗した場合はエラーになる。
    pub(crate) async fn recv_datagrams(&mut self) -> std::io::Result<Option<Vec<Vec<u8>>>> {
        match self {
            Self::Quic(handle) => {
                let mut first: Option<Bytes> = None;
                let mut closed = false;
                // 1 つも無いときに即座に返すと呼び出し側の受信ループが空回りして CPU を
                // 使い切り、同じ runtime で動く QUIC エンドポイントの I/O を飢えさせる。
                // 最初の 1 つは届くまで待ち、続けて届いている分はまとめて取る
                std::future::poll_fn(|cx| {
                    let result = handle.datagram_mut(
                        |receiver: &mut s2n_quic::provider::datagram::default::Receiver| {
                            receiver.poll_recv_datagram(cx)
                        },
                    );
                    match result {
                        Ok(Poll::Ready(Ok(datagram))) => {
                            first = Some(datagram);
                            Poll::Ready(())
                        }
                        Ok(Poll::Pending) => Poll::Pending,
                        // 接続エラー。datagram はもう届かないため、接続の終了として扱う
                        Ok(Poll::Ready(Err(e))) => {
                            tracing::debug!("datagram receive ended: {e}");
                            closed = true;
                            Poll::Ready(())
                        }
                        Err(e) => {
                            tracing::debug!("datagram query ended: {e}");
                            closed = true;
                            Poll::Ready(())
                        }
                    }
                })
                .await;
                if closed {
                    return Ok(None);
                }
                let first = first.expect("logical invariant: a datagram is received");
                let mut datagrams = vec![first.to_vec()];
                // 続けて届いている分を取る (空になったら返す)
                let _ = handle.datagram_mut(
                    |receiver: &mut s2n_quic::provider::datagram::default::Receiver| {
                        while let Some(next) = receiver.recv_datagram() {
                            datagrams.push(next.to_vec());
                        }
                    },
                );
                Ok(Some(datagrams))
            }
            Self::Ngtcp2(receiver) | Self::WtH2(receiver) => {
                let Some(first) = receiver.recv().await else {
                    return Ok(None);
                };
                let mut datagrams = vec![first];
                while let Ok(next) = receiver.try_recv() {
                    datagrams.push(next);
                }
                Ok(Some(datagrams))
            }
            Self::Qmux(connection) => {
                // QMux は速やかに配送できない DATAGRAM の破棄を許す。取りこぼしを追えるよう
                // 破棄件数が増えていれば記録する
                let dropped = connection.dropped_datagrams();
                if dropped > 0 {
                    tracing::debug!("QMux dropped datagrams: {dropped}");
                }
                connection
                    .recv_datagrams()
                    .await
                    .map_err(|e| io_error(format!("QMux datagram receive error: {e}")))
            }
        }
    }
}

/// MOQT のエラーコードを s2n-quic の application error へ変換する
///
/// 可変長整数で表現できない値は既存の挙動に合わせて `UNKNOWN` にする。
fn application_error(error_code: u64) -> s2n_quic::application::Error {
    s2n_quic::application::Error::new(error_code).unwrap_or(s2n_quic::application::Error::UNKNOWN)
}

/// 経路ごとのエラーを `std::io::Error` へ包む
fn io_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shiguredo_qmux::codec::role::Role;

    /// 実際の QMux 接続 (TCP ではなくメモリ上の duplex) を client / server で 1 対作る
    ///
    /// モックやスタブではなく、qmux-rs の本物の状態機械を両端で動かす。TLS と TCP は
    /// 外部サーバが必要なため単体テストでは扱わず、ここではストリーム操作だけを検証する。
    fn qmux_pair() -> (Arc<tokio_qmux::Connection>, Arc<tokio_qmux::Connection>) {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let config = super::super::qmux::build_config();
        let client = tokio_qmux::Connection::new(Role::Client, client_io, config.clone())
            .expect("client 側の QMux 接続を作れること");
        let server = tokio_qmux::Connection::new(Role::Server, server_io, config)
            .expect("server 側の QMux 接続を作れること");
        (Arc::new(client), Arc::new(server))
    }

    /// 単方向ストリームの open / write / finish / read が往復すること
    #[tokio::test]
    async fn qmux_unidirectional_stream_round_trip() {
        let (client, server) = qmux_pair();
        let mut handle = MoqConnectionHandle::Qmux(client);
        let mut acceptor = MoqRecvAcceptor::Qmux(server);

        let payload = b"hello qmux";
        let mut send = handle
            .open_send_stream()
            .await
            .expect("単方向 stream を開けること");
        send.write_all(payload).await.expect("payload を書けること");
        send.finish().await.expect("FIN を送れること");

        let mut recv = acceptor
            .accept_receive_stream()
            .await
            .expect("単方向 stream を受け入れられること")
            .expect("stream が届いていること");
        let mut buf = vec![0u8; payload.len() + 16];
        let mut total = 0;
        loop {
            let size = recv.read(&mut buf[total..]).await.expect("読み込めること");
            if size == 0 {
                break;
            }
            total += size;
        }
        assert_eq!(&buf[..total], payload);
    }

    /// 双方向ストリームの open / write / read / finish が往復すること
    #[tokio::test]
    async fn qmux_bidirectional_stream_round_trip() {
        let (client, server) = qmux_pair();
        let mut handle = MoqConnectionHandle::Qmux(client);
        let mut acceptor = MoqBidiAcceptor::Qmux(server);

        // 受信を先に待たせ、open と write を並行させる
        let server_task = tokio::spawn(async move {
            let (mut recv, mut send) = acceptor
                .accept_bidirectional_stream()
                .await
                .expect("双方向 stream を受け入れられること")
                .expect("stream が届いていること");
            let mut buf = [0u8; 5];
            let mut filled = 0;
            while filled < buf.len() {
                let size = recv.read(&mut buf[filled..]).await.expect("読み込めること");
                if size == 0 {
                    break;
                }
                filled += size;
            }
            send.write_all(b"world").await.expect("応答を書けること");
            send.finish().await.expect("FIN を送れること");
            (buf, filled)
        });

        let (mut recv, mut send) = handle
            .open_bidirectional_stream()
            .await
            .expect("双方向 stream を開けること");
        send.write_all(b"hello")
            .await
            .expect("payload を書けること");
        let mut buf = [0u8; 5];
        let mut filled = 0;
        while filled < buf.len() {
            let size = recv
                .read(&mut buf[filled..])
                .await
                .expect("応答を読み込めること");
            if size == 0 {
                break;
            }
            filled += size;
        }
        assert_eq!(&buf, b"world");

        let (received, filled) = server_task.await.expect("server タスクが完了すること");
        assert_eq!(filled, 5);
        assert_eq!(&received, b"hello");
    }

    /// QMux の DATAGRAM が送受信できること
    ///
    /// QMux の DATAGRAM は QUIC の DATAGRAM と違い信頼的かつ順序どおりに配送される
    /// (draft-ietf-quic-qmux-02 Section 1.1)。接続の確立 (ピアの transport parameter の
    /// 交換) 前に送った DATAGRAM は破棄され得るため、受信できるまで再送して確かめる。
    #[tokio::test]
    async fn qmux_datagram_round_trip() {
        let (client, server) = qmux_pair();
        let handle = MoqConnectionHandle::Qmux(client);
        let mut receiver = MoqDatagramReceiver::Qmux(server);

        let payload = b"audio object";
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut received = Vec::new();
        while received.is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "DATAGRAM を受信できること"
            );
            handle
                .send_datagram(payload)
                .await
                .expect("DATAGRAM を送信できること");
            match tokio::time::timeout(
                std::time::Duration::from_millis(50),
                receiver.recv_datagrams(),
            )
            .await
            {
                Ok(Ok(Some(datagrams))) => received = datagrams,
                Ok(Ok(None)) => break,
                Ok(Err(e)) => panic!("DATAGRAM の受信に失敗しました: {e}"),
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        }
        assert!(
            received.iter().any(|datagram| datagram == payload),
            "送信した DATAGRAM が受信できること: {received:?}"
        );
    }

    /// s2n-quic の datagram が送受信できること
    ///
    /// s2n-quic の QUIC DATAGRAM は再送されない (RFC 9221 §3)。loopback の実 QUIC 接続で、
    /// datagram を送って反対側の受信口で取り出せることを確かめる。
    #[tokio::test(flavor = "multi_thread")]
    async fn quic_datagram_round_trip() {
        use s2n_quic::provider::datagram::default::Endpoint as DatagramEndpoint;

        // loopback 接続用の自己署名証明書
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("自己署名証明書を生成できること");
        let certificate = certified.cert.der().to_vec();
        let private_key = certified.signing_key.serialize_der();

        // s2n-quic の datagram endpoint は送受信のキューの容量が既定で 0 である。
        // 送信容量が 0 だと datagram を 1 つも送れないため、両方に容量を設定する
        let datagram_endpoint = || {
            DatagramEndpoint::builder()
                .with_send_capacity(64)
                .expect("datagram の送信キューを設定できること")
                .with_recv_capacity(64)
                .expect("datagram の受信キューを設定できること")
                .build()
                .expect("datagram endpoint を構築できること")
        };

        let server_tls = s2n_quic::provider::tls::rustls::Server::builder()
            .with_certificate(vec![certificate.clone()], private_key)
            .expect("サーバーの証明書と秘密鍵を設定できること")
            .with_application_protocols([b"moqt-22".as_slice()].into_iter())
            .expect("ALPN を設定できること")
            .build()
            .expect("サーバーの TLS 設定を構築できること");
        let mut server = s2n_quic::Server::builder()
            .with_tls(server_tls)
            .expect("サーバーの TLS を設定できること")
            .with_io("127.0.0.1:0")
            .expect("サーバーの UDP ソケットを作成できること")
            .with_datagram(datagram_endpoint())
            .expect("サーバーの datagram endpoint を設定できること")
            .start()
            .expect("サーバーを起動できること");
        let addr = server
            .local_addr()
            .expect("サーバーの待ち受けアドレスを取得できること");

        let server_task = tokio::spawn(async move {
            let connection = server.accept().await.expect("接続を受け入れられること");
            let (handle, _acceptor) = connection.split();
            let mut receiver = MoqDatagramReceiver::Quic(handle);
            receiver
                .recv_datagrams()
                .await
                .expect("datagram を受信できること")
                .expect("接続が閉じていないこと")
        });

        let client_tls = s2n_quic::provider::tls::rustls::Client::builder()
            .with_certificate(vec![certificate])
            .expect("サーバーの証明書を信頼できること")
            .with_application_protocols([b"moqt-22".as_slice()].into_iter())
            .expect("ALPN を設定できること")
            .build()
            .expect("クライアントの TLS 設定を構築できること");
        let client = s2n_quic::Client::builder()
            .with_tls(client_tls)
            .expect("クライアントの TLS を設定できること")
            .with_io("0.0.0.0:0")
            .expect("クライアントの UDP ソケットを作成できること")
            .with_datagram(datagram_endpoint())
            .expect("クライアントの datagram endpoint を設定できること")
            .start()
            .expect("クライアントを起動できること");
        let connection = client
            .connect(s2n_quic::client::Connect::new(addr).with_server_name("localhost"))
            .await
            .expect("MOQ relay 相当のサーバーへ接続できること");
        let (handle, _acceptor) = connection.split();
        let handle = MoqConnectionHandle::Quic(handle);

        let payload = b"audio object";
        handle
            .send_datagram(payload)
            .await
            .expect("datagram を送信できること");

        let received = tokio::time::timeout(std::time::Duration::from_secs(10), server_task)
            .await
            .expect("datagram の受信がタイムアウトしないこと")
            .expect("サーバータスクが完了すること");
        assert!(
            received.iter().any(|datagram| datagram == payload),
            "送信した datagram が受信できること: {received:?}"
        );
    }
}
