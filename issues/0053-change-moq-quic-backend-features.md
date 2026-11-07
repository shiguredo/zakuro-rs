# MOQ の QUIC バックエンドを feature で選択可能にし、draft-23 へ追従する

- Created: 2026-11-08
- Completed: {YYYY-MM-DD}
- Branch: feature/change-moq-quic-backend-features
- Polished: {YYYY-MM-DD}

## 目的

moqt-rs が draft-23 に対応したため、zakuro-moq の MOQT を draft-23 に追従させる。
あわせて、ネイティブ QUIC のバックエンドを Cargo feature で選べるようにし、tobi-quic 以外の
実装 (s2n-quic / ngtcp2) でも負荷試験を比較できる状態にする。

## 現状

- `zakuro-moq/Cargo.toml` の `shiguredo_moqt` は rev `49c711e` (draft-22) に固定されている。
  ALPN も `moqt-22` であり、draft-23 の relay と接続できない。
- ネイティブ QUIC は tobi-quic 固定である。以前使っていた s2n-quic と ngtcp2 の実装は
  削除済みで、比較のための切り替えができない。
- `shiguredo_moqt` の draft-23 版では次の API が変わっている。
  - `encode_control_stream_setup` と `Message::encode` が `max_control_message_size` を要求する
  - `MessageDecoder::new` が `max_control_message_size` を要求する
  - `MessageParameters::forward` が廃止され、PAUSED パラメータ (`paused`) に変わった
  - `SessionEvent` に `TrackStatusUnsupported` と `GoawayDrainReady` が追加された
- moqt-rs の `tokio-moq` は `quic-tobi` (既定) / `quic-s2n` / `quic-ngtcp2` の 3 feature で
  ネイティブ QUIC のバックエンドを選び、ちょうど 1 つを要求する構成になっている。

## 設計方針

- `shiguredo_moqt` の rev を `66eb946` (draft-23 対応の develop 最新) へ更新し、ALPN を
  `moqt-23` にする。ALPN は `transport.rs` の `ALPN` 定数を唯一の正とし、QMux と
  WebTransport の `WT-Available-Protocols` / `WT-Protocol` も同じ値を使う。
- draft-23 の API 変更へ追従する。
  - SETUP のエンコードとデコードには `DEFAULT_MAX_CONTROL_MESSAGE_SIZE` を使い、
    peer の SETUP 受信後の制御メッセージは `Session::peer_control_message_size_limit` を使う。
  - REQUEST_UPDATE の FORWARD を PAUSED に置き換える (`paused == 0` を forwarding とみなす)。
  - 追加された `SessionEvent` は、zakuro-moq が発行しないイベントのためログのみで無視する。
- Cargo feature を `quic-tobi` (既定) / `quic-s2n` / `quic-ngtcp2` の 3 つにし、ちょうど 1 つ
  だけ有効にする (2 つ以上・0 個は `compile_error!` で拒否する)。
  - `quic-s2n` は `s2n-quic`、`quic-ngtcp2` は `shiguredo_ngtcp2_tokio` と `base64ct` を
    任意依存として有効にする。
  - `tobi_quic` / `tobi_webtransport` は WebTransport over HTTP/3 が常に使うため、
    バックエンドに関わらず依存に含める。
- バックエンドごとの実装を `zakuro-moq/src/moq_client/quic/` に分ける。
  - `quic/tobi.rs`: tobi-quic (従来の実装を移設)
  - `quic/s2n.rs`: s2n-quic
  - `quic/ngtcp2.rs`: ngtcp2 (削除前の実装から WebTransport の結線を除いて復元)
  - ストリーム・接続ハンドル・acceptor・datagram の enum は variant を `cfg` で出し分ける。
- `--quic-backend` は追加しない。バックエンドはビルド時に決まる。
- 各バックエンドで実サーバーとの loopback 接続によるストリームと datagram の往復テストを持つ。

## 完了条件

- ALPN が `moqt-23` になり、`shiguredo_moqt` が draft-23 対応の rev になる。
- `quic-tobi` / `quic-s2n` / `quic-ngtcp2` のそれぞれでビルドとテストが通る。
- 3 つのバックエンドすべてで、実 QUIC 接続によるストリームと datagram の往復テストが通る。
- `cargo fmt --all -- --check` / `cargo clippy --locked --workspace --all-targets
  --features fdk-aac -- -D warnings` / `cargo test --locked --workspace --features fdk-aac`
  が通る。
- README と docs、`THIRD_PARTY_LICENSES.md` が追随している。

## 解決方法

どのように対応するのかを明確にすること (例: どのようなコードを追加・修正するのか、どのようなテストを追加するのかなど)
