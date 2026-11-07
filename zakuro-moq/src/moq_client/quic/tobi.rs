//! tobi-quic を使った QUIC 直接接続
//!
//! MOQT の制御ストリーム・data stream・OBJECT_DATAGRAM は、このモジュールが
//! 構築する tobi_quic の接続に載せる。1 仮想クライアント = 1 接続であり、
//! UDP ソケットとエンドポイントタスクは接続先とアドレスファミリごとに共有する。

use std::net::SocketAddr;
use std::sync::Arc;

use tobi_quic::core::connection::ConnectionConfig;
use tobi_quic::core::transport_params::TransportParams;
use tobi_quic::{Client, ClientTlsConfig, Connection, DatagramConfig, IoConfig};

use super::super::transport::ALPN;
use crate::error::{ErrorMessage, Result};

/// 受信ウィンドウ (接続全体、バイト)
///
/// 既定値の 1 MiB では購読側の受信が詰まるため、負荷試験向けに 64 MiB にする。
const INITIAL_MAX_DATA: u64 = 64 * 1024 * 1024;

/// ストリーム 1 本あたりの受信ウィンドウ (バイト)
const INITIAL_MAX_STREAM_DATA: u64 = 16 * 1024 * 1024;

/// ピアが開ける単方向ストリームの上限
///
/// 既定値の 100 本では、relay が開けるストリームの累積上限に達して購読が止まる。
/// 負荷試験では購読側の本数も増えるため余裕を持たせる。
const INITIAL_MAX_STREAMS_UNI: u64 = 10_000;

/// ピアが開ける双方向ストリームの上限
const INITIAL_MAX_STREAMS_BIDI: u64 = 1_000;

/// keep-alive の送信間隔
///
/// MOQT には PING が無いため、Forward State 0 などで無通信になっても relay の
/// idle timeout で接続を切られないよう、アプリケーション層ではなく QUIC 層で
/// PING を送る。
const KEEP_ALIVE_INTERVAL: tobi_quic::core::time::Duration =
    tobi_quic::core::time::Duration::from_secs(5);

/// 指定したアドレスファミリ用のクライアントを構築する
///
/// 渡す TLS 設定は経路で共有する土台であり、ALPN はここで MOQT の `moqt-23` に
/// 設定する (WebTransport over HTTP/3 は同じ土台に `h3` を設定する)。
///
/// # Errors
///
/// クライアントの起動に失敗した場合はエラーになる。
pub(crate) fn build_client(tls: &Arc<rustls::ClientConfig>, is_ipv6: bool) -> Result<Client> {
    // ALPN は経路で異なるため、共有する設定を複製してから設定する
    let mut config = tls.as_ref().clone();
    config.alpn_protocols = vec![ALPN.to_vec()];
    let tls = ClientTlsConfig::new(Arc::new(config));
    let io = IoConfig::new(super::local_bind_addr(is_ipv6));
    Client::builder()
        .with_tls(tls)
        .with_io(io)
        // MOQT は object を datagram でも運ぶため、max_datagram_frame_size を広告する
        // (draft-ietf-moq-transport-23 §11.2)
        .with_datagram(DatagramConfig::default())
        .with_transport(build_transport_config())
        // 接続を操作していないタスクがある間も ACK とタイマーを処理する。
        // 受信待ちの future だけでは、複数タスクが同時に待つときに受信が
        // 遅れることがある
        .with_background_driver(true)
        .start()
        .map_err(|e| {
            ErrorMessage::new(format!("QUIC クライアントの起動に失敗しました: {e}")).into()
        })
}

/// QUIC の transport parameter とタイマー設定を構築する
fn build_transport_config() -> ConnectionConfig {
    ConnectionConfig {
        transport_params: TransportParams {
            initial_max_data: INITIAL_MAX_DATA,
            initial_max_stream_data_bidi_local: INITIAL_MAX_STREAM_DATA,
            initial_max_stream_data_bidi_remote: INITIAL_MAX_STREAM_DATA,
            initial_max_stream_data_uni: INITIAL_MAX_STREAM_DATA,
            initial_max_streams_bidi: INITIAL_MAX_STREAMS_BIDI,
            initial_max_streams_uni: INITIAL_MAX_STREAMS_UNI,
            ..Default::default()
        },
        // keep-alive は明示的に有効化する必要があるため、間隔もここで指定する。
        // 未指定だとローカルの idle timeout が 0 のとき PING を送らない
        keep_alive_interval: Some(KEEP_ALIVE_INTERVAL),
        ..Default::default()
    }
}

/// 1 つの接続先へ QUIC 接続を確立する
///
/// # Errors
///
/// ハンドシェイクに失敗した場合、または relay が MOQT の ALPN を選択しなかった
/// 場合はエラーになる。
pub(crate) async fn connect(
    client: &Client,
    remote: SocketAddr,
    server_name: &str,
) -> Result<Connection> {
    let connection = client
        .connect(tobi_quic::Connect::new(remote, server_name))
        .await
        .map_err(|e| ErrorMessage::new(format!("MOQ relay への QUIC 接続に失敗しました: {e}")))?;
    // ALPN が一致しないと MOQT ではなく別のプロトコルで接続してしまうため、
    // ハンドシェイク直後に確認する
    if connection.application_protocol().await.as_deref() != Some(ALPN) {
        return Err(ErrorMessage::new(format!(
            "MOQ relay が {} ALPN を選択しませんでした",
            String::from_utf8_lossy(ALPN)
        ))
        .into());
    }
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moq_client::stream::{MoqConnectionHandle, MoqDatagramReceiver, MoqRecvAcceptor};

    /// 負荷試験向けの受信ウィンドウとストリーム上限を広告すること
    ///
    /// 既定値のままだと relay が開けるストリームの累積上限に達して購読が止まる。
    #[test]
    fn transport_config_advertises_load_test_limits() {
        let config = build_transport_config();
        assert_eq!(config.transport_params.initial_max_data, INITIAL_MAX_DATA);
        assert_eq!(
            config.transport_params.initial_max_stream_data_uni,
            INITIAL_MAX_STREAM_DATA
        );
        assert_eq!(
            config.transport_params.initial_max_streams_uni,
            INITIAL_MAX_STREAMS_UNI
        );
        assert_eq!(
            config.transport_params.initial_max_streams_bidi,
            INITIAL_MAX_STREAMS_BIDI
        );
        // DATAGRAM の設定は `ClientBuilder::with_datagram` が start() 時に上書きする。
        // ここでは transport parameter の既定値 (未送信) のままである
        assert_eq!(config.transport_params.max_datagram_frame_size, None);
        // 無通信時に relay の idle timeout で切られないよう keep-alive を設定する
        assert_eq!(config.keep_alive_interval, Some(KEEP_ALIVE_INTERVAL));
    }

    /// loopback で tobi-quic のサーバーを起動する
    ///
    /// 自己署名証明書を生成し、ALPN `moqt-23` と QUIC DATAGRAM を有効にする。
    /// モックやスタブではなく、実際の QUIC ハンドシェイクを行うサーバーである。
    fn start_quic_server() -> (
        std::net::SocketAddr,
        tobi_quic::Server,
        rustls::pki_types::CertificateDer<'static>,
    ) {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("自己署名証明書を生成できること");
        let certificate = certified.cert.der().clone();
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let mut tls = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 を設定できること")
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], private_key)
        .expect("証明書と秘密鍵を設定できること");
        tls.alpn_protocols = vec![b"moqt-23".to_vec()];
        let server = tobi_quic::Server::builder()
            .with_tls(tobi_quic::ServerTlsConfig::new(std::sync::Arc::new(tls)))
            .with_io(tobi_quic::IoConfig::new(
                "127.0.0.1:0".parse().expect("アドレスを解釈できること"),
            ))
            .with_datagram(tobi_quic::DatagramConfig::default())
            .start()
            .expect("QUIC サーバーを起動できること");
        (server.local_addr(), server, certificate)
    }

    /// 起動済みのサーバーへ tobi-quic のクライアントで接続する
    async fn connect_quic(
        address: std::net::SocketAddr,
        certificate: rustls::pki_types::CertificateDer<'static>,
    ) -> tobi_quic::Connection {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(certificate)
            .expect("自己署名証明書を信頼ストアへ追加できること");
        let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 を設定できること")
        .with_root_certificates(roots)
        .with_no_client_auth();
        let client = build_client(&std::sync::Arc::new(tls), false)
            .expect("QUIC クライアントを構築できること");
        connect(&client, address, "localhost")
            .await
            .expect("MOQ relay 相当のサーバーへ接続できること")
    }

    /// tobi-quic の接続で単方向ストリームが往復すること
    ///
    /// MOQT の data stream は単方向ストリームで送る。実際の QUIC 接続で
    /// open / write / finish / accept / read が成立することを確かめる。
    #[tokio::test(flavor = "multi_thread")]
    async fn quic_unidirectional_stream_round_trip() {
        let (address, mut server, certificate) = start_quic_server();
        // サーバーの accept が QUIC のハンドシェイクを進めるため、クライアントの
        // 接続より先に待たせておく
        let server_task = tokio::spawn(async move {
            let connection = server.accept().await.expect("接続を受け入れられること");
            let (_, acceptor) = connection.split();
            let mut acceptor = MoqRecvAcceptor::Quic(acceptor);
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

        let connection = connect_quic(address, certificate).await;
        let (client_handle, _) = connection.split();
        let mut client = MoqConnectionHandle::Quic(client_handle);

        let payload = b"hello tobi-quic";
        let mut send = client
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

    /// tobi-quic の接続で DATAGRAM が送受信できること
    ///
    /// MOQT の OBJECT_DATAGRAM は QUIC DATAGRAM で運ぶ。loopback の実 QUIC 接続で、
    /// 送信した datagram が反対側の受信口で取り出せることを確かめる。
    #[tokio::test(flavor = "multi_thread")]
    async fn quic_datagram_round_trip() {
        let (address, mut server, certificate) = start_quic_server();
        // サーバーの accept が QUIC のハンドシェイクを進めるため、クライアントの
        // 接続より先に待たせておく
        let server_task = tokio::spawn(async move {
            let connection = server.accept().await.expect("接続を受け入れられること");
            let mut receiver = MoqDatagramReceiver::Quic(connection);
            receiver
                .recv_datagrams()
                .await
                .expect("datagram を受信できること")
                .expect("接続が閉じていないこと")
        });

        let connection = connect_quic(address, certificate).await;
        let (client_handle, _) = connection.split();
        let client = MoqConnectionHandle::Quic(client_handle);

        let payload = b"audio object";
        client
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
