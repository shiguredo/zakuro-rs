//! ヘルスチェックと JSON-RPC を提供する HTTP/1.1 サーバー
//!
//! `GET /.ok` はヘルスチェック、`POST /rpc` は JSON-RPC 2.0 のエンドポイントである。

use shiguredo_http11::{RequestDecoder, Response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// HTTP リクエストのルーティング結果
pub struct HttpRequest {
    /// HTTP メソッド (`GET` / `POST` など)
    pub method: String,
    /// リクエスト URI
    pub uri: String,
    /// リクエストボディ
    pub body: Vec<u8>,
}

/// HTTP リクエストハンドラ
///
/// ルーティングとレスポンス生成を担当するトレイト。
/// JSON-RPC やヘルスチェックなどのエンドポイントはこのトレイトを実装する。
pub trait HttpHandler: Send + Sync + 'static {
    /// リクエストを処理してレスポンスを返す
    fn handle(&self, request: &HttpRequest) -> Response;
}

/// デフォルトハンドラ
///
/// ヘルスチェックと JSON-RPC エンドポイントを提供する。
#[derive(Clone)]
pub struct DefaultHandler {
    /// JSON-RPC の GetVersion が返すサービス名
    name: &'static str,
    /// JSON-RPC の GetVersion が返すバージョン
    version: &'static str,
}

impl DefaultHandler {
    /// サービス名とバージョンを指定してハンドラを作る
    ///
    /// `name` / `version` は JSON-RPC の `GetVersion` の応答に使う。クレート定数の
    /// `env!("CARGO_PKG_NAME")` はビルドしたクレートの値を返すため、共有クレート側では
    /// 使えず、バイナリから渡す必要がある。
    pub fn new(name: &'static str, version: &'static str) -> Self {
        Self { name, version }
    }
}

impl HttpHandler for DefaultHandler {
    fn handle(&self, request: &HttpRequest) -> Response {
        match (request.method.as_str(), request.uri.as_str()) {
            // ヘルスチェック
            ("GET", "/.ok") => Response::new(200, "OK")
                .expect("static response 200 should not fail")
                .header("Content-Length", "0")
                .expect("static header should not fail")
                .header("Connection", "close")
                .expect("static header should not fail"),
            // JSON-RPC 2.0
            ("POST", "/rpc") => crate::json_rpc::handle_rpc(request, self.name, self.version),
            _ => Response::new(404, "Not Found")
                .expect("static response 404 should not fail")
                .header("Content-Length", "0")
                .expect("static header should not fail")
                .header("Connection", "close")
                .expect("static header should not fail"),
        }
    }
}

/// HTTP サーバー
pub struct HttpServer {
    listener: TcpListener,
    token: CancellationToken,
}

impl HttpServer {
    /// HTTP サーバーを起動する
    pub async fn bind(host: &str, port: u16, token: CancellationToken) -> std::io::Result<Self> {
        let addr = format!("{host}:{port}");
        let listener = TcpListener::bind(&addr).await?;
        log::info!("HTTP server listening on {}", addr);
        Ok(Self { listener, token })
    }

    /// 接続を受け付けてリクエストを処理するループ
    pub async fn run(self, handler: impl HttpHandler + Clone) {
        loop {
            tokio::select! {
                biased;
                _ = self.token.cancelled() => {
                    log::info!("HTTP server shutting down");
                    break;
                }
                result = self.listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let handler = handler.clone();
                            let token = self.token.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, &handler, token).await {
                                    log::warn!("HTTP connection error from {}: {}", addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            log::warn!("HTTP accept error: {}", e);
                        }
                    }
                }
            }
        }
    }
}

/// 単一の TCP 接続を処理する
async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    handler: &impl HttpHandler,
    token: CancellationToken,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; 8192];

    loop {
        if token.is_cancelled() {
            break;
        }

        let n = tokio::select! {
            biased;
            _ = token.cancelled() => break,
            result = stream.read(&mut buf) => result?,
        };

        if n == 0 {
            break;
        }

        let mut decoder = RequestDecoder::new();
        if decoder.feed(&buf[..n]).is_err() {
            let response = bad_request_response();
            stream
                .write_all(&response.encode().expect("response encode should not fail"))
                .await?;
            break;
        }

        // リクエストが完全にデコードできるまでデータを読み続ける
        let request = loop {
            match decoder.decode() {
                Ok(Some(req)) => break req,
                Ok(None) => {
                    // データが不足しているので追加で読む
                    let n = tokio::select! {
                        biased;
                        _ = token.cancelled() => return Ok(()),
                        result = stream.read(&mut buf) => result?,
                    };
                    if n == 0 {
                        return Ok(());
                    }
                    if decoder.feed(&buf[..n]).is_err() {
                        let response = bad_request_response();
                        stream
                            .write_all(&response.encode().expect("response encode should not fail"))
                            .await?;
                        return Ok(());
                    }
                }
                Err(_) => {
                    let response = bad_request_response();
                    stream
                        .write_all(&response.encode().expect("response encode should not fail"))
                        .await?;
                    return Ok(());
                }
            }
        };

        let keep_alive = request.is_keep_alive();

        let http_request = HttpRequest {
            method: request.method().to_string(),
            uri: request.uri().to_string(),
            body: request.body_bytes().map(|b| b.to_vec()).unwrap_or_default(),
        };

        let response = handler.handle(&http_request);
        stream
            .write_all(&response.encode().expect("response encode should not fail"))
            .await?;

        if !keep_alive {
            break;
        }
    }

    Ok(())
}

/// 400 Bad Request レスポンスを生成する
fn bad_request_response() -> Response {
    Response::new(400, "Bad Request")
        .expect("static response 400 should not fail")
        .header("Content-Length", "0")
        .expect("static header should not fail")
        .header("Connection", "close")
        .expect("static header should not fail")
}
