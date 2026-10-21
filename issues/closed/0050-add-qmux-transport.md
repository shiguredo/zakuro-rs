# QMux 経由で MOQT relay へ接続できるようにする

- Created: 2026-10-21
- Completed: 2026-10-21
- Branch: feature/add-qmux-transport
- Polished: {YYYY-MM-DD}

## 目的

zakuro-moq が UDP を使えない環境や、TCP しか通らない環境でも MOQT relay への負荷試験を
できるようにする。QMux (draft-ietf-quic-qmux-02) は順序保証された 1 本の双方向バイトストリーム
(TLS over TCP) の上で、QUIC version 1 のストリーム操作と DATAGRAM 操作を多重化する経路で
ある。加えて、moqt-rs の rev が古く `Transport::Qmux` を持たないため、最新の rev へ更新する。

## 現状

- `zakuro-moq/src/moq_client/transport.rs` は s2n-quic の `Client` / `Connection` を直接持ち、
  UDP ソケットを接続先ごとに共有している。`session.rs` も `Connection` / `Handle` /
  `SendStream` / `ReceiveStream` を直接持つため、QMux を足すには両方の抽象化が要る。
- `shiguredo_moqt` の固定 rev `f91baf3` には `Transport::Qmux` が無い。moqt-rs の最新
  `49c711e` で追加されており、zakuro-moq が使う `Session` の公開 API と `SessionEvent` の
  variant に破壊的変更は無い (差分で確認済み)。
- qmux-rs (`shiguredo_qmux` / `tokio-qmux`) は crates.io 未公開で、moqt-rs の
  examples/tokio-moq も git の rev 固定で取り込んでいる。
- QMux を終端できる実 relay は確認できていない。moqt-rs の E2E も QMux を対象外にしている。

## 設計方針

- `--transport quic|qmux` を追加する (既定 `quic`)。JSONC では `"transport"` キーで指定でき、
  `instances` ごとに上書きできる。URL は両経路とも `moqt://` のままとする。
- s2n-quic と tokio-qmux のストリーム・接続ハンドル・accept を enum で包むアダプタ
  (`moq_client/stream.rs`) を作り、`session.rs` は経路を意識せずに駆動する。トレイトは作らない。
- QMux の接続確立 (`moq_client/qmux.rs`) は TCP + TLS (ALPN `moqt-22`) で行い、
  `tokio_qmux::Connection` を作る。`Config` は QUIC 直接接続の limits と同等のクレジットを
  明示する (`Config::default()` は全クレジット 0 のため)。
- SETUP は QMux でも AUTHORITY / PATH を送る (`Transport::Qmux` を `Session::new_client` に渡す)。
- Forward State 0 の無通信で切られないよう、QX_PING を実効 idle timeout の 1/3 の間隔で送る。
  ピアのトランスポートパラメータ到着後に間隔を決める。
- DATAGRAM で object を送受信する機能は現状の QUIC 経路と同じく追加しない。
  `max_datagram_frame_size` の広告だけ揃える。
- 既存の QUIC 経路の挙動 (limits、keep-alive、FIN / RESET の順序、ログ形式) は変えない。

## 完了条件

- `--transport quic` (既定) の挙動が従来と変わらないこと。
- `--transport qmux` で QMux 接続を確立し、MOQT セッションを開始できること。
- QMux のストリームアダプタが本物の QMux client / server で往復できること (モックなし)。
- `cargo fmt` / `cargo clippy` / `cargo test` / `make smoke` / tombi が全て通ること。
- 実 QMux relay に対する E2E で publish / subscribe が成立すること。

## 解決方法

- `zakuro-moq/Cargo.toml` で `shiguredo_moqt` の rev を `49c711e` へ更新し、
  `shiguredo_qmux` / `tokio-qmux` (rev `3e0dc0f`) と `tokio-rustls` を追加した。
  `Cargo.lock` も更新した。
- `moq_client/qmux.rs` を追加した。TCP 接続 (TCP_NODELAY)、TLS 1.3 ハンドシェイク、
  ALPN `moqt-22` の確認、`tokio_qmux::Connection` の生成、QMux の `Config`、
  idle timeout の 1/3 間隔で QX_PING を送る keep-alive タスクを実装した。
- `moq_client/stream.rs` を追加した。`MoqSendStream` / `MoqRecvStream` /
  `MoqConnectionHandle` / `MoqRecvAcceptor` / `MoqBidiAcceptor` で両経路を包み、
  `finish` / `reset` を async に揃え、QMux の `Ok(None)` (FIN) を `Ok(0)` に正規化した。
- `moq_client/transport.rs` に `MoqTransport` / `MoqConnection` を追加し、TLS 設定を
  QUIC 用と QMux 用で共有する `build_rustls_config` に整理した。`--ca-cert` は PEM を
  自分でパースするようにした。QMux の `EndpointState` は名前解決結果と SNI だけを持ち、
  QUIC の UDP ソケットを作らない。
- `moq_client/session.rs` を経路非依存にした。`Session::new_client` に `Transport::Quic` /
  `Transport::Qmux` を渡し、ストリーム操作はアダプタ経由にした。`close_stream` /
  `reset_stream` は async にした。
- `args.rs` に `--transport` と `InstanceArgs::transport` を追加し、`main.rs` で
  `MoqClientConfig` へ引き回した。起動ログに `transport=` を出した。
- テストを追加した。`--transport` の CLI / JSONC 解釈、既定値、不正値、
  QMux の TLS 設定の ALPN、存在しない `--ca-cert` のエラー、QMux の `Config`、
  keep-alive の間隔計算、QMux の接続コンテキストが QUIC クライアントを作らないこと、
  実 QMux の duplex 接続を使った単方向 / 双方向ストリームの往復を検証した。
- ドキュメントを更新した。README に `--transport` とトランスポートの節を追加し、
  `docs/MOQ_QMUX.md` に仕様上の判断と制約をまとめ、`docs/MOQ_GOAWAY.md` の rev と
  接続コンテキストの説明を更新し、`docs/LOADTEST_GAP.md` のトランスポート記述を更新した。
- 検証結果: `cargo fmt --all -- --check` / `cargo clippy --locked --workspace --all-targets
  --features fdk-aac -- -D warnings` / `cargo test --locked --workspace --features fdk-aac` /
  `make smoke` / `prek run tombi-lint --all-files` / `prek run tombi-format --all-files` が
  全て成功した。
- 実 QMux relay (`moqt://sora-moq.shiguredo.co.jp/`) に対する E2E を 2026-10-21 に実施した。
  TLS の証明書検証を有効にしたまま `--transport qmux` で接続し、次の 2 条件で
  publish / subscribe が成立した。
  - VC 1 本 20 秒 (`video:30:1000`): sent-objects=600 / received-objects=600 /
    payload-mismatches=0
  - VC 3 本 10 秒 (`video:10:500`): sent-objects=300 / received-objects=300 /
    payload-mismatches=0 (VC ごとに TCP + TLS 接続を張る)
