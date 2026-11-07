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

use super::transport::ALPN;
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

/// アドレスファミリに合わせたローカルバインドアドレスを返す
///
/// 1 つの UDP ソケットは別のアドレスファミリ宛に送信できないため、接続先の
/// 名前解決結果に含まれるファミリごとにクライアントを作る。
pub(crate) fn local_bind_addr(is_ipv6: bool) -> SocketAddr {
    if is_ipv6 {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0))
    } else {
        SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0))
    }
}

/// 指定したアドレスファミリ用のクライアントを構築する
///
/// 渡す TLS 設定は経路で共有する土台であり、ALPN はここで MOQT の `moqt-22` に
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
    let io = IoConfig::new(local_bind_addr(is_ipv6));
    Client::builder()
        .with_tls(tls)
        .with_io(io)
        // MOQT は object を datagram でも運ぶため、max_datagram_frame_size を広告する
        // (draft-ietf-moq-transport-22 §11.2)
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
        return Err(ErrorMessage::new("MOQ relay が moqt-22 ALPN を選択しませんでした").into());
    }
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// アドレスファミリに合わせてローカルアドレスを選ぶこと
    #[test]
    fn local_bind_addr_matches_address_family() {
        assert_eq!(local_bind_addr(false).to_string(), "0.0.0.0:0");
        assert_eq!(local_bind_addr(true).to_string(), "[::]:0");
    }

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
}
