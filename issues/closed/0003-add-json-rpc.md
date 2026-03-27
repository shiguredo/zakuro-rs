# JSON-RPC 2.0 /rpc エンドポイントを追加する

Created: 2026-03-27
Completed: 2026-03-27
Model: Opus 4.6

## 概要

HTTP サーバー上に JSON-RPC 2.0 準拠の `POST /rpc` エンドポイントを追加する。最初のメソッドとして `GetVersion` を実装する。

## 根拠

zakuro (C++) では PR #73 で JSON-RPC 2.0 の `/rpc` エンドポイントが追加されている。RPC 経由で zakuro の状態取得や操作を行うための基盤であり、外部ツールやテストフレームワークとの連携に必要。C++ 版との機能互換性を維持するために対応する。

## 参考

- https://github.com/shiguredo/zakuro/pull/73

## 対応内容

### 1. JSON-RPC モジュール

- JSON-RPC 2.0 仕様に準拠したリクエストパース・レスポンス生成を実装する
- メソッドディスパッチ機構を実装する
- Notification (id なし) の場合はレスポンスを返さない
- エラーレスポンス (Parse error, Method not found 等) を実装する

### 2. GetVersion メソッド

- zakuro-rs のバージョン情報を返す

### 3. HTTP サーバーとの統合

- `POST /rpc` エンドポイントを追加する
- Content-Type: application/json のバリデーション

## 解決方法

`src/json_rpc.rs` を新規作成し、JSON-RPC 2.0 仕様に準拠したリクエスト処理を実装した。`nojson::RawJson` で JSON パース、メソッドディスパッチ、Notification 対応、エラーレスポンス生成を含む。`GetVersion` メソッドでバージョン情報を返す。`src/http_server.rs` の `DefaultHandler` に `POST /rpc` エンドポイントを追加した。

## 依存

- 0002-add-http-server
