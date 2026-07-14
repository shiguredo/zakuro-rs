# mTLS 対応 (`--client-cert`, `--client-key`) を追加する

Created: 2026-03-27
Completed: 2026-07-14
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

## 解決方法

`src/args.rs` の `CommonArgs` に `--client-cert` / `--client-key` を追加し、ファイル存在チェックと両方指定必須のバリデーションを入れた。JSONC 最上位の同キーも `is_common_key` 経由で CommonArgs に載る。

`src/main.rs` で PEM を 1 度読み込み、各インスタンスの `VirtualClientConfig` に渡す。`src/virtual_client.rs` では `SoraConnectionBuilder::client_cert` に設定し、sora-rust-sdk 内の rustls `with_client_auth_cert` でシグナリング WebSocket の TLS に反映する。

プロセス共通の証明書のみ対応する (per-instance 証明書は持たない)。`--insecure` との併用も可能。
