//! s2n-quic による QUIC 直接接続 (quic-s2n)
//!
//! MOQT の制御ストリーム・data stream・OBJECT_DATAGRAM は、このモジュールが
//! 構築する s2n-quic の接続に載せる。1 仮想クライアント = 1 接続であり、
//! UDP ソケットとエンドポイントタスクは接続先とアドレスファミリごとに共有する。

use std::net::SocketAddr;

use s2n_quic::Client;
use s2n_quic::client::Connect;
use s2n_quic::provider::tls::rustls as s2n_rustls;

use super::super::transport::{ALPN, MoqTlsOptions, build_rustls_config};
use crate::error::{ErrorMessage, Result};

/// s2n-quic の TLS クライアント
pub(crate) type Tls = s2n_rustls::Client;

/// QUIC 直接接続用の TLS クライアントを構築する
///
/// # Errors
///
/// TLS 設定の構築に失敗した場合はエラーになる。
pub(crate) fn build_tls(options: &MoqTlsOptions) -> Result<Tls> {
    let mut config = build_rustls_config(options)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(s2n_rustls::Client::from(config))
}

/// 指定したアドレスファミリ用のクライアントを構築する
///
/// # Errors
///
/// TLS 設定、UDP ソケットの作成、limits の設定、クライアントの起動に失敗した場合は
/// エラーになる。
pub(crate) fn build_client(tls: &Tls, is_ipv6: bool) -> Result<Client> {
    // s2n-quic の datagram endpoint は送受信のキューの容量が既定でどちらも 0 である。
    // 送信容量が 0 のままだと `poll_send_datagram` が空きを待ち続けて datagram を 1 つも
    // 送れないため、送信にも容量を設定する
    let datagram_endpoint = s2n_quic::provider::datagram::default::Endpoint::builder()
        .with_send_capacity(64)
        .map_err(|e| ErrorMessage::new(format!("datagram endpoint の構築に失敗しました: {e}")))?
        .with_recv_capacity(64)
        .map_err(|e| ErrorMessage::new(format!("datagram endpoint の構築に失敗しました: {e}")))?
        .build()
        .expect("datagram endpoint must build after capacities are set");

    Client::builder()
        .with_tls(tls.clone())
        .map_err(|e| ErrorMessage::new(format!("TLS 設定に失敗しました: {e}")))?
        .with_io(super::local_bind_addr(is_ipv6))
        .map_err(|e| ErrorMessage::new(format!("UDP ソケットの作成に失敗しました: {e}")))?
        .with_datagram(datagram_endpoint)
        .map_err(|e| ErrorMessage::new(format!("datagram provider の設定に失敗しました: {e}")))?
        .with_limits(build_limits()?)
        .map_err(|e| ErrorMessage::new(format!("limits provider の設定に失敗しました: {e}")))?
        .start()
        .map_err(|e| {
            ErrorMessage::new(format!("QUIC クライアントの起動に失敗しました: {e}")).into()
        })
}

/// QUIC の接続 limits を構築する
///
/// 既定値 (remote uni stream 100 本) のままだと、relay が開けるストリームの累積上限に
/// 達して購読が止まる。負荷試験では購読側の本数も増えるため余裕を持たせる。
///
/// # Errors
///
/// いずれかの limits 値が s2n-quic の許容範囲外の場合はエラーになる。
fn build_limits() -> Result<s2n_quic::provider::limits::Limits> {
    let limits = s2n_quic::provider::limits::Limits::new()
        .with_max_open_remote_unidirectional_streams(10_000)
        .map_err(|e| {
            ErrorMessage::new(format!("remote uni stream limit の設定に失敗しました: {e}"))
        })?
        .with_max_open_remote_bidirectional_streams(1_000)
        .map_err(|e| {
            ErrorMessage::new(format!(
                "remote bidi stream limit の設定に失敗しました: {e}"
            ))
        })?
        .with_data_window(64 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("data window の設定に失敗しました: {e}")))?
        .with_unidirectional_data_window(16 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("uni stream window の設定に失敗しました: {e}")))?
        .with_bidirectional_local_data_window(16 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("bidi local window の設定に失敗しました: {e}")))?
        .with_bidirectional_remote_data_window(16 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("bidi remote window の設定に失敗しました: {e}")))?;
    Ok(limits)
}

/// 1 つの接続先へ QUIC 接続を確立する
///
/// # Errors
///
/// ハンドシェイクに失敗した場合はエラーになる。
pub(crate) async fn connect(
    client: &Client,
    remote: SocketAddr,
    server_name: &str,
) -> Result<s2n_quic::connection::Connection> {
    client
        .connect(Connect::new(remote).with_server_name(server_name))
        .await
        .map_err(|e| {
            ErrorMessage::new(format!("MOQ relay への QUIC 接続に失敗しました: {e}")).into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::moq_client::stream::{
        MoqConnectionHandle, MoqDatagramReceiver, MoqRecvAcceptor, MoqSendStream,
    };

    /// loopback で s2n-quic のサーバーを起動する
    ///
    /// 自己署名証明書を生成し、ALPN と QUIC DATAGRAM を有効にする。モックや
    /// スタブではなく、実際の QUIC ハンドシェイクを行うサーバーである。
    fn start_server() -> (
        std::net::SocketAddr,
        s2n_quic::Server,
        rustls::pki_types::CertificateDer<'static>,
    ) {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("自己署名証明書を生成できること");
        let certificate = certified.cert.der().to_vec();
        let private_key = certified.signing_key.serialize_der();

        let datagram_endpoint = || {
            s2n_quic::provider::datagram::default::Endpoint::builder()
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
            .with_application_protocols([ALPN].into_iter())
            .expect("ALPN を設定できること")
            .build()
            .expect("サーバーの TLS 設定を構築できること");
        let server = s2n_quic::Server::builder()
            .with_tls(server_tls)
            .expect("サーバーの TLS を設定できること")
            .with_io("127.0.0.1:0")
            .expect("サーバーの UDP ソケットを作成できること")
            .with_datagram(datagram_endpoint())
            .expect("サーバーの datagram endpoint を設定できること")
            .start()
            .expect("サーバーを起動できること");
        let address = server
            .local_addr()
            .expect("サーバーの待ち受けアドレスを取得できること");
        (address, server, certified.cert.der().clone())
    }

    /// 起動済みのサーバーへ s2n-quic のクライアントで接続する
    async fn connect_to(
        address: std::net::SocketAddr,
        certificate: rustls::pki_types::CertificateDer<'static>,
    ) -> s2n_quic::connection::Connection {
        let client_tls = s2n_quic::provider::tls::rustls::Client::builder()
            .with_certificate(vec![certificate.to_vec()])
            .expect("サーバーの証明書を信頼できること")
            .with_application_protocols([ALPN].into_iter())
            .expect("ALPN を設定できること")
            .build()
            .expect("クライアントの TLS 設定を構築できること");
        let client = s2n_quic::Client::builder()
            .with_tls(client_tls)
            .expect("クライアントの TLS を設定できること")
            .with_io("0.0.0.0:0")
            .expect("クライアントの UDP ソケットを作成できること")
            .with_datagram(
                s2n_quic::provider::datagram::default::Endpoint::builder()
                    .with_send_capacity(64)
                    .expect("datagram の送信キューを設定できること")
                    .with_recv_capacity(64)
                    .expect("datagram の受信キューを設定できること")
                    .build()
                    .expect("datagram endpoint を構築できること"),
            )
            .expect("クライアントの datagram endpoint を設定できること")
            .start()
            .expect("クライアントを起動できること");
        client
            .connect(s2n_quic::client::Connect::new(address).with_server_name("localhost"))
            .await
            .expect("MOQ relay 相当のサーバーへ接続できること")
    }

    /// s2n-quic の接続で単方向ストリームが往復すること
    #[tokio::test(flavor = "multi_thread")]
    async fn stream_round_trip() {
        let (address, mut server, certificate) = start_server();
        let server_task = tokio::spawn(async move {
            let connection = server.accept().await.expect("接続を受け入れられること");
            let (_, acceptor) = connection.split();
            let (_, recv) = acceptor.split();
            let mut acceptor = MoqRecvAcceptor::S2n(recv);
            let mut stream = acceptor
                .accept_receive_stream()
                .await
                .expect("単方向 stream を受け入れられること")
                .expect("stream が届いていること");
            let mut buf = vec![0u8; 64];
            let mut total = 0;
            loop {
                let size = stream
                    .read(&mut buf[total..])
                    .await
                    .expect("読み込めること");
                if size == 0 {
                    break;
                }
                total += size;
            }
            buf.truncate(total);
            buf
        });

        let connection = connect_to(address, certificate).await;
        let (handle, _) = connection.split();
        let mut client = MoqConnectionHandle::S2n(handle);

        let payload = b"hello s2n-quic";
        let mut send = client
            .open_send_stream()
            .await
            .expect("単方向 stream を開けること");
        assert!(
            matches!(send, MoqSendStream::S2n(_)),
            "s2n の variant を使うこと"
        );
        send.write_all(payload).await.expect("payload を書けること");
        send.finish().await.expect("FIN を送れること");

        let received = tokio::time::timeout(Duration::from_secs(10), server_task)
            .await
            .expect("stream の受信がタイムアウトしないこと")
            .expect("サーバータスクが完了すること");
        assert_eq!(received, payload);
    }

    /// s2n-quic の接続で datagram が送受信できること
    ///
    /// s2n-quic の QUIC DATAGRAM は再送されない (RFC 9221 §3)。loopback の実 QUIC 接続で、
    /// datagram を送って反対側の受信口で取り出せることを確かめる。
    #[tokio::test(flavor = "multi_thread")]
    async fn datagram_round_trip() {
        let (address, mut server, certificate) = start_server();
        let server_task = tokio::spawn(async move {
            let connection = server.accept().await.expect("接続を受け入れられること");
            let (handle, _) = connection.split();
            let mut receiver = MoqDatagramReceiver::S2n(handle);
            receiver
                .recv_datagrams()
                .await
                .expect("datagram を受信できること")
                .expect("接続が閉じていないこと")
        });

        let connection = connect_to(address, certificate).await;
        let (handle, _) = connection.split();
        let client = MoqConnectionHandle::S2n(handle);

        let payload = b"audio object";
        client
            .send_datagram(payload)
            .await
            .expect("datagram を送信できること");

        let received = tokio::time::timeout(Duration::from_secs(10), server_task)
            .await
            .expect("datagram の受信がタイムアウトしないこと")
            .expect("サーバータスクが完了すること");
        assert!(
            received.iter().any(|datagram| datagram == payload),
            "送信した datagram が受信できること: {received:?}"
        );
    }
}
