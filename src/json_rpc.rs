use nojson::{JsonValueKind, RawJson};
use shiguredo_http11::Response;

use crate::http_server::HttpRequest;

/// JSON-RPC 2.0 エラーコード
const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;

/// JSON-RPC 2.0 リクエストを処理して HTTP レスポンスを返す
pub(crate) fn handle_rpc(request: &HttpRequest) -> Response {
    let body = match std::str::from_utf8(&request.body) {
        Ok(s) => s,
        Err(_) => return rpc_error_response(PARSE_ERROR, "Parse error", "null"),
    };

    let json = match RawJson::parse(body) {
        Ok(v) => v,
        Err(_) => return rpc_error_response(PARSE_ERROR, "Parse error", "null"),
    };

    let root = json.value();
    if root.kind() != JsonValueKind::Object {
        return rpc_error_response(PARSE_ERROR, "Parse error", "null");
    }

    // jsonrpc フィールドの確認
    let jsonrpc: Option<String> = root
        .to_member("jsonrpc")
        .ok()
        .and_then(|m| m.required().ok())
        .and_then(|v| v.try_into().ok());
    if jsonrpc.as_deref() != Some("2.0") {
        return rpc_error_response(PARSE_ERROR, "Parse error", "null");
    }

    // id の取得 (Notification の場合は存在しない)
    let id_member = root.to_member("id").ok().and_then(|m| m.required().ok());
    let id_json = match id_member {
        Some(v) => v.as_raw_str().to_string(),
        None => {
            // Notification: レスポンスを返さない
            return Response::new(204, "No Content")
                .expect("static response 204 should not fail")
                .header("Content-Length", "0")
                .expect("static header should not fail")
                .header("Connection", "close")
                .expect("static header should not fail");
        }
    };

    // method の取得
    let method: Option<String> = root
        .to_member("method")
        .ok()
        .and_then(|m| m.required().ok())
        .and_then(|v| v.try_into().ok());
    let method = match method {
        Some(m) => m,
        None => return rpc_error_response(PARSE_ERROR, "Parse error", &id_json),
    };

    // メソッドディスパッチ
    match method.as_str() {
        "GetVersion" => {
            let result = format!(
                r#"{{"name":"{}","version":"{}"}}"#,
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            );
            rpc_success_response(&id_json, &result)
        }
        _ => rpc_error_response(METHOD_NOT_FOUND, "Method not found", &id_json),
    }
}

/// JSON-RPC 2.0 成功レスポンスを生成する
fn rpc_success_response(id_json: &str, result: &str) -> Response {
    let body = format!(r#"{{"jsonrpc":"2.0","result":{result},"id":{id_json}}}"#);
    Response::new(200, "OK")
        .expect("static response 200 should not fail")
        .header("Content-Type", "application/json")
        .expect("static header should not fail")
        .header("Connection", "close")
        .expect("static header should not fail")
        .body(body.into_bytes())
}

/// JSON-RPC 2.0 エラーレスポンスを生成する
fn rpc_error_response(code: i64, message: &str, id_json: &str) -> Response {
    let body = format!(
        r#"{{"jsonrpc":"2.0","error":{{"code":{code},"message":"{message}"}},"id":{id_json}}}"#
    );
    Response::new(200, "OK")
        .expect("static response 200 should not fail")
        .header("Content-Type", "application/json")
        .expect("static header should not fail")
        .header("Connection", "close")
        .expect("static header should not fail")
        .body(body.into_bytes())
}
