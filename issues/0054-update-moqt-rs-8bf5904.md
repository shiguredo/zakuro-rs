# moqt-rs の rev を 8bf5904 へ更新する

- Created: 2026-11-16
- Completed: {YYYY-MM-DD}
- Branch: feature/update-moqt-rs-8bf5904
- Polished: {YYYY-MM-DD}

## 目的

`zakuro-moq` が固定している `shiguredo_moqt` の rev を現行 `develop` の最新へ更新する。
固定 rev が古いままだと、moqt-rs 側で追加された namespace 操作と SUBSCRIBE_TRACKS の
低レイヤー API、およびプロトコル実装の修正が `zakuro-moq` の負荷試験に反映されない。

## 現状

- `zakuro-moq/Cargo.toml` は `shiguredo_moqt` を rev
  `2699bd26e280accd5fe5fd8642353a59315291e9` (2026-11-07) で固定している
- 現行 `develop` の最新は `8bf5904986a13eaec935599163d8d553b4247b56` (2026-11-16) であり、
  差分は 192 コミットである。タグは無く、`8bf5904` より新しい ref も存在しない
- 差分には namespace 操作と SUBSCRIBE_TRACKS の低レイヤー API を完成させる変更が含まれる
  (draft-ietf-moq-transport-23 §9.13〜§9.18)
- `SessionEvent` に次の 6 variant が追加された
  - `PublishNamespaceReceived`
  - `SubscribeNamespaceReceived`
  - `SubscribeTracksReceived`
  - `NamespaceReceived`
  - `NamespaceDoneReceived`
  - `PublishSkippedReceived`
- `zakuro-moq` の `drain_events` は `SessionEvent` を網羅 match しているため、
  この rev ではコンパイルできない
- ALPN は `moqt-23` のままで、draft の改版は含まれない
- moqt-rs の `rust-version` は 1.93 で、zakuro-rs の MSRV 1.99 の範囲内である
- この環境には libfdk-aac の開発ヘッダーが無いため、`--features fdk-aac` を含む
  `make ci` は実行できない。fdk-aac を含む全体検証は CI で行う

## 設計方針

- rev を `8bf5904` へ更新し、`cargo update -p shiguredo_moqt` で `Cargo.lock` を再生成する
- 追加された 6 イベントは、zakuro-moq が送らない namespace 操作と SUBSCRIBE_TRACKS の
  ためのものなので、受信してもセッションの状態を変えずに無視する。到達しない想定である
  ことをコメントに残す
- 既存の PUBLISH / SUBSCRIBE 経路の挙動は変えない
- 検証は `cargo fmt --all -- --check`、`cargo clippy --locked -p zakuro-moq --all-targets -- -D warnings`、
  `cargo test --locked -p zakuro-moq`、`cargo check --locked --workspace` で行う

## 完了条件

- `shiguredo_moqt` の rev が `8bf5904` になり、`Cargo.lock` が一致していること
- `zakuro-moq` のコンパイル、clippy、テストが通ること
- ワークスペース全体が `cargo check` で通ること
- ALPN と既存の PUBLISH / SUBSCRIBE の挙動が変わっていないこと
