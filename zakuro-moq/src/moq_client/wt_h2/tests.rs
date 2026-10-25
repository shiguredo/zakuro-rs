//! wt_h2 の接続・ストリームを実 TCP + TLS + HTTP/2 のサーバーと検証する
//!
//! モックやスタブは使わない。rcgen の自己署名証明書で tokio-rustls のサーバーを立て、
//! shiguredo_http2 の Sans I/O サーバー接続で WebTransport over HTTP/2 を終端し、
//! クライアントの接続・ストリーム操作をエンドツーエンドで確かめる。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use shiguredo_http2::webtransport::stream::stream_id as wt_stream_id;
use shiguredo_http2::webtransport::{
    Capsule, CapsuleEncoder, WtConfig, WtErrorKind, WtEvent, WtSession, WtStreamId,
};
use shiguredo_http2::{ErrorCode, Event, HeaderField, Limits, StreamId};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;

use crate::moq_client::StreamReset;
use crate::moq_client::transport::ALPN;

use super::{
    H2_ALPN, H2_INITIAL_WINDOW_SIZE, H2Io, WEBTRANSPORT_PROTOCOL, WT_INITIAL_MAX_DATA,
    WT_INITIAL_MAX_STREAM_DATA, WT_INITIAL_MAX_STREAMS_BIDI, WT_INITIAL_MAX_STREAMS_UNI,
    WT_SEND_CHUNK_SIZE, build_connect_headers, build_limits, build_wt_config,
    connect_with_wt_config, receive_window_increment, session_close_code, verify_wt_protocol,
    wt_http2_error_code,
};

/// テスト全体のタイムアウト
const TEST_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// テスト用の TLS / サーバー
// ---------------------------------------------------------------------------

/// テスト用の TLS 設定を作る
///
/// サーバーは `localhost` の自己署名証明書を使い、クライアントはその証明書だけをルート
/// として信頼する。ALPN は両方 `h2` にする。
fn test_tls_pair() -> (TlsAcceptor, Arc<rustls::ClientConfig>) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("自己署名証明書の生成に成功すること");
    let certificate = certified.cert.der().clone();
    let private_key =
        rustls::pki_types::PrivateKeyDer::try_from(certified.signing_key.serialize_der())
            .expect("秘密鍵の変換に成功すること");

    let mut server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("TLS バージョンの設定に成功すること")
    .with_no_client_auth()
    .with_single_cert(vec![certificate.clone()], private_key)
    .expect("TLS サーバー設定の構築に成功すること");
    server_config.alpn_protocols = vec![H2_ALPN.to_vec()];

    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(certificate)
        .expect("ルート証明書の追加に成功すること");
    let mut client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("TLS バージョンの設定に成功すること")
    .with_root_certificates(roots)
    .with_no_client_auth();
    client_config.alpn_protocols = vec![H2_ALPN.to_vec()];

    (
        TlsAcceptor::from(Arc::new(server_config)),
        Arc::new(client_config),
    )
}

/// テスト用の TCP リスナーを作り、待ち受けアドレスを返す
async fn bind_listener() -> (TcpListener, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("テスト用の TCP リスナーの bind に成功すること");
    let addr = listener
        .local_addr()
        .expect("リスナーのアドレスを取得できること");
    (listener, addr)
}

/// テスト用のサーバー側 TLS ストリーム型
type ServerTlsStream = tokio_rustls::server::TlsStream<TcpStream>;

/// テストサーバーの 1 ステップ分の結果
enum ServerStep {
    /// WT イベントと、CONNECT ストリームが END_STREAM で終了したか
    Events { events: Vec<WtEvent>, ended: bool },
    /// 接続が閉じた
    Closed,
}

/// 実 TLS の上で WebTransport over HTTP/2 を終端するテストサーバー
struct TestServer {
    /// HTTP/2 + TLS + TCP の I/O
    io: H2Io<ServerTlsStream>,
    /// Sans I/O の WT セッション
    wt: WtSession,
    /// WebTransport の CONNECT ストリーム
    connect_stream: StreamId,
    /// WT の出力でまだ HTTP/2 へ流していないバイト列
    pending: Vec<u8>,
    /// 受信した HTTP/2 PING の数 (keep-alive の検証用)
    ping_count: Arc<AtomicU64>,
}

impl TestServer {
    /// 接続を待ち受け、CONNECT を検証して WebTransport セッションを確立する
    ///
    /// `wt_config` はサーバーが SETTINGS で広告する WT の初期フロー制御値であり、
    /// クライアントの送信上限になる。
    async fn accept(
        listener: TcpListener,
        tls: TlsAcceptor,
        wt_config: WtConfig,
    ) -> std::result::Result<Self, String> {
        let (tcp, _) = listener
            .accept()
            .await
            .map_err(|e| format!("TCP accept に失敗しました: {e}"))?;
        let stream = tls
            .accept(tcp)
            .await
            .map_err(|e| format!("TLS accept に失敗しました: {e}"))?;

        let limits = build_limits(&wt_config)
            .map_err(|e| format!("HTTP/2 の Limits の構築に失敗しました: {e}"))?;
        let mut io = H2Io::server(stream, limits);
        io.initiate()
            .await
            .map_err(|e| format!("サーバーの HTTP/2 初期化に失敗しました: {e}"))?;

        loop {
            let event = io
                .next_event()
                .await
                .map_err(|e| format!("サーバーの HTTP/2 読み込みに失敗しました: {e}"))?;
            match event {
                Event::HeadersReceived {
                    stream_id,
                    headers,
                    end_stream,
                    protocol,
                } => {
                    if protocol.as_deref() != Some(WEBTRANSPORT_PROTOCOL) {
                        return Err(format!(
                            "Extended CONNECT の :protocol が webtransport ではありません: {protocol:?}"
                        ));
                    }
                    verify_connect_request(&headers)?;
                    if end_stream {
                        return Err("CONNECT リクエストが END_STREAM 付きです".to_string());
                    }
                    let wt_protocol = shiguredo_http2::webtransport::serialize_wt_protocol(ALPN)
                        .map_err(|e| format!("WT-Protocol のシリアライズに失敗しました: {e}"))?;
                    let response = vec![
                        HeaderField::from_static(b":status", b"200"),
                        HeaderField::new(b"wt-protocol", wt_protocol).map_err(|e| {
                            format!("WT-Protocol ヘッダーの構築に失敗しました: {e}")
                        })?,
                    ];
                    io.conn
                        .send_response(stream_id, response, false)
                        .map_err(|e| format!("CONNECT 応答の送信に失敗しました: {e}"))?;
                    io.flush()
                        .await
                        .map_err(|e| format!("CONNECT 応答の書き出しに失敗しました: {e}"))?;

                    let mut local = wt_config.clone();
                    local.overlay_settings(io.conn.local_settings());
                    let mut peer = WtConfig::peer_default();
                    peer.overlay_settings(io.conn.remote_settings());
                    let mut wt = WtSession::server(local, peer);
                    wt.initiate()
                        .map_err(|e| format!("WtSession::initiate に失敗しました: {e}"))?;
                    return Ok(Self {
                        io,
                        wt,
                        connect_stream: stream_id,
                        pending: Vec::new(),
                        ping_count: Arc::new(AtomicU64::new(0)),
                    });
                }
                Event::ConnectionError { error_code, reason } => {
                    return Err(format!("HTTP/2 接続エラー: {error_code}: {reason}"));
                }
                _ => {}
            }
        }
    }

    /// H2 のイベントを 1 つ待ち、CONNECT ストリームの DATA を WT へ流す
    async fn step(&mut self) -> std::result::Result<ServerStep, String> {
        loop {
            let event = match self.io.next_event().await {
                Ok(event) => event,
                // クライアントが切断した場合は接続終了として扱う
                Err(_) => return Ok(ServerStep::Closed),
            };
            match event {
                Event::DataReceived {
                    stream_id,
                    data,
                    end_stream,
                } if stream_id == self.connect_stream => {
                    self.wt
                        .feed(&data)
                        .map_err(|e| format!("サーバーの WT feed に失敗しました: {e}"))?;
                    self.wt
                        .process()
                        .map_err(|e| format!("サーバーの WT process に失敗しました: {e}"))?;
                    let events = self.drain_events();
                    // テストのサーバーは受け取ったデータを即座に消費したことにして、
                    // クライアントの送信ウィンドウを広げる
                    for event in &events {
                        if let WtEvent::StreamData {
                            stream_id,
                            data,
                            fin,
                        } = event
                        {
                            self.consume(*stream_id, data.len() as u64, *fin)?;
                        }
                    }
                    // RFC 9113 §6.9.1: 受信したデータの分だけウィンドウを戻す
                    let size = u32::try_from(data.len())
                        .expect("logical invariant: HTTP/2 DATA の長さは u32 に収まる");
                    if size > 0 {
                        self.io
                            .conn
                            .send_window_update(StreamId::Connection, size)
                            .map_err(|e| format!("接続 WINDOW_UPDATE の送信に失敗しました: {e}"))?;
                        if !end_stream {
                            self.io
                                .conn
                                .send_window_update(self.connect_stream, size)
                                .map_err(|e| {
                                    format!("ストリーム WINDOW_UPDATE の送信に失敗しました: {e}")
                                })?;
                        }
                    }
                    let has_events = !events.is_empty();
                    if let Err(error) = self.flush().await {
                        // クライアントが終了した直後は書き出しに失敗しうる。受信済みの
                        // イベントを失わないよう、伝えるべきものがある場合は継続する
                        if !has_events && !end_stream {
                            return Ok(ServerStep::Closed);
                        }
                        tracing::debug!("テストサーバーの書き出しに失敗しました: {error}");
                    }
                    return Ok(ServerStep::Events {
                        events,
                        ended: end_stream,
                    });
                }
                Event::StreamReset { stream_id, .. } | Event::StreamClosed { stream_id }
                    if stream_id == self.connect_stream =>
                {
                    return Ok(ServerStep::Closed);
                }
                Event::ConnectionError { .. } => return Ok(ServerStep::Closed),
                Event::WindowUpdateReceived { .. } | Event::SettingsReceived { .. } => {
                    // 送信ウィンドウが開いた可能性があるため、待っていた出力を流す。
                    // クライアントが接続を閉じた直後は失敗しうるため、書き出しの失敗は
                    // 無視する (次の読み込みが接続終了を返す)
                    let _ = self.flush().await;
                }
                Event::PingReceived { ack: false, .. } => {
                    self.ping_count.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
    }

    /// WT の出力を HTTP/2 の DATA として流し、TCP まで書き出す
    async fn flush(&mut self) -> std::result::Result<(), String> {
        while let Some(output) = self.wt.poll_output() {
            self.pending.extend_from_slice(&output);
        }
        while !self.pending.is_empty() && !self.io.conn.has_pending_send_data(self.connect_stream) {
            let size = self.pending.len().min(WT_SEND_CHUNK_SIZE);
            let chunk: Vec<u8> = self.pending.drain(..size).collect();
            self.io
                .conn
                .send_data(self.connect_stream, chunk, false)
                .map_err(|e| format!("サーバーの DATA 送信に失敗しました: {e}"))?;
        }
        self.io
            .flush()
            .await
            .map_err(|e| format!("サーバーの書き出しに失敗しました: {e}"))
    }

    /// WT イベントをすべて取り出す
    fn drain_events(&mut self) -> Vec<WtEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.wt.poll_event() {
            events.push(event);
        }
        events
    }

    /// 受信データを消費したものとして受信ウィンドウを拡張する
    fn consume(
        &mut self,
        stream_id: WtStreamId,
        size: u64,
        fin: bool,
    ) -> std::result::Result<(), String> {
        if fin || size == 0 {
            return Ok(());
        }
        self.wt
            .grow_recv_window(size)
            .map_err(|e| format!("セッションの受信ウィンドウ拡張に失敗しました: {e}"))?;
        // 終端済みのストリームでは拡張できないため、失敗は無視する
        let _ = self.wt.grow_stream_recv_window(stream_id, size);
        Ok(())
    }

    /// 指定ストリームの WT_STOP_SENDING を観測するまで読む
    ///
    /// WT セッションの出力を流さないため、WT_STOP_SENDING に自動で返る
    /// `WT_RESET_STREAM` はクライアントへ届かない。これにより、STOP_SENDING を送った
    /// 直後にピアから届く「飛行中のデータ」を決定的に再現できる。
    async fn wait_for_stop_sending(
        &mut self,
        target: WtStreamId,
    ) -> std::result::Result<(), String> {
        loop {
            let event = self
                .io
                .next_event()
                .await
                .map_err(|e| format!("サーバーの HTTP/2 読み込みに失敗しました: {e}"))?;
            match event {
                Event::DataReceived {
                    stream_id,
                    data,
                    end_stream,
                } if stream_id == self.connect_stream => {
                    self.wt
                        .feed(&data)
                        .map_err(|e| format!("サーバーの WT feed に失敗しました: {e}"))?;
                    self.wt
                        .process()
                        .map_err(|e| format!("サーバーの WT process に失敗しました: {e}"))?;
                    let size = u32::try_from(data.len())
                        .expect("logical invariant: HTTP/2 DATA の長さは u32 に収まる");
                    if size > 0 {
                        self.io
                            .conn
                            .send_window_update(StreamId::Connection, size)
                            .map_err(|e| format!("接続 WINDOW_UPDATE の送信に失敗しました: {e}"))?;
                        if !end_stream {
                            self.io
                                .conn
                                .send_window_update(self.connect_stream, size)
                                .map_err(|e| {
                                    format!("ストリーム WINDOW_UPDATE の送信に失敗しました: {e}")
                                })?;
                        }
                    }
                    while let Some(event) = self.wt.poll_event() {
                        if let WtEvent::StopSending { stream_id, .. } = event
                            && stream_id == target
                        {
                            return Ok(());
                        }
                    }
                    if end_stream {
                        return Err(
                            "WT_STOP_SENDING を受信する前に CONNECT ストリームが終了しました"
                                .to_string(),
                        );
                    }
                }
                Event::StreamReset { .. }
                | Event::StreamClosed { .. }
                | Event::ConnectionError { .. } => {
                    return Err("WT_STOP_SENDING を受信する前に接続が終了しました".to_string());
                }
                _ => {}
            }
        }
    }

    /// WT セッションの状態機械を通さず、WT_STREAM capsule を HTTP/2 の DATA として送る
    ///
    /// ピアの `WtSession` は WT_STOP_SENDING を受けるとストリームを reset して以降の
    /// 送信を拒否するため、STOP_SENDING 後に届く「飛行中のデータ」は通常の API では
    /// 再現できない。テストでは capsule を直接組み立てて送る。
    async fn inject_stream_data(
        &mut self,
        stream_id: WtStreamId,
        data: &[u8],
    ) -> std::result::Result<(), String> {
        let mut encoder = CapsuleEncoder::new();
        encoder.encode(&Capsule::WtStream {
            stream_id,
            data: data.to_vec(),
            fin: false,
        });
        let bytes = encoder.take();
        let mut offset = 0;
        while offset < bytes.len() {
            if self.io.conn.has_pending_send_data(self.connect_stream) {
                // 送信バッファが空くまで読み進め、WINDOW_UPDATE を処理する
                self.io
                    .next_event()
                    .await
                    .map_err(|e| format!("サーバーの HTTP/2 読み込みに失敗しました: {e}"))?;
                continue;
            }
            let size = (bytes.len() - offset).min(WT_SEND_CHUNK_SIZE);
            self.io
                .conn
                .send_data(
                    self.connect_stream,
                    bytes[offset..offset + size].to_vec(),
                    false,
                )
                .map_err(|e| format!("注入する DATA の送信に失敗しました: {e}"))?;
            offset += size;
        }
        self.io
            .flush()
            .await
            .map_err(|e| format!("注入した DATA の書き出しに失敗しました: {e}"))
    }
}

/// Extended CONNECT のヘッダーが仕様どおりかを検証する
///
/// draft-ietf-webtrans-http2-15 §3.1 / §3.2 の擬似ヘッダーと、MOQT のプロトコル
/// 識別子を通知する WT-Available-Protocols を確認する。
fn verify_connect_request(headers: &[HeaderField]) -> std::result::Result<(), String> {
    let value = |name: &[u8]| {
        headers
            .iter()
            .find(|header| header.name() == name)
            .map(HeaderField::value)
    };
    if value(b":method") != Some(b"CONNECT".as_slice()) {
        return Err(format!(
            "Extended CONNECT の :method が CONNECT ではありません: {:?}",
            value(b":method")
        ));
    }
    if value(b":scheme") != Some(b"https".as_slice()) {
        return Err(format!(
            "Extended CONNECT の :scheme が https ではありません: {:?}",
            value(b":scheme")
        ));
    }
    if value(b":authority").is_none_or(<[u8]>::is_empty) {
        return Err("Extended CONNECT の :authority がありません".to_string());
    }
    if value(b":path").is_none_or(|path| !path.starts_with(b"/")) {
        return Err("Extended CONNECT の :path がありません".to_string());
    }
    let available = value(b"wt-available-protocols")
        .ok_or_else(|| "WT-Available-Protocols がありません".to_string())?;
    if available != br#""moqt-22""# {
        return Err(format!(
            "WT-Available-Protocols が moqt-22 ではありません: {:?}",
            String::from_utf8_lossy(available)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// クライアント側のヘルパー
// ---------------------------------------------------------------------------

/// テストサーバーへ WebTransport over HTTP/2 で接続する
async fn connect_client(
    addr: std::net::SocketAddr,
    tls: &Arc<rustls::ClientConfig>,
) -> crate::error::Result<super::Connection> {
    super::connect(
        addr,
        "localhost",
        &format!("localhost:{}", addr.port()),
        "/test",
        tls,
    )
    .await
}

/// WT の初期フロー制御値を差し替えてテストサーバーへ接続する
async fn connect_client_with_wt_config(
    addr: std::net::SocketAddr,
    tls: &Arc<rustls::ClientConfig>,
    wt_config: &WtConfig,
) -> crate::error::Result<super::Connection> {
    connect_with_wt_config(
        addr,
        "localhost",
        &format!("localhost:{}", addr.port()),
        "/test",
        tls,
        wt_config,
    )
    .await
}

/// ストリームを FIN まで読み切る
async fn read_to_end(stream: &mut super::RecvStream) -> std::io::Result<Vec<u8>> {
    let mut received = Vec::new();
    let mut buf = vec![0u8; 4096];
    loop {
        let size = stream.read(&mut buf).await?;
        if size == 0 {
            return Ok(received);
        }
        received.extend_from_slice(&buf[..size]);
    }
}

/// ストリームから指定長を読み切る
async fn read_exact(
    stream: &mut super::RecvStream,
    buf: &mut [u8],
) -> std::result::Result<(), String> {
    let mut filled = 0;
    while filled < buf.len() {
        let size = stream
            .read(&mut buf[filled..])
            .await
            .map_err(|e| format!("ストリームの読み込みに失敗しました: {e}"))?;
        if size == 0 {
            return Err("ストリームが途中で終了しました".to_string());
        }
        filled += size;
    }
    Ok(())
}

/// 読み込みエラーに含まれるストリーム reset のエラーコードを取り出す
fn stream_reset_code(error: &std::io::Error) -> Option<u64> {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<StreamReset>())
        .map(|reset| reset.0)
}

/// 接続拒否のテストでエラーを取り出す
fn expect_connect_error(result: crate::error::Result<super::Connection>) -> crate::error::AppError {
    match result {
        Ok(_) => panic!("接続が拒否されること"),
        Err(error) => error,
    }
}

// ---------------------------------------------------------------------------
// ヘッダー / フロー制御の単体テスト
// ---------------------------------------------------------------------------

/// Extended CONNECT の必須フィールドが設定される
#[test]
fn build_connect_headers_sets_extended_connect_fields() {
    let headers = build_connect_headers("relay.example.com:4443", "/path?x=1")
        .expect("CONNECT ヘッダーの構築に成功すること");
    let value = |name: &[u8]| {
        headers
            .iter()
            .find(|header| header.name() == name)
            .map(HeaderField::value)
    };
    assert_eq!(
        value(b":method"),
        Some(b"CONNECT".as_slice()),
        ":method が CONNECT であること"
    );
    assert_eq!(
        value(b":protocol"),
        Some(WEBTRANSPORT_PROTOCOL),
        ":protocol が webtransport であること"
    );
    assert_eq!(
        value(b":scheme"),
        Some(b"https".as_slice()),
        ":scheme が https であること"
    );
    assert_eq!(
        value(b":authority"),
        Some(b"relay.example.com:4443".as_slice()),
        ":authority が接続先の authority であること"
    );
    assert_eq!(
        value(b":path"),
        Some(b"/path?x=1".as_slice()),
        ":path が接続先の path であること"
    );
}

/// MOQT のプロトコル識別子を WT-Available-Protocols で通知する
#[test]
fn build_connect_headers_offers_moqt_protocol() {
    let headers =
        build_connect_headers("example.com", "/").expect("CONNECT ヘッダーの構築に成功すること");
    let available = headers
        .iter()
        .find(|header| header.name() == b"wt-available-protocols")
        .map(HeaderField::value)
        .expect("WT-Available-Protocols が設定されること");
    // RFC 8941 の sf-string を 1 要素の List として送る
    assert_eq!(
        available, br#""moqt-22""#,
        "WT-Available-Protocols が moqt-22 であること"
    );
}

/// 2xx 応答の WT-Protocol が moqt-22 なら受理する
#[test]
fn verify_wt_protocol_accepts_moqt_22() {
    let headers = vec![HeaderField::from_static(b"wt-protocol", b"\"moqt-22\"")];
    verify_wt_protocol(&headers).expect("moqt-22 は受理されること");
}

/// WT-Protocol のパラメータは無視する (RFC 8941 / draft-ietf-webtrans-http2-15 §3.3)
#[test]
fn verify_wt_protocol_ignores_parameters() {
    let headers = vec![HeaderField::from_static(b"wt-protocol", b"\"moqt-22\";x=1")];
    verify_wt_protocol(&headers).expect("パラメータ付きでも受理されること");
}

/// WT-Protocol が無い / 別の値 / List / 非 String は交渉失敗にする
#[test]
fn verify_wt_protocol_rejects_invalid_selection() {
    for value in [
        // 別のプロトコル
        &b"\"moqt-21\""[..],
        // List (Item of String ではない)
        &b"\"moqt-22\", \"moqt-21\""[..],
        // 非 String
        &b"token"[..],
    ] {
        let headers =
            vec![HeaderField::new(b"wt-protocol", value).expect("ヘッダーの構築に成功すること")];
        verify_wt_protocol(&headers).expect_err("交渉に失敗すること");
    }
    verify_wt_protocol(&[]).expect_err("WT-Protocol 無しは拒否されること");
}

/// SETTINGS に WebTransport の初期フロー制御値を設定する
#[test]
fn build_limits_advertises_webtransport_settings() {
    let config = build_wt_config();
    let limits = build_limits(&config).expect("Limits の構築に成功すること");
    assert!(
        limits.enable_connect_protocol(),
        "SETTINGS_ENABLE_CONNECT_PROTOCOL を広告すること"
    );
    assert!(limits.wt_enabled(), "SETTINGS_WT_ENABLED を広告すること");
    assert_eq!(
        limits.wt_initial_max_data(),
        Some(WT_INITIAL_MAX_DATA as u32),
        "WT のセッション受信ウィンドウを広告すること"
    );
    assert_eq!(
        limits.wt_initial_max_stream_data_uni(),
        Some(WT_INITIAL_MAX_STREAM_DATA as u32),
        "WT の単方向ストリーム受信ウィンドウを広告すること"
    );
    assert_eq!(
        limits.wt_initial_max_streams_uni(),
        Some(WT_INITIAL_MAX_STREAMS_UNI as u32),
        "WT の単方向ストリーム数を広告すること"
    );
    assert_eq!(
        limits.wt_initial_max_streams_bidi(),
        Some(WT_INITIAL_MAX_STREAMS_BIDI as u32),
        "WT の双方向ストリーム数を広告すること"
    );
    assert_eq!(
        limits.initial_window_size().get(),
        H2_INITIAL_WINDOW_SIZE,
        "HTTP/2 のストリーム受信ウィンドウを広告すること"
    );
}

/// 受信ウィンドウの拡張量を計算する
#[test]
fn receive_window_increment_grows_only_when_credit_runs_low() {
    // 残り枠が初期ウィンドウの半分以上なら拡張しない
    assert_eq!(receive_window_increment(100, 0, 0, 0), None);
    assert_eq!(receive_window_increment(100, 10, 10, 0), None);
    // 残り枠が半分を切ったら、消費済みでまだ再広告していない分を返す
    assert_eq!(receive_window_increment(100, 60, 60, 0), Some(60));
    // 追加広告した分は残り枠に戻っている
    assert_eq!(receive_window_increment(100, 60, 60, 60), None);
    assert_eq!(receive_window_increment(100, 160, 160, 60), Some(100));
    // 初期ウィンドウ 0 はフロー制御を行わない
    assert_eq!(receive_window_increment(0, 10, 10, 0), None);
}

/// セッションエラーの種類が HTTP/2 の RST_STREAM コードへ対応付くこと
#[test]
fn wt_http2_error_code_maps_session_errors() {
    assert_eq!(
        wt_http2_error_code(WtErrorKind::StreamStateError),
        Some(ErrorCode::WtStreamStateError),
        "ストリーム状態エラーは WT_STREAM_STATE_ERROR にすること"
    );
    assert_eq!(
        wt_http2_error_code(WtErrorKind::FlowControlError),
        Some(ErrorCode::WtFlowControlError),
        "フロー制御エラーは WT_FLOW_CONTROL_ERROR にすること"
    );
    assert_eq!(
        wt_http2_error_code(WtErrorKind::SessionStateError),
        Some(ErrorCode::WtError),
        "セッション状態エラーは WT_ERROR にすること"
    );
    assert_eq!(
        wt_http2_error_code(WtErrorKind::InvalidInput),
        None,
        "セッションエラーではない種類では RST_STREAM を送らないこと"
    );
}

/// MOQT のセッション終了コードを WT_CLOSE_SESSION の 32 ビットコードへ変換する
#[test]
fn session_close_code_fits_local_only_codes_into_no_error() {
    assert_eq!(session_close_code(0x0), 0x0, "NO_ERROR はそのまま送ること");
    assert_eq!(
        session_close_code(0x1234),
        0x1234,
        "32 ビットに収まるコードはそのまま送ること"
    );
    assert_eq!(
        session_close_code(u64::from(u32::MAX)),
        u32::MAX,
        "32 ビットの最大値はそのまま送ること"
    );
    // MOQT のローカル専用コード (SESSION_LOCAL_*) は wire に載せられない
    assert_eq!(
        session_close_code(0xFFFF_FFFF_FFFF_FF01),
        0,
        "32 ビットに収まらないコードは NO_ERROR にすること"
    );
}

// ---------------------------------------------------------------------------
// 実 TLS の結合テスト
// ---------------------------------------------------------------------------

/// テストサーバーがクライアントの操作を観測したことを知らせる通知
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientAction {
    /// クライアントの WT_RESET_STREAM を観測した
    Reset,
    /// クライアントの WT_STOP_SENDING を観測した
    StopSending,
}

/// WebTransport セッションを確立し、単方向 / 双方向ストリームと reset / STOP_SENDING を
/// 往復するテストサーバー
///
/// クライアントが reset / STOP_SENDING の送信直後に接続を閉じると、サーバーの受信
/// バッファに残ったデータが RST で破棄されうる。テストを決定的にするため、観測した
/// ことを `notices` でクライアントへ知らせ、クライアントは通知を待ってから閉じる。
async fn run_roundtrip_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    notices: mpsc::UnboundedSender<ClientAction>,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    // クライアント起動の単方向ストリームは複数あるため、最初に開かれたストリームだけを
    // "hello" の受信に使い、それ以降は reset 対象のデータとして分けて集める
    let mut first_uni: Option<WtStreamId> = None;
    let mut received_uni = Vec::new();
    let mut received_reset = Vec::new();
    let mut received_bidi = Vec::new();
    let mut sent_streams = false;
    let mut sent_response = false;
    let mut saw_reset = false;
    let mut saw_stop_sending = false;
    let mut saw_close = false;
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { events, ended } => {
                for event in events {
                    match event {
                        WtEvent::StreamData {
                            stream_id, data, ..
                        } => {
                            if wt_stream_id::is_bidirectional(stream_id) {
                                received_bidi.extend_from_slice(&data);
                                if !sent_response && received_bidi == b"req" {
                                    server
                                        .wt
                                        .send_stream_data(stream_id, b"res", true)
                                        .map_err(|e| {
                                            format!("双方向ストリームの応答送信に失敗しました: {e}")
                                        })?;
                                    sent_response = true;
                                }
                            } else if first_uni.is_none() || Some(stream_id) == first_uni {
                                first_uni = Some(stream_id);
                                received_uni.extend_from_slice(&data);
                                if !sent_streams && received_uni == b"hello" {
                                    // サーバー起動の単方向ストリームで "world" を送る
                                    let uni = server.wt.open_uni_stream().map_err(|e| {
                                        format!("uni ストリームの open に失敗しました: {e}")
                                    })?;
                                    server.wt.send_stream_data(uni, b"world", true).map_err(
                                        |e| format!("uni ストリームの送信に失敗しました: {e}"),
                                    )?;
                                    // サーバー起動の双方向ストリームで "bidi-from-server" を送る
                                    let bidi = server.wt.open_bidi_stream().map_err(|e| {
                                        format!("bidi ストリームの open に失敗しました: {e}")
                                    })?;
                                    server
                                        .wt
                                        .send_stream_data(bidi, b"bidi-from-server", true)
                                        .map_err(|e| {
                                            format!("bidi ストリームの送信に失敗しました: {e}")
                                        })?;
                                    // クライアントが STOP_SENDING する単方向ストリームを開く
                                    let extra = server.wt.open_uni_stream().map_err(|e| {
                                        format!("uni ストリームの open に失敗しました: {e}")
                                    })?;
                                    server.wt.send_stream_data(extra, b"extra", false).map_err(
                                        |e| format!("uni ストリームの送信に失敗しました: {e}"),
                                    )?;
                                    sent_streams = true;
                                }
                            } else {
                                received_reset.extend_from_slice(&data);
                            }
                        }
                        WtEvent::StreamReset { .. } => {
                            if !saw_reset {
                                let _ = notices.send(ClientAction::Reset);
                            }
                            saw_reset = true;
                        }
                        WtEvent::StopSending { .. } => {
                            if !saw_stop_sending {
                                let _ = notices.send(ClientAction::StopSending);
                            }
                            saw_stop_sending = true;
                        }
                        WtEvent::SessionClosed { .. } => saw_close = true,
                        _ => {}
                    }
                }
                // クライアントが接続を閉じた直後は書き出しが失敗しうる。データの到達は
                // クライアント側の assert で検証しているため、ここでは無視する
                let _ = server.flush().await;
                if ended {
                    break;
                }
            }
        }
    }
    assert_eq!(
        received_uni, b"hello",
        "クライアントの単方向ストリームのデータを受信すること"
    );
    assert_eq!(
        received_reset, b"reset-me",
        "reset 対象の単方向ストリームのデータを受信すること"
    );
    assert_eq!(
        received_bidi, b"req",
        "クライアントの双方向ストリームのデータを受信すること"
    );
    assert!(saw_reset, "クライアントの WT_RESET_STREAM を受信すること");
    assert!(
        saw_stop_sending,
        "クライアントの WT_STOP_SENDING を受信すること"
    );
    assert!(saw_close, "クライアントの WT_CLOSE_SESSION を受信すること");
    Ok(())
}

/// WebTransport over HTTP/2 の接続とストリーム操作が実 TLS 上で往復すること
#[tokio::test(flavor = "multi_thread")]
async fn session_roundtrip_over_real_tls() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let (notice_tx, mut notice_rx) = mpsc::unbounded_channel();
    let server = tokio::spawn(run_roundtrip_server(listener, server_tls, notice_tx));

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, mut bidi_acceptor, _datagram) =
            connect_client(addr, &client_tls)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();

        // クライアント -> サーバー (単方向ストリーム)
        let mut send = handle
            .open_send_stream()
            .await
            .expect("単方向ストリームを開けること");
        assert_eq!(
            send.id() % 4,
            2,
            "クライアント起動の単方向ストリームの ID であること"
        );
        send.write_all(b"hello")
            .await
            .expect("データを送信できること");
        send.finish().await.expect("FIN を送信できること");
        // FIN 済みのストリームへの送信は失敗すること
        send.write_all(b"after-fin")
            .await
            .expect_err("FIN 済みのストリームへの送信は失敗すること");

        // クライアント -> サーバー (双方向ストリーム) とサーバーからの応答
        let (mut recv, mut send) = handle
            .open_bidirectional_stream()
            .await
            .expect("双方向ストリームを開けること");
        assert_eq!(
            recv.id(),
            send.id(),
            "双方向ストリームの受信方向と送信方向の ID が一致すること"
        );
        assert_eq!(
            recv.id() % 4,
            0,
            "クライアント起動の双方向ストリームの ID であること"
        );
        send.write_all(b"req")
            .await
            .expect("双方向ストリームへデータを送信できること");
        send.finish()
            .await
            .expect("双方向ストリームの FIN を送信できること");
        assert_eq!(
            read_to_end(&mut recv)
                .await
                .expect("双方向ストリームの応答を読み込めること"),
            b"res",
            "サーバーの応答を受信すること"
        );

        // サーバー -> クライアント (単方向ストリーム)
        let mut recv = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("サーバー起動の単方向ストリームが届くこと");
        assert_eq!(
            read_to_end(&mut recv)
                .await
                .expect("サーバー起動の単方向ストリームを読み込めること"),
            b"world",
            "サーバー起動の単方向ストリームのデータを受信すること"
        );

        // サーバー -> クライアント (双方向ストリーム)
        let (mut recv, _send) = bidi_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("サーバー起動の双方向ストリームが届くこと");
        assert_eq!(
            read_to_end(&mut recv)
                .await
                .expect("サーバー起動の双方向ストリームを読み込めること"),
            b"bidi-from-server",
            "サーバー起動の双方向ストリームのデータを受信すること"
        );

        // サーバー -> クライアント (単方向 "extra") を読んでから STOP_SENDING を送る
        let mut extra = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("サーバー起動の単方向ストリームが届くこと");
        let mut buf = [0u8; 5];
        read_exact(&mut extra, &mut buf)
            .await
            .expect("extra のデータを読み込めること");
        assert_eq!(&buf, b"extra", "extra のデータを受信すること");
        drop(extra);

        // クライアント -> サーバー (WT_RESET_STREAM)
        let mut reset_send = handle
            .open_send_stream()
            .await
            .expect("単方向ストリームを開けること");
        reset_send
            .write_all(b"reset-me")
            .await
            .expect("reset 対象のデータを送信できること");
        reset_send
            .reset(0x1)
            .await
            .expect("ストリームを reset できること");

        // サーバーが reset と STOP_SENDING を観測するまで待つ。受信バッファに残った
        // データが接続終了の RST で破棄されるのを避け、テストを決定的にする
        let mut saw_reset = false;
        let mut saw_stop_sending = false;
        while !(saw_reset && saw_stop_sending) {
            match notice_rx.recv().await {
                Some(ClientAction::Reset) => saw_reset = true,
                Some(ClientAction::StopSending) => saw_stop_sending = true,
                None => panic!("サーバーが reset と STOP_SENDING を観測すること"),
            }
        }

        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// WT の送信ウィンドウを小さく広告するテストサーバー
///
/// クライアントの `write_all` はサーバーが受信ウィンドウを広げるまで停止するため、
/// フロー制御による送信の保留と再開を検証できる。
async fn run_small_window_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    expected_size: usize,
) -> std::result::Result<(), String> {
    let small = WtConfig {
        initial_max_data: 16 * 1024,
        initial_max_stream_data_uni: 8 * 1024,
        ..build_wt_config()
    };
    let mut server = TestServer::accept(listener, tls, small).await?;
    let mut received = Vec::new();
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { events, ended } => {
                for event in events {
                    if let WtEvent::StreamData { data, .. } = event {
                        received.extend_from_slice(&data);
                    }
                }
                if ended {
                    break;
                }
            }
        }
    }
    assert_eq!(
        received.len(),
        expected_size,
        "送信ウィンドウの解放後に残りのデータがすべて届くこと"
    );
    Ok(())
}

/// WT の送信ウィンドウが枯渇しても、ピアの受信で送信が再開すること
#[tokio::test(flavor = "multi_thread")]
async fn blocked_write_resumes_after_receive_window_growth() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let payload_size = 96 * 1024;
    let server = tokio::spawn(run_small_window_server(listener, server_tls, payload_size));

    timeout(TEST_TIMEOUT, async {
        let (handle, _uni_acceptor, _bidi_acceptor, _datagram) = connect_client(addr, &client_tls)
            .await
            .expect("WebTransport セッションを確立できること")
            .split();
        let payload = vec![0xABu8; payload_size];
        let mut send = handle
            .open_send_stream()
            .await
            .expect("単方向ストリームを開けること");
        // ストリームの送信ウィンドウ (8 KiB) より大きいデータを送る。サーバーが読むたびに
        // WT_MAX_DATA / WT_MAX_STREAM_DATA が届き、保留していた送信が再開する
        send.write_all(&payload)
            .await
            .expect("送信ウィンドウの解放後に送信を再開できること");
        send.finish().await.expect("FIN を送信できること");
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// クライアントの送信ストリームへ WT_STOP_SENDING を送るテストサーバー
///
/// クライアントの単方向ストリームの送信ウィンドウを 8 バイトだけ広告するため、
/// STOP_SENDING が届いた時点で未送信データが駆動タスクに残る。
async fn run_stop_sending_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    error_code: u64,
) -> std::result::Result<(), String> {
    let small = WtConfig {
        initial_max_stream_data_uni: 8,
        ..build_wt_config()
    };
    let mut server = TestServer::accept(listener, tls, small).await?;
    let mut client_uni: Option<WtStreamId> = None;
    let mut after_cancel = Vec::new();
    let mut sent_stop_sending = false;
    let mut saw_close = false;
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { events, ended } => {
                for event in events {
                    match event {
                        WtEvent::StreamOpened {
                            stream_id,
                            bidirectional: false,
                        } => {
                            client_uni.get_or_insert(stream_id);
                        }
                        WtEvent::StreamData {
                            stream_id,
                            ref data,
                            ..
                        } if Some(stream_id) == client_uni
                            && !data.is_empty()
                            && !sent_stop_sending =>
                        {
                            server
                                .wt
                                .stop_sending(stream_id, error_code)
                                .map_err(|e| format!("STOP_SENDING の送信に失敗しました: {e}"))?;
                            sent_stop_sending = true;
                            // cancel 後もセッションが継続することを示すため、別ストリームを送る
                            let alive = server.wt.open_uni_stream().map_err(|e| {
                                format!("uni ストリームの open に失敗しました: {e}")
                            })?;
                            server
                                .wt
                                .send_stream_data(alive, b"alive", true)
                                .map_err(|e| format!("uni ストリームの送信に失敗しました: {e}"))?;
                        }
                        WtEvent::StreamData {
                            stream_id,
                            ref data,
                            ..
                        } if sent_stop_sending
                            && Some(stream_id) != client_uni
                            && !data.is_empty() =>
                        {
                            after_cancel.extend_from_slice(data);
                        }
                        WtEvent::SessionClosed { .. } => saw_close = true,
                        _ => {}
                    }
                }
                let _ = server.flush().await;
                if ended {
                    break;
                }
            }
        }
    }
    assert!(
        sent_stop_sending,
        "クライアントの単方向ストリームへ WT_STOP_SENDING を送ること"
    );
    assert_eq!(
        after_cancel, b"ok",
        "cancel 後もクライアントが新しいストリームへ送信できること"
    );
    assert!(saw_close, "WT_CLOSE_SESSION を受信すること");
    Ok(())
}

/// ピアの WT_STOP_SENDING が該当ストリームだけを終端し、セッションは継続すること
#[tokio::test(flavor = "multi_thread")]
async fn peer_stop_sending_fails_writes() {
    // cancel に載せるのは MOQT §12.5 (Stream Reset Error Codes) のコードである
    let error_code = 0x1;
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_stop_sending_server(listener, server_tls, error_code));

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, _bidi_acceptor, _datagram) =
            connect_client(addr, &client_tls)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();
        let mut send = handle
            .open_send_stream()
            .await
            .expect("単方向ストリームを開けること");
        let payload = vec![0xCDu8; 64];
        // 送信ウィンドウ (8 バイト) を超えるデータを送るため、write_all は STOP_SENDING が
        // 届くまで戻らない。テスト本体を止めないよう送信はタスクへ逃がす
        let send_task = tokio::spawn(async move {
            let result = send.write_all(&payload).await;
            (send, result)
        });
        let (mut send, result) = send_task.await.expect("送信タスクの join に成功すること");
        assert_eq!(
            stream_reset_code(&result.expect_err("STOP_SENDING された送信は失敗すること")),
            Some(error_code),
            "WT_STOP_SENDING が運んだ MOQT のコードがそのまま伝わること"
        );
        // cancel 後の送信 / FIN も該当ストリームの終端として失敗すること
        assert_eq!(
            stream_reset_code(
                &send
                    .write_all(b"more")
                    .await
                    .expect_err("送信は失敗すること")
            ),
            Some(error_code),
            "cancel 後の送信がエラーコード付きで失敗すること"
        );
        assert_eq!(
            stream_reset_code(&send.finish().await.expect_err("FIN は失敗すること")),
            Some(error_code),
            "cancel 後の FIN がエラーコード付きで失敗すること"
        );

        // cancel 後も新しいストリームへは送信できること
        let mut resumed = handle
            .open_send_stream()
            .await
            .expect("cancel 後も単方向ストリームを開けること");
        resumed
            .write_all(b"ok")
            .await
            .expect("cancel 後も新しいストリームへ送信できること");
        // セッションは継続し、サーバー起動の単方向ストリームを受信できること
        let mut alive = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("サーバー起動の単方向ストリームが届くこと");
        assert_eq!(
            read_to_end(&mut alive)
                .await
                .expect("サーバー起動の単方向ストリームを読み込めること"),
            b"alive",
            "cancel 後も他のストリームを受信できること"
        );
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// サーバー起動の単方向ストリームへデータを送った直後に WT_RESET_STREAM を送るサーバー
async fn run_peer_reset_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    error_code: u64,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    let uni = server
        .wt
        .open_uni_stream()
        .map_err(|e| format!("uni ストリームの open に失敗しました: {e}"))?;
    server
        .wt
        .send_stream_data(uni, b"partial", false)
        .map_err(|e| format!("uni ストリームの送信に失敗しました: {e}"))?;
    server
        .wt
        .reset_stream(uni, error_code)
        .map_err(|e| format!("WT_RESET_STREAM の送信に失敗しました: {e}"))?;
    server.flush().await?;

    let mut saw_close = false;
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { events, ended } => {
                for event in events {
                    if let WtEvent::SessionClosed { .. } = event {
                        saw_close = true;
                    }
                }
                if ended {
                    break;
                }
            }
        }
    }
    assert!(saw_close, "WT_CLOSE_SESSION を受信すること");
    Ok(())
}

/// ピアの WT_RESET_STREAM が受信ストリームの読み込みエラーとして surface すること
#[tokio::test(flavor = "multi_thread")]
async fn peer_reset_surfaces_stream_reset_error() {
    let error_code = 0x2;
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_peer_reset_server(listener, server_tls, error_code));

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, _bidi_acceptor, _datagram) =
            connect_client(addr, &client_tls)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();
        let mut recv = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("サーバー起動の単方向ストリームが届くこと");
        let mut received = Vec::new();
        let mut buf = [0u8; 32];
        let error = loop {
            match recv.read(&mut buf).await {
                Ok(0) => panic!("reset されたストリームが FIN として返らないこと"),
                Ok(size) => received.extend_from_slice(&buf[..size]),
                Err(error) => break error,
            }
        };
        assert_eq!(
            received, b"partial",
            "reset の前に送られたデータを受信できること"
        );
        assert_eq!(
            stream_reset_code(&error),
            Some(error_code),
            "WT_RESET_STREAM のエラーコードが読み込みエラーに載ること"
        );
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// datagram を送ってから単方向ストリームを送るサーバー
async fn run_datagram_server(
    listener: TcpListener,
    tls: TlsAcceptor,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    // datagram を送っても接続とストリームが壊れないことを確かめる
    server
        .wt
        .send_datagram(b"unused-datagram")
        .map_err(|e| format!("datagram の送信に失敗しました: {e}"))?;
    let uni = server
        .wt
        .open_uni_stream()
        .map_err(|e| format!("uni ストリームの open に失敗しました: {e}"))?;
    server
        .wt
        .send_stream_data(uni, b"after-datagram", true)
        .map_err(|e| format!("uni ストリームの送信に失敗しました: {e}"))?;
    server.flush().await?;

    let mut saw_close = false;
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { events, ended } => {
                for event in events {
                    if let WtEvent::SessionClosed { .. } = event {
                        saw_close = true;
                    }
                }
                if ended {
                    break;
                }
            }
        }
    }
    assert!(saw_close, "WT_CLOSE_SESSION を受信すること");
    Ok(())
}

/// 受信した WT_DATAGRAM が datagram 受信口へ渡り、接続とストリームが壊れないこと
#[tokio::test(flavor = "multi_thread")]
async fn received_datagram_is_forwarded() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_datagram_server(listener, server_tls));

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, _bidi_acceptor, mut datagram) =
            connect_client(addr, &client_tls)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();
        let datagram = datagram.recv().await.expect("datagram を受信できること");
        assert_eq!(
            datagram,
            b"unused-datagram".to_vec(),
            "受信した datagram がそのまま渡ること"
        );
        let mut recv = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("datagram の後の単方向ストリームが届くこと");
        assert_eq!(
            read_to_end(&mut recv)
                .await
                .expect("datagram の後のストリームを読み込めること"),
            b"after-datagram",
            "datagram を受信してもストリームを受信できること"
        );
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// CONNECT に応答するだけで終了するテストサーバー
///
/// `limits` はサーバーが広告する SETTINGS、`status` は CONNECT への応答 status、
/// `include_wt_protocol` は 2xx 応答に WT-Protocol を付けるかどうかである。
async fn run_connect_only_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    limits: Limits,
    status: u16,
    include_wt_protocol: bool,
) -> std::result::Result<(), String> {
    let (tcp, _) = listener
        .accept()
        .await
        .map_err(|e| format!("TCP accept に失敗しました: {e}"))?;
    let stream = tls
        .accept(tcp)
        .await
        .map_err(|e| format!("TLS accept に失敗しました: {e}"))?;
    let mut io = H2Io::server(stream, limits);
    io.initiate()
        .await
        .map_err(|e| format!("サーバーの HTTP/2 初期化に失敗しました: {e}"))?;

    loop {
        match io.next_event().await {
            Ok(Event::HeadersReceived { stream_id, .. }) => {
                let mut response = vec![
                    HeaderField::new(b":status", status.to_string())
                        .map_err(|e| format!(":status の構築に失敗しました: {e}"))?,
                ];
                if (200..300).contains(&status) && include_wt_protocol {
                    let wt_protocol = shiguredo_http2::webtransport::serialize_wt_protocol(ALPN)
                        .map_err(|e| format!("WT-Protocol のシリアライズに失敗しました: {e}"))?;
                    response.push(
                        HeaderField::new(b"wt-protocol", wt_protocol)
                            .map_err(|e| format!("WT-Protocol の構築に失敗しました: {e}"))?,
                    );
                }
                io.conn
                    .send_response(stream_id, response, !(200..300).contains(&status))
                    .map_err(|e| format!("CONNECT 応答の送信に失敗しました: {e}"))?;
                io.flush()
                    .await
                    .map_err(|e| format!("CONNECT 応答の書き出しに失敗しました: {e}"))?;
                return Ok(());
            }
            Ok(_) => {}
            // クライアントがエラーで切断した場合は正常終了として扱う
            Err(_) => return Ok(()),
        }
    }
}

/// 2xx 以外の応答は接続失敗として status を報告すること
#[tokio::test(flavor = "multi_thread")]
async fn connect_reports_rejection_status() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let limits = build_limits(&build_wt_config()).expect("サーバーの Limits の構築に成功すること");
    let server = tokio::spawn(run_connect_only_server(
        listener, server_tls, limits, 403, false,
    ));

    let error = expect_connect_error(connect_client(addr, &client_tls).await);
    assert!(
        error.to_string().contains("status=403"),
        "拒否 status をエラーに含めること: {error}"
    );

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// SETTINGS で WebTransport の対応を宣言しないサーバーとは接続できないこと
#[tokio::test(flavor = "multi_thread")]
async fn connect_requires_webtransport_settings() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    // SETTINGS_ENABLE_CONNECT_PROTOCOL だけを広告し、SETTINGS_WT_ENABLED は広告しない
    let limits = Limits::builder()
        .enable_connect_protocol(true)
        .build()
        .expect("サーバーの Limits の構築に成功すること");
    let server = tokio::spawn(run_connect_only_server(
        listener, server_tls, limits, 200, true,
    ));

    let error = expect_connect_error(connect_client(addr, &client_tls).await);
    assert!(
        error.to_string().contains("does not support WebTransport"),
        "WebTransport 非対応のエラーになること: {error}"
    );

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// 2xx でも WT-Protocol が無い応答はプロトコル交渉の失敗にすること
#[tokio::test(flavor = "multi_thread")]
async fn connect_requires_wt_protocol_selection() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let limits = build_limits(&build_wt_config()).expect("サーバーの Limits の構築に成功すること");
    let server = tokio::spawn(run_connect_only_server(
        listener, server_tls, limits, 200, false,
    ));

    let error = expect_connect_error(connect_client(addr, &client_tls).await);
    assert!(
        error.to_string().contains("moqt-22"),
        "moqt-22 の交渉失敗のエラーになること: {error}"
    );

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// クライアントの受信ウィンドウを超えるデータを送るテストサーバー
///
/// クライアントは初期ストリーム受信ウィンドウ (16 MiB) を超えた分を読むと
/// WT_MAX_STREAM_DATA を返すため、サーバーはその空きに応じて送信を再開する。
async fn run_large_payload_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    payload: Vec<u8>,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    let uni = server
        .wt
        .open_uni_stream()
        .map_err(|e| format!("uni ストリームの open に失敗しました: {e}"))?;
    let mut offset = 0;
    loop {
        // 送信ウィンドウの空きに応じて送る
        loop {
            let stream_available = server
                .wt
                .stream(uni)
                .map_or(0, |stream| stream.send_available());
            let available = stream_available.min(server.wt.flow_control().send_available());
            if available == 0 {
                break;
            }
            let take = (available as usize)
                .min(payload.len() - offset)
                .min(WT_SEND_CHUNK_SIZE);
            let end = offset + take;
            let fin = end == payload.len();
            server
                .wt
                .send_stream_data(uni, &payload[offset..end], fin)
                .map_err(|e| format!("uni ストリームの送信に失敗しました: {e}"))?;
            offset = end;
            if fin {
                break;
            }
        }
        server.flush().await?;
        if offset == payload.len() {
            break;
        }
        // クライアントの WT_MAX_STREAM_DATA を受けて送信を再開する
        match server.step().await? {
            ServerStep::Closed => {
                return Err("クライアントが接続を閉じてしまいました".to_string());
            }
            ServerStep::Events { .. } => {}
        }
    }

    // クライアントの WT_CLOSE_SESSION を待つ
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { ended, .. } => {
                if ended {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// クライアントの受信ウィンドウの拡張で大容量のデータを受信できること
#[tokio::test(flavor = "multi_thread")]
async fn large_payload_received_across_receive_window_growth() {
    // クライアントの初期ストリーム受信ウィンドウ (16 MiB) を超えるサイズにする
    let payload_size = 18 * 1024 * 1024;
    let payload = vec![0x5Au8; payload_size];
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_large_payload_server(listener, server_tls, payload));

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, _bidi_acceptor, _datagram) =
            connect_client(addr, &client_tls)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();
        let mut recv = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("サーバー起動の単方向ストリームが届くこと");
        let received = read_to_end(&mut recv)
            .await
            .expect("受信ウィンドウの拡張で大容量のデータを受信できること");
        assert_eq!(
            received.len(),
            payload_size,
            "すべてのデータを受信できること"
        );
        assert!(
            received.iter().all(|&byte| byte == 0x5A),
            "受信したデータが送信した内容と一致すること"
        );
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// クライアントのストリーム数上限を小さく広告するテストサーバー
///
/// クライアントが開いたストリームが FIN で閉じるたびに WT_MAX_STREAMS を 1 増やし、
/// 保留されていたストリームの open が再開できることを検証する。
async fn run_stream_limit_server(
    listener: TcpListener,
    tls: TlsAcceptor,
) -> std::result::Result<(), String> {
    let small = WtConfig {
        initial_max_streams_uni: 1,
        ..build_wt_config()
    };
    let mut server = TestServer::accept(listener, tls, small).await?;
    let mut received = Vec::new();
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { events, ended } => {
                for event in events {
                    match event {
                        WtEvent::StreamData { ref data, fin, .. } => {
                            received.extend_from_slice(data);
                            if fin {
                                // クライアントの 1 本が閉じたので、次の 1 本を開けるよう
                                // 上限を増やす
                                server.wt.grow_max_streams(1, false).map_err(|e| {
                                    format!("WT_MAX_STREAMS の送信に失敗しました: {e}")
                                })?;
                            }
                        }
                        WtEvent::SessionClosed { .. } => {}
                        _ => {}
                    }
                }
                // WT_MAX_STREAMS はイベント処理の後に生成されるため、次の読み込みで
                // ブロックする前に明示的に流す。クライアントが接続を閉じた直後は
                // 失敗しうるため、書き出しの失敗は無視する
                let _ = server.flush().await;
                if ended {
                    break;
                }
            }
        }
    }
    assert_eq!(
        received, b"onetwo",
        "上限の解放後に開いたストリームのデータも受信すること"
    );
    Ok(())
}

/// ピアの WT_MAX_STREAMS で保留していたストリームの open が再開すること
#[tokio::test(flavor = "multi_thread")]
async fn open_stream_resumes_after_stream_limit_growth() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_stream_limit_server(listener, server_tls));

    timeout(TEST_TIMEOUT, async {
        let (handle, _uni_acceptor, _bidi_acceptor, _datagram) = connect_client(addr, &client_tls)
            .await
            .expect("WebTransport セッションを確立できること")
            .split();
        // サーバーは単方向ストリームを 1 本しか許さない。1 本目を閉じると
        // WT_MAX_STREAMS が届き、2 本目の open が再開する
        let mut first = timeout(Duration::from_secs(5), handle.open_send_stream())
            .await
            .expect("1 本目の open がタイムアウトしないこと")
            .expect("1 本目の単方向ストリームを開けること");
        timeout(Duration::from_secs(5), first.write_all(b"one"))
            .await
            .expect("1 本目の write がタイムアウトしないこと")
            .expect("1 本目へ送信できること");
        timeout(Duration::from_secs(5), first.finish())
            .await
            .expect("1 本目の finish がタイムアウトしないこと")
            .expect("1 本目の FIN を送信できること");

        let mut second = timeout(Duration::from_secs(5), handle.open_send_stream())
            .await
            .expect("2 本目の open がタイムアウトしないこと")
            .expect("上限の解放後に 2 本目の単方向ストリームを開けること");
        timeout(Duration::from_secs(5), second.write_all(b"two"))
            .await
            .expect("2 本目の write がタイムアウトしないこと")
            .expect("2 本目へ送信できること");
        timeout(Duration::from_secs(5), second.finish())
            .await
            .expect("2 本目の finish がタイムアウトしないこと")
            .expect("2 本目の FIN を送信できること");
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// 単方向ストリームを送ってから接続を閉じるサーバー
async fn run_peer_close_server(
    listener: TcpListener,
    tls: TlsAcceptor,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    let uni = server
        .wt
        .open_uni_stream()
        .map_err(|e| format!("uni ストリームの open に失敗しました: {e}"))?;
    server
        .wt
        .send_stream_data(uni, b"bye", true)
        .map_err(|e| format!("uni ストリームの送信に失敗しました: {e}"))?;
    server.flush().await?;
    // クライアントが読み終えるのを待たずに接続を閉じる
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

/// ピアが接続を閉じると acceptor が `Ok(None)` を返し、新しいストリームを開けなくなること
#[tokio::test(flavor = "multi_thread")]
async fn peer_close_ends_acceptors() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_peer_close_server(listener, server_tls));

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, _bidi_acceptor, _datagram) =
            connect_client(addr, &client_tls)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();
        let mut recv = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("ピア起動の単方向ストリームが届くこと");
        assert_eq!(
            read_to_end(&mut recv)
                .await
                .expect("接続終了前のストリームを読み込めること"),
            b"bye",
            "接続終了前のデータを受信できること"
        );

        // 接続が閉じると acceptor は Ok(None) を返す
        loop {
            if uni_acceptor
                .accept()
                .await
                .expect("accept はエラーにならないこと")
                .is_none()
            {
                break;
            }
        }
        // 接続終了後は新しいストリームを開けない
        assert!(
            handle.open_send_stream().await.is_err(),
            "接続終了後はストリームを開けないこと"
        );
        // 終了済みの接続を閉じても panic しない
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// ハンドルがすべて破棄されたらピアへ終了を伝えて接続を閉じるサーバー
async fn run_drop_server(
    listener: TcpListener,
    tls: TlsAcceptor,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    let mut saw_close = false;
    loop {
        match server.step().await? {
            ServerStep::Closed => {
                return Err("クライアントが END_STREAM を送らずに接続を閉じました".to_string());
            }
            ServerStep::Events { events, ended } => {
                for event in events {
                    if let WtEvent::SessionClosed { .. } = event {
                        saw_close = true;
                    }
                }
                if ended {
                    break;
                }
            }
        }
    }
    assert!(saw_close, "ハンドル全破棄時に WT_CLOSE_SESSION を送ること");
    Ok(())
}

/// ハンドルをすべて破棄すると駆動タスクが接続を閉じること
#[tokio::test(flavor = "multi_thread")]
async fn dropping_all_handles_closes_connection() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_drop_server(listener, server_tls));

    let connection = timeout(TEST_TIMEOUT, connect_client(addr, &client_tls))
        .await
        .expect("テストがタイムアウトしないこと")
        .expect("WebTransport セッションを確立できること");
    let (handle, recv_acceptor, bidi_acceptor, _datagram) = connection.split();
    drop(handle);
    drop(recv_acceptor);
    drop(bidi_acceptor);

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// keep-alive の HTTP/2 PING を観測するテストサーバー
async fn run_keep_alive_server(
    listener: TcpListener,
    tls: TlsAcceptor,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    loop {
        match server.step().await? {
            ServerStep::Closed => break,
            ServerStep::Events { ended, .. } => {
                if ended {
                    break;
                }
            }
        }
    }
    assert!(
        server.ping_count.load(Ordering::Relaxed) >= 2,
        "keep-alive の HTTP/2 PING を 2 回以上受信すること"
    );
    Ok(())
}

/// keep-alive が HTTP/2 PING を定期的に送ること
#[tokio::test(flavor = "multi_thread")]
async fn keep_alive_sends_periodic_pings() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_keep_alive_server(listener, server_tls));

    timeout(TEST_TIMEOUT, async {
        let (handle, _uni_acceptor, _bidi_acceptor, _datagram) = connect_client(addr, &client_tls)
            .await
            .expect("WebTransport セッションを確立できること")
            .split();
        // 本番は 5 秒間隔だが、テストでは 50 ms に差し替えて短時間で観測する
        handle.enable_keep_alive_with_interval(Duration::from_millis(50));
        // 2 回以上の PING (50 ms × 2) が届く時間を待ってから閉じる
        tokio::time::sleep(Duration::from_millis(400)).await;
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// WT_CLOSE_SESSION を送り、クライアントの END_STREAM 応答を待つテストサーバー
async fn run_peer_session_close_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    error_code: u32,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    // draft-ietf-webtrans-http2-15 §6.12: WT_CLOSE_SESSION を送ってセッションを閉じる
    server
        .wt
        .close(error_code, "bye")
        .map_err(|e| format!("WT_CLOSE_SESSION の送信に失敗しました: {e}"))?;
    server.flush().await?;

    // クライアントは CONNECT ストリームを END_STREAM で閉じる (MUST)
    loop {
        match server.step().await? {
            ServerStep::Closed => {
                return Err(
                    "クライアントが CONNECT ストリームを END_STREAM で閉じませんでした".to_string(),
                );
            }
            ServerStep::Events { ended, .. } => {
                if ended {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// ピアの WT_CLOSE_SESSION に END_STREAM で応答すること
#[tokio::test(flavor = "multi_thread")]
async fn peer_session_close_gets_end_stream_response() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_peer_session_close_server(listener, server_tls, 0x10));

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, mut bidi_acceptor, _datagram) =
            connect_client(addr, &client_tls)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();
        // ピアの WT_CLOSE_SESSION で駆動タスクが CONNECT ストリームを閉じて終了する
        assert!(
            uni_acceptor
                .accept()
                .await
                .expect("accept はエラーにならないこと")
                .is_none(),
            "ピアのセッション終了で単方向 acceptor が Ok(None) を返すこと"
        );
        assert!(
            bidi_acceptor
                .accept()
                .await
                .expect("accept はエラーにならないこと")
                .is_none(),
            "ピアのセッション終了で双方向 acceptor が Ok(None) を返すこと"
        );
        assert!(
            handle.open_send_stream().await.is_err(),
            "セッション終了後はストリームを開けないこと"
        );
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// WT_CLOSE_SESSION のエラーコードを観測するテストサーバー
async fn run_close_code_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    expected_code: u32,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    let mut close_code = None;
    loop {
        match server.step().await? {
            ServerStep::Closed => {
                return Err(
                    "クライアントが WT_CLOSE_SESSION を送らずに接続を閉じました".to_string()
                );
            }
            ServerStep::Events { events, ended } => {
                for event in events {
                    if let WtEvent::SessionClosed { error_code, .. } = event {
                        close_code = Some(error_code);
                    }
                }
                if ended {
                    break;
                }
            }
        }
    }
    assert_eq!(
        close_code,
        Some(expected_code),
        "WT_CLOSE_SESSION が MOQT のセッション終了コードを運ぶこと"
    );
    Ok(())
}

/// `Handle::close` が MOQT のセッション終了コードを WT_CLOSE_SESSION で送ること
#[tokio::test(flavor = "multi_thread")]
async fn handle_close_sends_moqt_code() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_close_code_server(listener, server_tls, 0x1234));

    timeout(TEST_TIMEOUT, async {
        let (handle, _uni_acceptor, _bidi_acceptor, _datagram) = connect_client(addr, &client_tls)
            .await
            .expect("WebTransport セッションを確立できること")
            .split();
        handle.close(0x1234).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// STOP_SENDING 後の破棄データでセッション受信クレジットが返ることを確認するサーバー
///
/// クライアントの初期セッション受信ウィンドウを使い切る量のデータを、WT_STOP_SENDING を
/// 受け取った後に `WT_STREAM` capsule として直接送る。クライアントが破棄した分の
/// クレジットを返すと `WT_MAX_DATA` が届いて送信可能枠が広がり、返さない場合は広がらない。
async fn run_discarded_data_credit_server(
    listener: TcpListener,
    tls: TlsAcceptor,
    stream_window: u64,
) -> std::result::Result<(), String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    // クライアントが受け入れてすぐ破棄する単方向ストリームを開き、1 バイトだけ送る
    let uni = server
        .wt
        .open_uni_stream()
        .map_err(|e| format!("uni ストリームの open に失敗しました: {e}"))?;
    server
        .wt
        .send_stream_data(uni, b"x", false)
        .map_err(|e| format!("uni ストリームの送信に失敗しました: {e}"))?;
    server.flush().await?;

    // クライアントの WT_STOP_SENDING を待つ。この間 WT 出力は流さないため、
    // STOP_SENDING への自動 WT_RESET_STREAM はクライアントへ届かない
    server.wait_for_stop_sending(uni).await?;
    // 自動 WT_RESET_STREAM は、この後で注入するデータと reliable size が食い違うため
    // クライアントへ送らない (WT セッションの出力バッファから捨てる)
    while server.wt.poll_output().is_some() {}

    // クライアントのストリーム受信ウィンドウの残りを一度に送る。これはクライアントの
    // セッション受信ウィンドウも使い切る量であり、破棄分のクレジットが返らないと
    // 以後の送信ができなくなる
    let baseline = server.wt.flow_control().send_available();
    let remaining = stream_window - 1;
    server
        .inject_stream_data(uni, &vec![0xa5u8; remaining as usize])
        .await?;

    // 破棄したデータの WT_MAX_DATA が届くと、サーバーの送信可能枠が広がる
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if server.wt.flow_control().send_available() > baseline {
            break;
        }
        match tokio::time::timeout_at(deadline, server.step()).await {
            Ok(Ok(ServerStep::Events { ended, .. })) => {
                if ended {
                    return Err("クレジットが返る前に CONNECT ストリームが終了しました".to_string());
                }
            }
            Ok(Ok(ServerStep::Closed)) => {
                return Err("クレジットが返る前に接続が終了しました".to_string());
            }
            Ok(Err(error)) => return Err(error),
            // 期限までに WT_MAX_DATA が届かなかった
            Err(_) => {
                return Err(
                    "破棄したデータのセッション受信クレジットが返りませんでした".to_string()
                );
            }
        }
    }
    Ok(())
}

/// WT_STOP_SENDING 後の破棄データでもセッション受信クレジットが返ること
#[tokio::test(flavor = "multi_thread")]
async fn discarded_data_after_stop_sending_returns_session_credit() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    // クライアントの初期受信ウィンドウを小さくして、破棄データによる枯渇を短時間で再現する
    let session_window = 8 * 1024;
    let stream_window = 8 * 1024;
    let server = tokio::spawn(run_discarded_data_credit_server(
        listener,
        server_tls,
        stream_window,
    ));
    let wt_config = WtConfig {
        initial_max_data: session_window,
        initial_max_stream_data_uni: stream_window,
        ..build_wt_config()
    };

    timeout(TEST_TIMEOUT, async {
        let (handle, mut uni_acceptor, _bidi_acceptor, _datagram) =
            connect_client_with_wt_config(addr, &client_tls, &wt_config)
                .await
                .expect("WebTransport セッションを確立できること")
                .split();
        let stream = uni_acceptor
            .accept()
            .await
            .expect("accept に成功すること")
            .expect("サーバー起動の単方向ストリームが届くこと");
        // 受信方向を放棄する。以後に届く飛行中のデータは駆動タスクが破棄する
        drop(stream);
        // サーバーがクレジットの回復を観測して接続を閉じるまで待つ
        while uni_acceptor
            .accept()
            .await
            .expect("accept はエラーにならないこと")
            .is_some()
        {}
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");

    server
        .await
        .expect("サーバータスクの join に成功すること")
        .expect("サーバーがエラーなく終了すること");
}

/// datagram を 1 つ受け取るまでサーバーを駆動する
async fn run_datagram_receiver_server(
    listener: TcpListener,
    tls: TlsAcceptor,
) -> std::result::Result<Vec<u8>, String> {
    let mut server = TestServer::accept(listener, tls, build_wt_config()).await?;
    loop {
        match server.step().await? {
            ServerStep::Closed => {
                return Err("datagram を受け取る前に接続が閉じました".to_string());
            }
            ServerStep::Events { events, ended } => {
                for event in events {
                    if let WtEvent::DatagramReceived { data } = event {
                        return Ok(data);
                    }
                }
                if ended {
                    return Err("datagram を受け取る前に接続が終了しました".to_string());
                }
            }
        }
    }
}

/// クライアントから送った datagram が `WT_DATAGRAM` capsule で届くこと
#[tokio::test(flavor = "multi_thread")]
async fn send_datagram_over_http2() {
    let (listener, addr) = bind_listener().await;
    let (server_tls, client_tls) = test_tls_pair();
    let server = tokio::spawn(run_datagram_receiver_server(listener, server_tls));

    timeout(TEST_TIMEOUT, async {
        let (handle, _uni_acceptor, _bidi_acceptor, _datagram) = connect_client(addr, &client_tls)
            .await
            .expect("WebTransport セッションを確立できること")
            .split();
        let payload = b"client datagram";
        handle
            .send_datagram(payload)
            .await
            .expect("datagram を送信できること");
        let received = server
            .await
            .expect("サーバータスクの join に成功すること")
            .expect("サーバーが datagram を受け取ること");
        assert_eq!(received, payload, "送信した datagram が受信できること");
        handle.close(0).await;
    })
    .await
    .expect("テストがタイムアウトしないこと");
}
