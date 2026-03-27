# mTLS 対応 (`--client-cert`, `--client-key`) を追加する

Created: 2026-03-27
Model: Opus 4.6

## 概要

`--client-cert` と `--client-key` オプションでクライアント証明書と秘密鍵を指定し、Sora シグナリング接続で mTLS を利用できるようにする。

## 根拠

zakuro (C++) では `--client-cert` と `--client-key` で mTLS が利用できる。セキュリティ要件の厳しい環境での負荷試験に必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. コマンドライン引数

- `--client-cert <FILE>` オプションを追加する (PEM 形式のクライアント証明書)
- `--client-key <FILE>` オプションを追加する (PEM 形式の秘密鍵)

### 2. TLS 設定

- rustls でクライアント証明書を設定する
- Sora SDK の接続設定に反映する
