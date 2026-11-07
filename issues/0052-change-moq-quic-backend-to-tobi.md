# MOQ の QUIC 実装を tobi-quic へ置き換える

- Created: 2026-11-08
- Completed: {YYYY-MM-DD}
- Branch: feature/change-moq-quic-backend-to-tobi
- Polished: {YYYY-MM-DD}

## 目的

zakuro-moq の QUIC 依存を自社実装の tobi-quic に一本化し、負荷試験で tobi-quic のクライアントを
実際に叩ける状態にする。

現在は s2n-quic と ngtcp2-rs の 2 実装を経路ごとに使い分けており、WebTransport over HTTP/3 の
ためだけに ngtcp2 のイベント駆動 API へ HTTP/3 と WebTransport を自前で結線している。
`shiguredo/tobi-quic` の main ブランチには Multipath QUIC のクライアント API と、
WebTransport over HTTP/3 の結線 (`tobi-webtransport`) が揃っているため、これらへ置き換えると
QUIC 層の依存と結線コードを大幅に減らせる。

## 現状

- `--quic-backend` で `s2n-quic` / `ngtcp2` を選ぶ。`--transport quic` の既定は `s2n-quic`、
  `--transport wt-h3` は `ngtcp2` 固定である。
- s2n-quic は `RESET_STREAM_AT` を送出できないため、WebTransport over HTTP/3 のデータ
  ストリーム reset (draft-ietf-webtrans-http3-16 §4.4) を満たせず `wt-h3` に使えない。
  このため wt-h3 は ngtcp2 + `shiguredo_http3` の独自結線になっている。
- `zakuro-moq/src/moq_client/ngtcp2.rs` と `ngtcp2/webtransport.rs` は、ngtcp2 のイベント
  駆動 API に合わせて接続の駆動タスク、ストリーム、HTTP/3、WebTransport のフロー制御を
  自前で実装している。
- `tobi_quic::Connection` は `open_send_stream` / `open_bidirectional_stream` /
  `accept_receive_stream` / `accept_bidirectional_stream` / `send_datagram_wait` /
  `receive_datagram` / `keep_alive` / `close_application` を提供する。
- `tobi_webtransport::client::Client` は ALPN `h3` と WebTransport の前提
  (DATAGRAM、`reset_stream_at`) を設定して接続し、`wt-protocol` の交渉まで行う。

## 設計方針

- `tobi_quic` と `tobi_webtransport` を `https://github.com/shiguredo/tobi-quic` の
  `main` ブランチへの git 依存として追加する。crates.io ではなく main を追うのは、
  自社実装の最新状態を常に負荷試験へ通すためである。
- `s2n-quic` / `shiguredo_ngtcp2_tokio` / `shiguredo_http3` / `base64ct` / `bytes` の依存と、
  `--quic-backend` を削除し、QUIC 実装を 1 本化する。`--transport` の値
  (`quic` / `wt-h3` / `wt-h2` / `qmux`) は変えない。
- QUIC 直接接続は `zakuro-moq/src/moq_client/quic.rs` に集約する。
  - `Client::builder()` に TLS、`IoConfig`、`DatagramConfig`、`ConnectionConfig` を渡し、
    `with_background_driver(true)` で受信とタイマーを常時進める。
  - `ConnectionConfig` で受信ウィンドウとストリーム上限を負荷試験向けに広げ、
    keep-alive を 5 秒間隔にする。MOQT には PING が無く、Forward State 0 で無通信に
    なっても relay の idle timeout で切られないようにするためである。
  - ハンドシェイク後に ALPN が `moqt-22` であることを確認する。
- WebTransport over HTTP/3 は `zakuro-moq/src/moq_client/wt_h3.rs` に集約する。
  `tobi_webtransport::client::Client` でセッションを開設し、拡張 CONNECT で
  `moqt-22` を `wt-available-protocols` として提示する。
  `Session` の受信 API と accept は `&mut self` を要求するため、`Session` を専用タスクが
  所有し、接続ハンドル・acceptor・datagram 受け口はチャネルで操作する
  (`wt_h2.rs` と同じ構造)。
- `zakuro-moq/src/moq_client/stream.rs` の enum の variant を `Quic` / `WtH3` に
  入れ替える。reset のエラーコードは `StreamReset` として `io::Error` に載せ、
  `session.rs` の downcast を維持する。
- `zakuro-moq/src/moq_client/transport.rs` の `EndpointState` を、QUIC と wt-h3 それぞれの
  tobi クライアントに置き換える。アドレスファミリごとに UDP ソケットを持つ構成と、
  接続先ごとに状態をキャッシュする構成は維持する。
- `--ca-cert` の扱いを「WebPKI の代わりにその PEM だけを信頼する」に統一する。
- wt-h3 の keep-alive は tobi-webtransport が QUIC 接続を公開しないため設定できない。
  無通信の wt-h3 セッションは relay の idle timeout で切られる可能性があるため、
  残存リスクとして記録する。
- テストはモックやスタブを使わず、loopback の実 tobi-quic / tobi-webtransport サーバーとの
  接続でストリームと datagram の往復を検証する。

## 完了条件

- `--transport quic` と `--transport wt-h3` が tobi-quic / tobi-webtransport で接続できる。
- 既存の `wt-h2` / `qmux` の経路とテストが変わらず動く。
- loopback の実接続で QUIC / wt-h3 の単方向ストリームと datagram の往復を検証するテストが
  通る。
- `cargo fmt --all -- --check` / `cargo clippy --locked --workspace --all-targets
  --features fdk-aac -- -D warnings` / `cargo test --locked --workspace --features fdk-aac`
  が通る。
- README / docs / `THIRD_PARTY_LICENSES.md` が追随している。

## 解決方法

どのように対応するのかを明確にすること (例: どのようなコードを追加・修正するのか、どのようなテストを追加するのかなど)
