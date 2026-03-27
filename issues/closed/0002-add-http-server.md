# HTTP サーバーを追加する

Created: 2026-03-27
Completed: 2026-03-27
Model: Opus 4.6

## 概要

`--http-host` と `--http-port` オプションを指定すると HTTP サーバーが起動するようにする。後続の JSON-RPC やヘルスチェック機能の基盤となる。

## 根拠

zakuro (C++) では PR #72 で HTTP サーバー基盤が追加されている。zakuro-rs にも HTTP API 機能 (ヘルスチェック、JSON-RPC) を実装するためには、まず HTTP サーバー基盤が必要。C++ 版との機能互換性を維持するために対応する。

## 参考

- https://github.com/shiguredo/zakuro/pull/72

## 対応内容

### 1. HTTP サーバーモジュール

- tokio ベースの HTTP/1.1 サーバーを実装する
- `--http-host` と `--http-port` の両方が指定された場合のみ起動する
- shiguredo_http11 を使用して HTTP リクエスト/レスポンスを処理する
- グレースフルシャットダウンに対応する (CancellationToken 連携)

### 2. コマンドライン引数

- `--http-host <ADDR>` オプションを追加する
- `--http-port <PORT>` オプションを追加する
- 両方指定時のみ HTTP サーバーを起動する

### 3. main.rs の接続

- HTTP サーバーの起動処理を追加する
- シャットダウン時に HTTP サーバーも停止する

## 解決方法

`src/http_server.rs` を新規作成し、tokio の `TcpListener` + `shiguredo_http11` の `RequestDecoder`/`Response` で HTTP/1.1 サーバーを実装した。`HttpHandler` トレイトによるルーティング抽象化、`CancellationToken` 連携のグレースフルシャットダウン、Keep-Alive 対応を含む。`src/args.rs` に `--http-host` と `--http-port` オプションを追加し、両方指定時のみ起動する。
