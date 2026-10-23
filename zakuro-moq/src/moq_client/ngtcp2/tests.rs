//! 実 UDP / TLS / QUIC 接続でバックエンドのストリーム操作を検証する

use super::super::transport::{
    MoqClientContext, MoqConnection, MoqEndpoint, MoqTransport, QuicBackend,
};
use super::*;
use shiguredo_ngtcp2_tokio::{AcceptedConnection, Server, ServerConfig};
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 自己署名証明書を信頼する実接続を作る
///
/// 証明書と鍵は一時ディレクトリに生成し、ハンドシェイク完了後に破棄する。
async fn pair(
    backend: QuicBackend,
    params: TransportParams,
) -> (MoqConnection, AcceptedConnection) {
    pair_with_client_params(backend, params, None).await
}

/// 受信枠の更新を少量のデータで検証できるよう、クライアント側の枠も指定する
async fn pair_with_client_params(
    backend: QuicBackend,
    params: TransportParams,
    client_params: Option<TransportParams>,
) -> (MoqConnection, AcceptedConnection) {
    let directory = tempfile::tempdir().expect("証明書用ディレクトリを作れること");
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
        .expect("証明書を生成できること");
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).expect("証明書を書けること");
    std::fs::write(&key_path, cert.signing_key.serialize_pem()).expect("鍵を書けること");
    let mut server = Server::bind(
        "127.0.0.1:0".parse().expect("アドレスが有効であること"),
        &cert_path,
        &key_path,
        Some(ServerConfig::new(&[ALPN]).with_transport_params(params)),
    )
    .await
    .expect("QUIC サーバーを起動できること");
    let endpoint = MoqEndpoint::parse(&format!("moqt://127.0.0.1:{}", server.local_addr().port()))
        .expect("接続先を解釈できること");
    let tls = MoqTlsOptions {
        insecure: false,
        ca_cert: Some(cert_path.to_string_lossy().into_owned()),
    };
    let context = MoqClientContext::new(&endpoint, &tls, MoqTransport::Quic, backend)
        .await
        .expect("接続設定を作れること");
    let addr = server.local_addr();
    let (client, server) = timeout(TEST_TIMEOUT, async {
        let connect_client = async {
            if let Some(params) = client_params {
                let config = build_config(&tls)?.with_transport_params(params);
                connect(
                    addr,
                    "127.0.0.1:0".parse().expect("アドレスが有効であること"),
                    "localhost",
                    &config,
                )
                .await
                .map(MoqConnection::Ngtcp2)
            } else {
                context.connect_to(&endpoint).await
            }
        };
        tokio::join!(connect_client, server.accept())
    })
    .await
    .expect("ハンドシェイクが完了すること");
    let client = client.expect("CA 証明書を検証して接続できること");
    assert_eq!(
        matches!(&client, MoqConnection::Ngtcp2(_)),
        backend == QuicBackend::Ngtcp2,
        "指定した実装を使うこと"
    );
    (
        client,
        server
            .expect("accept が成功すること")
            .expect("接続が届くこと"),
    )
}

/// 受信を続けながらストリームのデータと FIN を集める
async fn receive_stream(server: &mut AcceptedConnection, expected_id: i64) -> Vec<u8> {
    let mut received = Vec::new();
    loop {
        if let ConnectionEvent::StreamData {
            stream_id,
            data,
            fin,
        } = server.recv_event().await.expect("イベントを受信できること")
        {
            assert_eq!(stream_id, expected_id, "対象ストリームのデータであること");
            server
                .extend_max_stream_offset(stream_id, data.len() as u64)
                .expect("受信枠を戻せること");
            received.extend_from_slice(&data);
            if fin {
                return received;
            }
        }
    }
}

/// 両実装で単方向 / 双方向の送信と FIN、およびピア起動ストリームを扱えること
#[tokio::test]
async fn backends_stream_round_trip() {
    for backend in [QuicBackend::S2nQuic, QuicBackend::Ngtcp2] {
        timeout(TEST_TIMEOUT, async {
            let (client, mut server) = pair(backend, TransportParams::new()).await;
            let (mut handle, mut uni, mut bidi) = client.split();
            let peer = tokio::spawn(async move {
                assert_eq!(receive_stream(&mut server, 2).await, b"hello");
                let id = server
                    .open_uni_stream()
                    .expect("単方向 stream を開けること");
                server
                    .write_stream(id, b"world", true)
                    .expect("応答を書けること");
                let id = server
                    .open_bidi_stream()
                    .expect("双方向 stream を開けること");
                server
                    .write_stream(id, b"request", true)
                    .expect("要求を書けること");
                server.flush().await.expect("応答と要求を送れること");
                assert_eq!(receive_stream(&mut server, id).await, b"reply");
                assert_eq!(receive_stream(&mut server, 0).await, b"ping");
                server
                    .write_stream(0, b"pong", true)
                    .expect("双方向の応答を書けること");
                server.flush().await.expect("応答を送れること");
                // クライアントの read が完了するまで接続を維持し、ACK も処理する。
                while !matches!(
                    server.recv_event().await,
                    Ok(ConnectionEvent::ConnectionClosed { .. }) | Err(_)
                ) {}
            });
            let mut send = handle
                .open_send_stream()
                .await
                .expect("単方向 stream を開けること");
            send.write_all(b"hello").await.expect("送信できること");
            send.finish().await.expect("FIN を送れること");
            let mut recv = uni
                .accept_receive_stream()
                .await
                .expect("accept が成功すること")
                .expect("stream が届くこと");
            let mut data = Vec::new();
            let mut buf = [0; 3];
            loop {
                let size = recv.read(&mut buf).await.expect("単方向の応答を読めること");
                if size == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..size]);
            }
            assert_eq!(data, b"world");
            let (mut recv, mut send) = bidi
                .accept_bidirectional_stream()
                .await
                .expect("accept が成功すること")
                .expect("要求が届くこと");
            data.clear();
            loop {
                let size = recv.read(&mut buf).await.expect("双方向の要求を読めること");
                if size == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..size]);
            }
            assert_eq!(data, b"request");
            send.write_all(b"reply").await.expect("応答を送れること");
            send.finish().await.expect("応答の FIN を送れること");
            let (mut recv, mut send) = handle
                .open_bidirectional_stream()
                .await
                .expect("双方向 stream を開けること");
            send.write_all(b"ping").await.expect("要求を送れること");
            send.finish().await.expect("要求の FIN を送れること");
            data.clear();
            loop {
                let size = recv.read(&mut buf).await.expect("双方向の応答を読めること");
                if size == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..size]);
            }
            assert_eq!(data, b"pong");
            handle.close(0).await;
            peer.await.expect("サーバーが終了すること");
        })
        .await
        .expect("ストリームの往復が完了すること");
    }
}

/// reliable reset と通常の RESET_STREAM へのフォールバックを実接続で検証する
#[tokio::test]
async fn reliable_reset_and_fallback() {
    for supports_reset in [true, false] {
        timeout(TEST_TIMEOUT, async {
            let (client, mut server) = pair(
                QuicBackend::Ngtcp2,
                TransportParams::new().with_reset_stream_at(supports_reset),
            )
            .await;
            assert!(
                server.supports_reset_stream_at(),
                "クライアントが RESET_STREAM_AT を広告すること"
            );
            let (mut handle, _, _) = client.split();
            let peer = tokio::spawn(async move {
                let mut data = Vec::new();
                loop {
                    match server.recv_event().await.expect("リセットを受信できること") {
                        ConnectionEvent::StreamData { data: chunk, .. } => {
                            data.extend_from_slice(&chunk)
                        }
                        ConnectionEvent::StreamReset { app_error_code, .. } => {
                            assert_eq!(app_error_code, 42, "reset のコードが一致すること");
                            if supports_reset {
                                assert_eq!(data, b"partial", "送信済みのデータを欠落させないこと");
                            }
                            return;
                        }
                        _ => {}
                    }
                }
            });
            let mut send = handle
                .open_send_stream()
                .await
                .expect("stream を開けること");
            send.write_all(b"partial")
                .await
                .expect("データを送れること");
            send.reset(42).await.expect("reset できること");
            peer.await.expect("リセットの検証が完了すること");
            handle.close(0).await;
        })
        .await
        .expect("reset の検証が完了すること");
    }
}

/// 小さい受信枠を何度も更新する送受信と、ストリーム数の枠の更新を検証する
#[tokio::test]
async fn flow_control_and_stream_credit() {
    timeout(TEST_TIMEOUT, async {
        let params = TransportParams::new()
            .with_max_streams_uni(1)
            .with_initial_max_data(1024)
            .with_initial_max_stream_data_uni(1024);
        let (client, mut server) = pair(QuicBackend::Ngtcp2, params).await;
        let (mut handle, _, _) = client.split();
        let peer = tokio::spawn(async move {
            for index in 0..3 {
                let data = receive_stream(&mut server, 2 + index * 4).await;
                assert_eq!(
                    data,
                    vec![7; WRITE_CHUNK_SIZE * 3],
                    "受信枠を超えるデータが届くこと"
                );
                // ngtcp2 は MAX_STREAMS を自動更新しないため、受信処理後に枠を返す。
                server.extend_max_streams_uni(1);
                server
                    .flush()
                    .await
                    .expect("ストリーム枠の更新を送れること");
            }
            while !matches!(
                server.recv_event().await,
                Ok(ConnectionEvent::ConnectionClosed { .. }) | Err(_)
            ) {}
        });
        for _ in 0..3 {
            let mut send = handle
                .open_send_stream()
                .await
                .expect("空いた枠で stream を開けること");
            send.write_all(&vec![7; WRITE_CHUNK_SIZE * 3])
                .await
                .expect("受信枠を超えるデータを送れること");
            send.finish().await.expect("FIN を送れること");
        }
        handle.close(0).await;
        peer.await.expect("サーバーが終了すること");
    })
    .await
    .expect("フロー制御で送受信が停止しないこと");
}

/// ngtcp2 の受信キューは reset 前に届いたデータを読み切ってからコードを通知すること
#[tokio::test]
async fn receive_reset_and_connection_close() {
    timeout(TEST_TIMEOUT, async {
        let (client, mut server) = pair(
            QuicBackend::Ngtcp2,
            TransportParams::new().with_reset_stream_at(true),
        )
        .await;
        let (mut handle, mut uni, _) = client.split();
        let peer = tokio::spawn(async move {
            let id = server.open_uni_stream().expect("stream を開けること");
            server
                .write_stream(id, b"partial", false)
                .expect("データを書けること");
            server.flush().await.expect("データを送れること");
            server
                .reset_stream_reliable(id, 9)
                .expect("reliable reset できること");
            server.flush().await.expect("リセットを送れること");
            while !matches!(
                server.recv_event().await,
                Ok(ConnectionEvent::ConnectionClosed { .. }) | Err(_)
            ) {}
        });
        let mut recv = uni
            .accept_receive_stream()
            .await
            .expect("accept が成功すること")
            .expect("stream が届くこと");
        let mut data = Vec::new();
        let mut buf = [0; 3];
        loop {
            match recv.read(&mut buf).await {
                Ok(size) => {
                    assert!(size > 0, "reset を FIN に置き換えないこと");
                    data.extend_from_slice(&buf[..size]);
                }
                Err(error) => {
                    assert_eq!(
                        error
                            .get_ref()
                            .and_then(|e| e.downcast_ref::<StreamReset>())
                            .expect("reset の型を保つこと")
                            .0,
                        9
                    );
                    break;
                }
            }
        }
        assert_eq!(data, b"partial");
        // stream が届かない accept も接続終了によって解除されることを確認する。
        handle.close(0).await;
        assert!(
            uni.accept_receive_stream()
                .await
                .expect("accept が終了すること")
                .is_none()
        );
        assert!(
            handle.open_send_stream().await.is_err(),
            "接続終了後の open は失敗すること"
        );
        peer.await.expect("サーバーが終了すること");
    })
    .await
    .expect("受信 reset と接続終了を通知できること");
}

/// 読み捨てと通常の read の両方で受信枠とピア起動ストリーム数の枠が戻ること
#[tokio::test]
async fn receive_flow_control_and_dropped_stream() {
    timeout(TEST_TIMEOUT, async {
        let params = TransportParams::new()
            .with_max_streams_uni(1)
            .with_initial_max_data(1024)
            .with_initial_max_stream_data_uni(1024);
        let (client, mut server) =
            pair_with_client_params(QuicBackend::Ngtcp2, TransportParams::new(), Some(params))
                .await;
        let (handle, mut uni, _) = client.split();
        let peer = tokio::spawn(async move {
            for index in 0..3 {
                // 初期枠が 1 本のため、次のストリームは MAX_STREAMS を受け取るまで待つ。
                while server.streams_uni_left() == 0 {
                    server
                        .recv_event()
                        .await
                        .expect("ストリーム数の枠を受信できること");
                }
                let id = server.open_uni_stream().expect("stream を開けること");
                server
                    .write_stream(id, &vec![5; WRITE_CHUNK_SIZE * 2], true)
                    .expect("データを書けること");
                server.flush().await.expect("データを送れること");
                loop {
                    match server
                        .recv_event()
                        .await
                        .expect("ストリームの終端を確認できること")
                    {
                        ConnectionEvent::StreamStopSending { stream_id, .. } => {
                            assert_eq!(index, 0, "先頭のストリームだけが放棄されること");
                            assert_eq!(stream_id, id);
                        }
                        ConnectionEvent::StreamClosed { stream_id, .. } if stream_id == id => break,
                        _ => {}
                    }
                }
            }
            while !matches!(
                server.recv_event().await,
                Ok(ConnectionEvent::ConnectionClosed { .. }) | Err(_)
            ) {}
        });
        let abandoned = uni
            .accept_receive_stream()
            .await
            .expect("accept が成功すること")
            .expect("stream が届くこと");
        drop(abandoned);
        for _ in 0..2 {
            let mut recv = uni
                .accept_receive_stream()
                .await
                .expect("accept が成功すること")
                .expect("次の stream が届くこと");
            let mut data = Vec::new();
            let mut buf = [0; 511];
            loop {
                let size = recv
                    .read(&mut buf)
                    .await
                    .expect("受信枠を更新しながら読めること");
                if size == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..size]);
            }
            assert_eq!(data, vec![5; WRITE_CHUNK_SIZE * 2]);
        }
        handle.close(0).await;
        peer.await.expect("サーバーが終了すること");
    })
    .await
    .expect("放棄後も受信枠を回復して読み続けられること");
}
