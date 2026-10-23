//! 実 UDP / TLS / QUIC / HTTP/3 接続による WebTransport 結線の検証

use super::super::RecvStream;
use super::*;
use shiguredo_http3::ServerConnection as H3Server;
use shiguredo_http3::webtransport::ConnectResponse;
use shiguredo_ngtcp2_tokio::{AcceptedConnection, Server, ServerConfig, TransportParams};
use tokio::sync::oneshot;
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(15);

/// テスト相手も実際の QUIC と HTTP/3 状態機械を使い、パケットを交換する
struct Peer {
    quic: AcceptedConnection,
    h3: H3Server,
    events: VecDeque<WtEvent>,
}

impl Peer {
    async fn flush(&mut self) {
        for capsule in self.h3.take_wt_pending_capsules(0) {
            self.capsule(capsule);
        }
        let ids: Vec<_> = self.h3.writable_streams().collect();
        for id in ids {
            while let Some((data, fin)) = self.h3.take_stream_data(id) {
                self.quic
                    .write_stream(id as i64, &data, fin)
                    .expect("HTTP/3 データを送れること");
            }
        }
        self.quic.flush().await.expect("QUIC を駆動できること");
    }

    fn capsule(&mut self, capsule: Capsule) {
        let mut data = Vec::new();
        capsule.encode(&mut data);
        self.h3
            .send_body(0, &data, false)
            .expect("カプセルを送れること");
    }

    async fn next(&mut self) -> WtEvent {
        loop {
            self.flush().await;
            if let Some(event) = self.events.pop_front() {
                return event;
            }
            while let Some(event) = self
                .h3
                .poll_event()
                .expect("HTTP/3 イベントを取り出せること")
            {
                if let Event::WebTransport(event) = event {
                    if let WtEvent::BidiStreamData { data, .. }
                    | WtEvent::UniStreamData { data, .. } = &event
                    {
                        self.h3.wt_data_consumed(0, data.len() as u64);
                    }
                    self.events.push_back(event);
                }
            }
            if !self.events.is_empty() {
                continue;
            }
            match self
                .quic
                .recv_event()
                .await
                .expect("QUIC イベントを受信できること")
            {
                ConnectionEvent::StreamData {
                    stream_id,
                    data,
                    fin,
                } => {
                    self.h3
                        .feed_stream(stream_id as u64, &data, fin)
                        .expect("HTTP/3 へ受信を渡せること");
                    self.quic
                        .extend_max_stream_offset(stream_id, data.len() as u64)
                        .expect("QUIC の受信枠を返せること");
                }
                ConnectionEvent::StreamReset {
                    stream_id,
                    app_error_code,
                    final_size,
                } => {
                    self.h3
                        .stream_reset(stream_id as u64, app_error_code, final_size)
                        .expect("reset を通知できること");
                }
                ConnectionEvent::StreamStopSending {
                    stream_id,
                    app_error_code,
                } => {
                    self.h3
                        .stop_sending(stream_id as u64, app_error_code)
                        .expect("STOP_SENDING を通知できること");
                }
                ConnectionEvent::StreamClosed { stream_id, .. } if stream_id & 1 == 0 => {
                    if stream_id & 2 == 0 {
                        self.quic.extend_max_streams_bidi(1);
                    } else {
                        self.quic.extend_max_streams_uni(1);
                    }
                }
                ConnectionEvent::ConnectionClosed { error_code, .. } => {
                    panic!("WT イベントの前に接続が閉じた: {error_code:#x}")
                }
                _ => {}
            }
        }
    }

    fn open(&mut self, bidi: bool, data: &[u8], fin: bool) -> i64 {
        let id = if bidi {
            self.quic.open_bidi_stream()
        } else {
            self.quic.open_uni_stream()
        }
        .expect("ピア開始ストリームを開けること");
        self.h3
            .register_local_wt_stream(0, id as u64)
            .expect("WT ストリームを登録できること");
        let header = StreamHeader::new(0).expect("セッション ID が有効であること");
        let mut bytes = Vec::new();
        if bidi {
            header.encode_bidirectional(&mut bytes);
        } else {
            header.encode_unidirectional(&mut bytes);
        }
        bytes.extend_from_slice(data);
        self.quic
            .write_stream(id, &bytes, fin)
            .expect("WT ヘッダーと本文を送れること");
        id
    }

    async fn body(&mut self, bidi: bool) -> (u64, Vec<u8>) {
        let mut body = Vec::new();
        let mut id = None;
        loop {
            match self.next().await {
                WtEvent::UniStreamOpen { stream_id, .. } if !bidi => id = Some(stream_id),
                WtEvent::BidiStreamOpen { stream_id, .. } if bidi => id = Some(stream_id),
                WtEvent::UniStreamData { data, .. } | WtEvent::BidiStreamData { data, .. } => {
                    body.extend(data)
                }
                WtEvent::UniStreamEnd { stream_id } | WtEvent::BidiStreamEnd { stream_id } => {
                    assert_eq!(id, Some(stream_id), "対象のストリームであること");
                    return (stream_id, body);
                }
                _ => {}
            }
        }
    }
}

/// 小さな WT 枠を広告する実サーバーと、CA を検証するクライアントを接続する
async fn pair(status: u16) -> (Result<Connection>, Peer) {
    pair_with_name(status, "localhost").await
}

/// IP SAN の照合も実際の TLS 接続で確認する
async fn pair_with_name(status: u16, server_name: &str) -> (Result<Connection>, Peer) {
    pair_with_response(status, server_name, Some("moqt-22")).await
}

/// 不正なサブプロトコル応答も実際の HEADERS フレームとして送信する
async fn pair_with_response(
    status: u16,
    server_name: &str,
    protocol: Option<&str>,
) -> (Result<Connection>, Peer) {
    pair_with_transport(status, server_name, protocol, None).await
}

/// transport parameter の境界条件も実接続で指定する
async fn pair_with_transport(
    status: u16,
    server_name: &str,
    protocol: Option<&str>,
    params: Option<TransportParams>,
) -> (Result<Connection>, Peer) {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れること");
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
        .expect("証明書を作れること");
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).expect("証明書を書けること");
    std::fs::write(&key_path, cert.signing_key.serialize_pem()).expect("鍵を書けること");
    let params = params
        .unwrap_or_default()
        .with_datagram(65535)
        .with_reset_stream_at(true);
    let mut server = Server::bind(
        "127.0.0.1:0".parse().expect("アドレスが有効であること"),
        &cert_path,
        &key_path,
        Some(ServerConfig::new(&[H3_ALPN]).with_transport_params(params)),
    )
    .await
    .expect("サーバーを起動できること");
    let config = build_config(&MoqTlsOptions {
        insecure: false,
        ca_cert: Some(cert_path.to_string_lossy().into()),
    })
    .expect("CA 検証を設定できること");
    let client = connect(
        server.local_addr(),
        "127.0.0.1:0".parse().expect("アドレスが有効であること"),
        server_name,
        &config,
        "localhost:4433",
        "/moq?room=test",
    );
    let peer = async {
        let mut quic = server
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("接続が届くこと");
        assert!(
            quic.supports_reset_stream_at(),
            "reliable reset を交渉すること"
        );
        let wt = shiguredo_http3::webtransport::Settings::new()
            .wt_enabled(VarInt::from_static(1))
            .wt_initial_max_streams_uni(VarInt::from_static(1))
            .wt_initial_max_streams_bidi(VarInt::from_static(1))
            .wt_initial_max_data(VarInt::from_static(64));
        let mut h3 = H3Server::new(H3Settings::default().enable_webtransport_server(wt));
        h3.set_webtransport_transport_verified(true, quic.supports_reset_stream_at())
            .expect("transport parameter を検証できること");
        let control = quic.open_uni_stream().expect("control stream を開けること");
        let encoder = quic.open_uni_stream().expect("encoder stream を開けること");
        let decoder = quic.open_uni_stream().expect("decoder stream を開けること");
        let init = h3
            .init_h3_streams(control as u64, encoder as u64, decoder as u64)
            .expect("HTTP/3 を初期化できること");
        for (id, data) in [
            (control, init.control_data),
            (encoder, init.encoder_data),
            (decoder, init.decoder_data),
        ] {
            quic.write_stream(id, &data, false)
                .expect("HTTP/3 の初期データを送れること");
        }
        let mut headers = HashMap::new();
        loop {
            quic.flush().await.expect("初期データを送れること");
            if let ConnectionEvent::StreamData {
                stream_id,
                data,
                fin,
            } = quic.recv_event().await.expect("CONNECT を受信できること")
            {
                h3.feed_stream(stream_id as u64, &data, fin)
                    .expect("HTTP/3 データが有効であること");
                quic.extend_max_stream_offset(stream_id, data.len() as u64)
                    .expect("受信枠を返せること");
            }
            let mut request = None;
            while let Some(event) = h3.poll_event().expect("HTTP/3 イベントを取得できること")
            {
                match event {
                    Event::Header { name, value, .. } => {
                        headers.insert(name, value);
                    }
                    Event::HeadersEnd { stream_id } => request = Some(stream_id),
                    _ => {}
                }
            }
            if let Some(id) = request {
                assert_eq!(id, 0, "CONNECT が最初の bidi であること");
                for (name, value) in [
                    (b":method".as_slice(), b"CONNECT".as_slice()),
                    (b":scheme", b"https"),
                    (b":authority", b"localhost:4433"),
                    (b":path", b"/moq?room=test"),
                    (b"wt-available-protocols", b"\"moqt-22\""),
                ] {
                    assert_eq!(
                        headers.get(name).map(Vec::as_slice),
                        Some(value),
                        "CONNECT ヘッダーが正しいこと"
                    );
                }
                let mut response = ConnectResponse::new(status);
                if let Some(protocol) = protocol {
                    response = response.with_protocol(protocol);
                }
                let response = response.to_headers().expect("応答ヘッダーを作れること");
                if protocol == Some("moqt-22") {
                    h3.send_response(id, &response, false)
                        .expect("CONNECT 応答を作れること");
                } else {
                    // 送信側の状態機械は不正な WT-Protocol を拒否するため、このケースだけ
                    // QPACK とフレームエンコーダーで wire 上の不正入力を構築する。
                    let mut block = vec![0; 1024];
                    let len = shiguredo_http3::qpack::Encoder::new()
                        .encode(&mut block, &response, 0)
                        .expect("QPACK を符号化できること");
                    let mut frame = Vec::new();
                    shiguredo_http3::varint::encode_into_vec(&mut frame, VarInt::from_static(1));
                    shiguredo_http3::varint::encode_into_vec(
                        &mut frame,
                        VarInt::new(len as u64).expect("長さが varint に収まること"),
                    );
                    frame.extend_from_slice(&block[..len]);
                    quic.write_stream(id as i64, &frame, false)
                        .expect("不正応答を実際の QUIC で送れること");
                }
                let mut peer = Peer {
                    quic,
                    h3,
                    events: VecDeque::new(),
                };
                peer.flush().await;
                return peer;
            }
        }
    };
    tokio::join!(client, peer)
}

/// 単方向と両端開始の双方向、および小さな WT_MAX_DATA / WT_MAX_STREAMS を検証する
#[tokio::test]
async fn streams_and_flow_control() {
    timeout(TEST_TIMEOUT, async {
        let (client, mut peer) = pair(200).await;
        let (handle, mut uni, mut bidi) = client.expect("CONNECT が成功すること").split();
        let expected = vec![0x5a; 4096];
        let expected_peer = expected.clone();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                assert_eq!(
                    peer.body(false).await.1,
                    expected_peer,
                    "64 バイトの WT 枠を更新して全本文を受信すること"
                );
            }
            let (id, body) = peer.body(true).await;
            assert_eq!(
                body, b"bidi request",
                "クライアント開始 bidi の本文が届くこと"
            );
            peer.quic
                .write_stream(id as i64, b"bidi response", true)
                .expect("返信できること");
            peer.open(false, b"peer uni", true);
            let id = peer.open(true, b"peer bidi", true);
            peer.flush().await;
            let mut response = Vec::new();
            loop {
                match peer.next().await {
                    WtEvent::BidiStreamData { stream_id, data } if stream_id == id as u64 => {
                        response.extend(data)
                    }
                    WtEvent::BidiStreamEnd { stream_id } if stream_id == id as u64 => {
                        assert_eq!(response, b"peer response", "ピア開始 bidi に返信できること");
                    }
                    WtEvent::SessionClosed {
                        close_error_code, ..
                    } => {
                        assert_eq!(
                            close_error_code, 7,
                            "CLOSE_SESSION に MOQT のコードを載せること"
                        );
                        return;
                    }
                    _ => {}
                }
            }
        });
        for _ in 0..3 {
            let mut stream = handle
                .open_send_stream()
                .await
                .expect("WT ストリーム枠を再取得できること");
            stream
                .write_all(&expected)
                .await
                .expect("WT のデータ枠より大きい write が完了すること");
            stream.finish().await.expect("FIN を送れること");
        }
        let (mut recv, mut send) = handle
            .open_bidirectional_stream()
            .await
            .expect("bidi を開けること");
        send.write_all(b"bidi request")
            .await
            .expect("リクエストを送れること");
        send.finish().await.expect("送信方向を閉じられること");
        assert_eq!(read_all(&mut recv).await, b"bidi response");
        let mut recv = uni
            .accept()
            .await
            .expect("accept が成功すること")
            .expect("uni が届くこと");
        assert_eq!(
            read_all(&mut recv).await,
            b"peer uni",
            "WT ヘッダーを本文へ混ぜないこと"
        );
        let (mut recv, mut send) = bidi
            .accept()
            .await
            .expect("accept が成功すること")
            .expect("bidi が届くこと");
        assert_eq!(read_all(&mut recv).await, b"peer bidi");
        send.write_all(b"peer response")
            .await
            .expect("ピア開始 bidi へ送れること");
        send.finish()
            .await
            .expect("ピア開始 bidi の返信を閉じられること");
        handle.close(7).await;
        server.await.expect("ピアの検証が成功すること");
    })
    .await
    .expect("フロー制御で停止せず完了すること");
}

async fn read_all(stream: &mut RecvStream) -> Vec<u8> {
    let mut result = Vec::new();
    let mut buf = [0; 17];
    loop {
        let size = stream.read(&mut buf).await.expect("本文を読めること");
        if size == 0 {
            return result;
        }
        result.extend_from_slice(&buf[..size]);
    }
}

/// 非成功応答では MOQT の開始前に接続失敗を返す
#[tokio::test]
async fn rejects_connect_status() {
    timeout(TEST_TIMEOUT, async {
        let (client, _peer) = pair(403).await;
        let error = match client {
            Ok(_) => panic!("拒否応答を成功にしてはならない"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("403"),
            "拒否された HTTP ステータスを伝えること"
        );
    })
    .await
    .expect("拒否応答を速やかに返すこと");
}

/// MOQT のサブプロトコルを選択しない応答では MOQT を開始しないこと
#[tokio::test]
async fn rejects_unnegotiated_moqt_protocol() {
    timeout(TEST_TIMEOUT, async {
        for protocol in [None, Some("other-protocol")] {
            let (client, _peer) = pair_with_response(200, "localhost", protocol).await;
            let error = match client {
                Ok(_) => panic!("サブプロトコル未合意を成功にしてはならない"),
                Err(error) => error,
            };
            assert!(
                error.to_string().contains("CONNECT closed"),
                "WT_ALPN_ERROR による終了を通知すること"
            );
        }
    })
    .await
    .expect("サブプロトコル不一致を検出すること");
}

/// 複数パケットに分かれる WT ヘッダーを本文として読み手へ渡さないこと
#[tokio::test]
async fn fragmented_stream_header() {
    timeout(TEST_TIMEOUT, async {
        let (client, mut peer) = pair(200).await;
        let (handle, mut uni, _) = client.expect("CONNECT が成功すること").split();
        let server = tokio::spawn(async move {
            let id = peer.quic.open_uni_stream().expect("uni を開けること");
            peer.h3
                .register_local_wt_stream(0, id as u64)
                .expect("WT ストリームを登録できること");
            let mut header = Vec::new();
            StreamHeader::new(0)
                .expect("セッション ID が有効であること")
                .encode_unidirectional(&mut header);
            for byte in header {
                peer.quic
                    .write_stream(id, &[byte], false)
                    .expect("ヘッダーを 1 バイトずつ送れること");
                peer.flush().await;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            peer.quic
                .write_stream(id, b"payload", true)
                .expect("本文と FIN を送れること");
            peer.flush().await;
            loop {
                if let WtEvent::SessionClosed { .. } = peer.next().await {
                    return;
                }
            }
        });
        let mut recv = uni
            .accept()
            .await
            .expect("accept が成功すること")
            .expect("ヘッダーの完成後にストリームが届くこと");
        assert_eq!(
            read_all(&mut recv).await,
            b"payload",
            "分割されたヘッダーを取り除くこと"
        );
        handle.close(0).await;
        server.await.expect("ピアが完了すること");
    })
    .await
    .expect("分割されたヘッダーを処理できること");
}

/// DNS 名を使わず IP SAN を検証して WebTransport へ接続できること
#[tokio::test]
async fn verifies_ip_certificate() {
    timeout(TEST_TIMEOUT, async {
        let (client, mut peer) = pair_with_name(200, "127.0.0.1").await;
        let (handle, _, _) = client.expect("IP SAN を検証して接続できること").split();
        let close = handle.close(0);
        let receive = async {
            loop {
                if let WtEvent::SessionClosed {
                    close_error_code, ..
                } = peer.next().await
                {
                    assert_eq!(close_error_code, 0, "正常終了カプセルを受信すること");
                    return;
                }
            }
        };
        tokio::join!(close, receive);
    })
    .await
    .expect("IP アドレスでも交渉が完了すること");
}

/// reliable reset のヘッダー配送保証と、両方向のエラーコードの写像を検証する
#[tokio::test]
async fn reliable_resets() {
    timeout(TEST_TIMEOUT, async {
        let (client, mut peer) = pair(200).await;
        let (handle, mut uni, _) = client.expect("CONNECT が成功すること").split();
        let server = tokio::spawn(async move {
            let mut body = Vec::new();
            loop {
                match peer.next().await {
                    WtEvent::UniStreamData { data, .. } => body.extend(data),
                    WtEvent::StreamReset { error_code, .. } => {
                        assert_eq!(
                            ApplicationErrorCode::from_http3_code(error_code),
                            Some(42),
                            "送信 reset を HTTP/3 のコードへ写像すること"
                        );
                        assert_eq!(
                            body, b"before reset",
                            "reset より前に保証した本文が届くこと"
                        );
                        break;
                    }
                    _ => {}
                }
            }
            let id = peer.open(false, b"peer reset", false);
            peer.flush().await;
            assert!(
                !peer.quic.has_pending_data(),
                "ヘッダーを QUIC に渡し終えること"
            );
            peer.quic
                .reset_stream_reliable(id, ApplicationErrorCode::to_http3_code(55))
                .expect("ピアから reliable reset を送れること");
            peer.flush().await;
            loop {
                if let WtEvent::SessionClosed { .. } = peer.next().await {
                    return;
                }
            }
        });
        let mut send = handle
            .open_send_stream()
            .await
            .expect("ストリームを開けること");
        send.write_all(b"before reset")
            .await
            .expect("本文を送れること");
        send.reset(42).await.expect("reliable reset を送れること");
        let mut recv = uni
            .accept()
            .await
            .expect("accept が成功すること")
            .expect("reset 対象のストリームが届くこと");
        let mut body = Vec::new();
        let mut buf = [0; 64];
        loop {
            match recv.read(&mut buf).await {
                Ok(0) => panic!("reset を FIN として扱ってはならない"),
                Ok(size) => body.extend_from_slice(&buf[..size]),
                Err(error) => {
                    let reset = error
                        .get_ref()
                        .and_then(|e| e.downcast_ref::<StreamReset>())
                        .expect("MOQT の reset として通知すること");
                    assert_eq!(reset.0, 55, "HTTP/3 のコードを MOQT のコードへ戻すこと");
                    break;
                }
            }
        }
        assert_eq!(
            body, b"peer reset",
            "reliable reset 前のデータを受信すること"
        );
        handle.close(0).await;
        server.await.expect("ピアの reset 検証が成功すること");
    })
    .await
    .expect("reset で接続全体を止めないこと");
}

/// open 直後の reset でも WT ヘッダーを reliable size に含めること
#[tokio::test]
async fn reset_waits_for_stream_header() {
    timeout(TEST_TIMEOUT, async {
        let params = TransportParams::new().with_initial_max_stream_data_uni(1);
        let (client, mut peer) =
            pair_with_transport(200, "localhost", Some("moqt-22"), Some(params)).await;
        let (handle, _, _) = client
            .expect("1 バイトずつの受信枠でも CONNECT できること")
            .split();
        let (reset_tx, reset_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut opened = false;
            let mut reset_tx = Some(reset_tx);
            loop {
                match peer.next().await {
                    WtEvent::UniStreamOpen { .. } => opened = true,
                    WtEvent::StreamReset { error_code, .. } => {
                        assert!(opened, "ヘッダーの配送後に reset を通知すること");
                        assert_eq!(ApplicationErrorCode::from_http3_code(error_code), Some(17));
                        reset_tx
                            .take()
                            .expect("reset が 1 回だけ届くこと")
                            .send(())
                            .expect("受信を通知できること");
                    }
                    WtEvent::SessionClosed { .. } => return,
                    _ => {}
                }
            }
        });
        let mut send = handle
            .open_send_stream()
            .await
            .expect("ストリームを開けること");
        send.reset(17)
            .await
            .expect("ヘッダーの送信を待って reset できること");
        reset_rx.await.expect("実際にピアが reset を受信したこと");
        handle.close(0).await;
        server.await.expect("ヘッダーの配送保証を検証できること");
    })
    .await
    .expect("未送信のヘッダーを捨てずに reset すること");
}

/// WT データ枠を待つ write も STOP_SENDING で解除されること
#[tokio::test]
async fn stop_sending_unblocks_writer() {
    timeout(TEST_TIMEOUT, async {
        let (client, mut peer) = pair(200).await;
        let (handle, _, _) = client.expect("CONNECT が成功すること").split();
        let server = tokio::spawn(async move {
            loop {
                if let WtEvent::UniStreamOpen { stream_id, .. } = peer.next().await {
                    peer.quic
                        .stop_sending(stream_id as i64, ApplicationErrorCode::to_http3_code(91))
                        .expect("STOP_SENDING を送れること");
                    peer.flush().await;
                    break;
                }
            }
            loop {
                if let WtEvent::SessionClosed { .. } = peer.next().await {
                    return;
                }
            }
        });
        let mut send = handle
            .open_send_stream()
            .await
            .expect("ストリームを開けること");
        let error = send
            .write_all(&vec![1; 4096])
            .await
            .expect_err("停止要求で待機中の write が失敗すること");
        assert_eq!(
            error.kind(),
            io::ErrorKind::BrokenPipe,
            "送信方向の終了を通知すること"
        );
        handle.close(0).await;
        server.await.expect("ピアが終了すること");
    })
    .await
    .expect("停止要求で write が待ち続けないこと");
}

/// drain は新しいストリームを拒否し、既存のストリームは完了できること
#[tokio::test]
async fn drain_and_peer_close() {
    timeout(TEST_TIMEOUT, async {
        let (client, mut peer) = pair(200).await;
        let (handle, mut uni, _) = client.expect("CONNECT が成功すること").split();
        let server = tokio::spawn(async move {
            let mut body = Vec::new();
            loop {
                match peer.next().await {
                    WtEvent::UniStreamOpen { .. } => {
                        peer.capsule(Capsule::DrainSession);
                        peer.flush().await;
                    }
                    WtEvent::UniStreamData { data, .. } => body.extend(data),
                    WtEvent::UniStreamEnd { .. } => {
                        assert_eq!(
                            body, b"firstsecond",
                            "drain 後も既存ストリームを完了できること"
                        );
                        peer.capsule(Capsule::CloseSession {
                            error_code: 123,
                            message: "finished".into(),
                        });
                        peer.flush().await;
                        // クライアントがカプセルを処理するまで QUIC の ACK と受信を駆動する。
                        let _ = peer.quic.recv_event().await;
                        return;
                    }
                    _ => {}
                }
            }
        });
        let mut send = handle
            .open_send_stream()
            .await
            .expect("最初のストリームを開けること");
        send.write_all(b"first")
            .await
            .expect("最初の本文を送れること");
        assert!(
            handle.open_send_stream().await.is_err(),
            "drain 後の新規ストリームを拒否すること"
        );
        send.write_all(b"second")
            .await
            .expect("既存ストリームは送信を継続できること");
        send.finish().await.expect("既存ストリームを閉じられること");
        assert!(
            uni.accept().await.expect("終了を検出できること").is_none(),
            "peer close が acceptor へ伝わること"
        );
        server.await.expect("ピアが完了すること");
    })
    .await
    .expect("drain と close が待機タスクへ伝わること");
}

/// HTTP/3 の致命的な入力には規定の接続終了コードを返すこと
#[tokio::test]
async fn closes_with_http3_error_code() {
    timeout(TEST_TIMEOUT, async {
        for expected_code in [0x104, 0x33] {
            let (client, mut peer) = pair(200).await;
            let (_handle, mut uni, _) = client.expect("CONNECT が成功すること").split();
            let server = tokio::spawn(async move {
                if expected_code == 0x104 {
                    // サーバーの control stream は最初のサーバー開始 uni (ID 3)。
                    peer.quic
                        .reset_stream(3, 0)
                        .expect("critical stream を reset できること");
                    peer.quic.flush().await.expect("reset を送れること");
                } else {
                    // HTTP Datagram に必須の Quarter Stream ID を欠落させる。
                    peer.quic
                        .send_datagram(&[])
                        .await
                        .expect("空の QUIC DATAGRAM を送れること");
                }
                loop {
                    if let ConnectionEvent::ConnectionClosed { error_code, .. } = peer
                        .quic
                        .recv_event()
                        .await
                        .expect("接続終了を受信できること")
                    {
                        assert_eq!(
                            error_code, expected_code,
                            "HTTP/3 が指定したコードで接続を閉じること"
                        );
                        return;
                    }
                }
            });
            assert!(
                uni.accept().await.expect("終了を受け取れること").is_none(),
                "エラーを acceptor に伝えること"
            );
            server.await.expect("接続終了コードを検証できること");
        }
    })
    .await
    .expect("プロトコルエラーで接続を閉じること");
}
