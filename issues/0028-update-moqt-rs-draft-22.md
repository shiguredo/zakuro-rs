# moqt-rs を draft-22 対応の最新に追従させる

- Created: 2026-10-04
- Completed: 2026-10-04
- Branch: feature/update-moqt-rs-draft-22
- Polished: {YYYY-MM-DD}

## 目的

`zakuro-moq` は MOQT のプロトコル実装を `shiguredo_moqt` に依存している。moqt-rs は
draft-ietf-moq-transport-22 に追従し、QUIC の ALPN プロトコル識別子も `moqt-22` に
変更された。`zakuro-moq` が固定している rev は draft-21 ベースのままで、ALPN も
`moqt-21` を広告しているため、draft-22 対応の relay と接続できない。

## 現状

- `zakuro-moq/Cargo.toml` は `shiguredo_moqt` を git 依存の rev
  `056cf19a75ce22e28b2638c21fdff9df52096b28` (2026-10-01、draft-21 ベース) で固定している
- moqt-rs の latest develop は `641ebb115e2552049ac2110332b98b36ae986790` で、draft-22 追従
  (LOCATION_FILTER の符号化変更、Object Forwarding Preference から Delivery Mode への改名、
  REQUEST_OK の許可パラメータ縮小、ALPN の `moqt-22` 化) を含む
- `zakuro-moq` の ALPN 定数は `moqt-21` のまま (`zakuro-moq/src/moq_client/transport.rs` の `ALPN`)
- コメントの仕様参照が `draft-ietf-moq-transport-21` のまま。draft-22 で節構成が変わった
  参照が 2 件ある
  - Forward State の「publisher は Forward State 0 の間 Object を送らない」は
    §3.1 (Subscriptions) から §3.1.1 (Pausing Subscriptions) へ移動し、文面も
    "paused subscription" に変わった
  - 制御ストリームをセッション中に閉じてはならない規則は §6.4.1 ではなく
    §6.3 (Session initialization) にある

## 設計方針

- `shiguredo_moqt` の rev を最新 `641ebb1` に更新し、`Cargo.lock` を再生成する
- ALPN を `moqt-22` にする
- 仕様参照を一次資料 draft-ietf-moq-transport-22 に同期する。節番号が変わった 2 件は
  移動先の節と文面に合わせる
- 実装の挙動は ALPN の値以外は変えない

## 完了条件

- `make ci` (fmt / clippy / test / smoke) が通ること
- QUIC ハンドシェイクで ALPN `moqt-22` を広告すること
- `zakuro-moq` のコードとコメントから `draft-ietf-moq-transport-21` の表記がなくなること

## 変更対象

- `zakuro-moq/Cargo.toml`
- `Cargo.lock`
- `zakuro-moq/src/args.rs`
- `zakuro-moq/src/moq_client/transport.rs`
- `zakuro-moq/src/moq_client/session.rs`

## 解決方法

- `zakuro-moq/Cargo.toml` の rev を `641ebb115e2552049ac2110332b98b36ae986790`
  (draft-22 対応の develop 最新) に更新し、`cargo update -p shiguredo_moqt` で
  `Cargo.lock` を再生成した
- `zakuro-moq/src/moq_client/transport.rs` の `ALPN` を `moqt-22` に変更した
- 仕様参照を一次資料 draft-ietf-moq-transport-22 と突合して同期した。節番号が
  変わっていた 2 件は移動先に合わせた
  - Forward State の「paused subscription では Object を送らない」は
    §3.1 (Subscriptions) → §3.1.1 (Pausing Subscriptions)
  - 制御ストリームをセッション中に閉じてはならない規則は §6.4.1 → §6.3
- 検証: `make ci` (fmt / clippy / test / smoke) が exit 0。テストは 215 件 +
  zakuro-moq 50 件がすべて成功した
  - issue 0029 の webrtc 修正と同時に検証した (0029 を先にコミットしている)
