use shiguredo_http11::{RequestDecoder, Response};
use shiguredo_webrtc::{rtc_log_info, rtc_log_warning};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// HTTP リクエストのルーティング結果
#[allow(dead_code)]
pub(crate) struct HttpRequest {
    pub(crate) method: String,
    pub(crate) uri: String,
    pub(crate) body: Vec<u8>,
}

/// HTTP リクエストハンドラ
///
/// ルーティングとレスポンス生成を担当するトレイト。
/// JSON-RPC やヘルスチェックなどのエンドポイントはこのトレイトを実装する。
pub(crate) trait HttpHandler: Send + Sync + 'static {
    fn handle(&self, request: &HttpRequest) -> Response;
}

/// デフォルトハンドラ
///
/// ヘルスチェックと JSON-RPC エンドポイントを提供する。
#[derive(Clone)]
pub(crate) struct DefaultHandler;

impl HttpHandler for DefaultHandler {
    fn handle(&self, request: &HttpRequest) -> Response {
        match (request.method.as_str(), request.uri.as_str()) {
            // ヘルスチェック
            ("GET", "/.ok") => Response::new(200, "OK")
                .header("Content-Length", "0")
                .header("Connection", "close"),
            // JSON-RPC 2.0
            ("POST", "/rpc") => crate::json_rpc::handle_rpc(request),
            _ => Response::new(404, "Not Found")
                .header("Content-Length", "0")
                .header("Connection", "close"),
        }
    }
}

/// HTTP サーバー
pub(crate) struct HttpServer {
    listener: TcpListener,
    token: CancellationToken,
}

impl HttpServer {
    /// HTTP サーバーを起動する
    pub(crate) async fn bind(
        host: &str,
        port: u16,
        token: CancellationToken,
    ) -> std::io::Result<Self> {
        let addr = format!("{host}:{port}");
        let listener = TcpListener::bind(&addr).await?;
        rtc_log_info!("HTTP server listening on {}", addr);
        Ok(Self { listener, token })
    }

    /// 接続を受け付けてリクエストを処理するループ
    pub(crate) async fn run(self, handler: impl HttpHandler + Clone) {
        loop {
            tokio::select! {
                biased;
                _ = self.token.cancelled() => {
                    rtc_log_info!("HTTP server shutting down");
                    break;
                }
                result = self.listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let handler = handler.clone();
                            let token = self.token.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, &handler, token).await {
                                    rtc_log_warning!("HTTP connection error from {}: {}", addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            rtc_log_warning!("HTTP accept error: {}", e);
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
            let response = Response::new(400, "Bad Request")
                .header("Content-Length", "0")
                .header("Connection", "close");
            stream.write_all(&response.encode()).await?;
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
                        let response = Response::new(400, "Bad Request")
                            .header("Content-Length", "0")
                            .header("Connection", "close");
                        stream.write_all(&response.encode()).await?;
                        return Ok(());
                    }
                }
                Err(_) => {
                    let response = Response::new(400, "Bad Request")
                        .header("Content-Length", "0")
                        .header("Connection", "close");
                    stream.write_all(&response.encode()).await?;
                    return Ok(());
                }
            }
        };

        let keep_alive = request.is_keep_alive();

        let http_request = HttpRequest {
            method: request.method,
            uri: request.uri,
            body: request.body,
        };

        let response = handler.handle(&http_request);
        stream.write_all(&response.encode()).await?;

        if !keep_alive {
            break;
        }
    }

    Ok(())
}
