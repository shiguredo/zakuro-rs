//! MOQT の I/O 層で使うストリームと接続ハンドルの抽象化
//!
//! `shiguredo_moqt::Session` は Sans I/O の状態機械であり、ワイヤの読み書きは呼び出し側の
//! 責務である。zakuro-moq は QUIC 直接接続 (s2n-quic) と QMux (tokio-qmux) の 2 経路を持ち、
//! `session.rs` は経路を意識せずにセッションを駆動したい。そこでストリーム・接続ハンドル・
//! accept を enum で包み、必要な操作だけを提供する。
//!
//! トレイトは作らない (時雨堂 Rust 規約)。経路が 2 つしかなく、どちらも同じ操作を提供する
//! ため、enum の match で十分である。

use std::sync::Arc;

use s2n_quic::connection::{BidirectionalStreamAcceptor, Handle, ReceiveStreamAcceptor};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::qmux;

/// 送信ストリーム
pub(crate) enum MoqSendStream {
    /// QUIC 直接接続の送信ストリーム
    Quic(s2n_quic::stream::SendStream),
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
            Self::Qmux(stream) => stream.finish().await.map_err(io_error),
        }
    }

    /// ストリームの送信方向を reset する
    ///
    /// `error_code` は MOQT のストリームリセットエラーコード
    /// (draft-ietf-moq-transport-22 §12.5) である。QMux は QUIC version 1 のフレームを
    /// 多重化するため、コード空間は native QUIC と共通である
    /// (draft-ietf-quic-qmux-02 Section 4)。
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
        }
    }
}

/// 受信ストリーム
pub(crate) enum MoqRecvStream {
    /// QUIC 直接接続の受信ストリーム
    Quic(s2n_quic::stream::ReceiveStream),
    /// QMux の受信ストリーム
    Qmux(tokio_qmux::RecvStream),
}

impl MoqRecvStream {
    /// ストリーム ID を返す
    pub(crate) fn id(&self) -> u64 {
        match self {
            Self::Quic(stream) => stream.id(),
            Self::Qmux(stream) => stream.id().as_u64(),
        }
    }

    /// 読み込んだバイト数を返す (0 は FIN)
    ///
    /// # Errors
    ///
    /// 読み込みに失敗した場合はエラーになる。QMux の RESET_STREAM もエラーとして扱う
    /// (`reliable_size` は RESET_STREAM_AT 未対応のため使わない)。
    pub(crate) async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Quic(stream) => stream.read(buf).await,
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
            Self::Qmux(stream) => f.debug_tuple("Qmux").field(stream).finish(),
        }
    }
}

/// 接続のハンドル
///
/// 複数タスクから clone して使う (ストリームの open と接続の close)。QUIC は s2n-quic の
/// `Handle` を、QMux は `Arc<tokio_qmux::Connection>` を持つ。
#[derive(Clone)]
pub(crate) enum MoqConnectionHandle {
    /// QUIC 直接接続のハンドル
    Quic(Handle),
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
    pub(crate) fn close(&self, error_code: u64) {
        match self {
            Self::Quic(handle) => handle.close(application_error(error_code)),
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
    /// QUIC は s2n-quic の keep-alive を有効にする。QMux には keep-alive の API が無いため、
    /// QX_PING を定期的に送るタスクを起動する ([`qmux::spawn_keep_alive`])。
    ///
    /// # Errors
    ///
    /// QUIC の keep-alive の有効化に失敗した場合はエラーになる。QMux の PING タスクの起動は
    /// 失敗しない (タスクは接続終了で自ら止まる)。
    pub(crate) fn keep_alive(&mut self, token: &CancellationToken) -> std::io::Result<()> {
        match self {
            Self::Quic(handle) => handle.keep_alive(true).map_err(io_error),
            Self::Qmux(connection) => {
                qmux::spawn_keep_alive(connection, token);
                Ok(())
            }
        }
    }
}

/// ピア起動の単方向ストリーム (data stream) を受け入れる口
pub(crate) enum MoqRecvAcceptor {
    /// QUIC 直接接続の単方向ストリーム acceptor
    Quic(ReceiveStreamAcceptor),
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
}
