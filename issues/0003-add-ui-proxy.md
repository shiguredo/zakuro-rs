# UI リバースプロキシ機能を追加する

Created: 2026-03-27

## 概要

`--ui` オプションを指定すると、HTTP サーバーが Zakuro UI のリバースプロキシとして動作し、ブラウザから `http://<host>:<port>/` にアクセスすると Zakuro UI が表示されるようにする。`--ui-remote-url` でリモート URL を変更可能にする。

## 根拠

zakuro (C++) では PR #75 で UI リバースプロキシ (`HttpProxy`) が追加されている。Zakuro UI をブラウザから利用できるようにすることで、負荷試験の状態をリアルタイムに可視化できる。開発時には `--ui-remote-url` でローカルの開発サーバーを指定できる。C++ 版との機能互換性を維持するために対応する。

## 参考

- https://github.com/shiguredo/zakuro/pull/75

## 対応内容

### 1. HTTP プロキシモジュール

- リモート URL へのリクエスト転送を実装する
- HTTPS (TLS 1.2/1.3) のリモート URL に対応する
- リクエストヘッダーの転送 (Host ヘッダーの書き換え)
- レスポンスの中継
- タイムアウト処理

### 2. コマンドライン引数

- `--ui` フラグを追加する (UI 機能の有効化)
- `--ui-remote-url <URL>` オプションを追加する (デフォルトの Zakuro UI URL を上書き)

### 3. HTTP サーバーとの統合

- `--ui` が有効な場合、`/rpc` と `/.ok` 以外のリクエストをリモート URL に転送する
- `--http-host` と `--http-port` が必要 (HTTP サーバーが前提)

## 依存

- 0002-add-http-server
