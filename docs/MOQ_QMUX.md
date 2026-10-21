# QMux トランスポート対応の検討メモ

zakuro-moq が QMux (draft-ietf-quic-qmux-02) 経由で MOQT relay へ接続するための検討結果と
実装方針。仕様は draft-ietf-moq-transport-22 (2026-10-01 発行、最新の公開 I-D) と
draft-ietf-quic-qmux-02、依存は `shiguredo_moqt` rev `49c711e` と qmux-rs rev `3e0dc0f` を
前提とする。本文中の `ファイル:行` は実装前の検討時点のものを含むため、現在のソース上の位置は
シンボル名で検索すること。

## 結論

- `--transport quic|qmux` を追加し、既定は `quic` のままにする。QMux は TLS over TCP の
  1 本のバイトストリームに QUIC version 1 のストリームと DATAGRAM を多重化する経路であり、
  UDP が使えない環境でも負荷試験できる。
- MOQT 本体は TCP など他のトランスポート上で直接動かす方法を定義していない
  (draft-ietf-moq-transport-22 §6.2)。QMux 上の MOQT は individual draft
  (`draft-nandakumar-moq-qmux-moqt-00`) が扱う experimental な組み合わせである。
- 実装は qmux-rs の `tokio_qmux::Connection` を使い、s2n-quic の型に直接依存している
  `session.rs` はストリームの enum アダプタ経由にする。trait は作らない。
- moqt-rs の rev を `49c711e` へ更新する。`Transport::Qmux` はこの rev で追加されており、
  zakuro-moq が使う `Session` の公開 API に破壊的変更は無い。

## 1. 仕様上の位置づけと判断

### 1.1 ALPN

QMux 上の MOQT は native QUIC と同じプロトコル識別子を使う
(individual draft Section 3.2)。zakuro-moq も `moqt-22` を QUIC 直接接続と共有する。

一方 draft-ietf-quic-qmux-02 Section 8.1 は「同じプロトコルの QUIC マッピングと異なる識別子を
MUST」としている。MOQT 側に QMux 用の ALPN 登録が無く、SVCB 互換レコードを定める draft も
未整備であるため、interop を優先して同節から外れる。両 draft が決着したら見直す。

rustls は TCP + TLS では ALPN 未交渉でもハンドシェイクを成功させるため、接続後に
`alpn_protocol()` が `moqt-22` であることを明示的に確認する (§8.1 の client MUST)。

### 1.2 SETUP の AUTHORITY / PATH

draft-ietf-moq-transport-22 §9.1.1 (AUTHORITY) / §9.1.2 (PATH) は WebTransport での送信を
MUST NOT とするが、QMux は WebTransport ではない。QMux でも native QUIC と同じく
AUTHORITY / PATH を送る (individual draft の参照実装と同じ扱い)。`Session::new_client` には
`Transport::Qmux` を渡す。

### 1.3 制御ストリームとストリームの対応

individual draft Section 4.2 は control stream を QMux の双方向 stream 0 とするが、
これは draft-ietf-moq-transport-22 §6.4.1 (control stream は単方向) と矛盾する。
zakuro-moq は QMux が QUIC version 1 のストリームを多重化する性質に従い、QUIC と 1 対 1 に
対応させる。自側 control stream は単方向 stream を開いて stream type `0x2F00` + SETUP を書き、
peer の control stream は最初の単方向 stream として受理する。

### 1.4 DATAGRAM

QMux の DATAGRAM は下位の TCP が再送するため経路損失では失われないが、受信側のバッファが
溢れると破棄され得る (draft-ietf-quic-qmux-02 Section 6)。zakuro-moq は現状 DATAGRAM で
object を送受信しない (QUIC 直接接続と同じ) ため、`max_datagram_frame_size` を広告するだけに
とどめる。

### 1.5 TLS

QUIC 直接接続と揃えて TLS 1.3 のみを使う。QMux の仕様は TLS 1.3 を必須にしていないため、
TLS 1.2 しか使えない relay には接続できない。必要になったら経路ごとにバージョン指定を
分ける。

### 1.6 idle timeout と keep-alive

QMux の実効 idle timeout は両端の広告値のうち 0 でない値の最小値になる
(RFC 9000 Section 10.1)。zakuro-moq は 30 秒を広告するが、Forward State 0 で object を
送らない間に切られないよう QX_PING を送る。PING の間隔は実効 idle timeout の 1/3 を基本とし、
100 ms から 10 秒の範囲に収める。ピアのトランスポートパラメータが届く前に間隔を決めると
ピアの短い値を無視するため、パラメータ到着を最大 5 秒待ってから決める。

## 2. 実装構造

- `zakuro-moq/src/moq_client/qmux.rs`: QMux 接続の確立 (TCP + TLS + ALPN 確認 +
  `tokio_qmux::Connection` の生成)、QMux の `Config`、keep-alive の PING タスク。
- `zakuro-moq/src/moq_client/stream.rs`: s2n-quic と tokio-qmux のストリーム・接続ハンドル・
  accept を enum で包むアダプタ。`write_all` / `finish` / `reset` は QMux 側が async のため
  両経路とも async に揃える。`read` は QMux の `Ok(None)` (FIN) を `Ok(0)` に正規化する。
- `zakuro-moq/src/moq_client/transport.rs`: `MoqTransport` (quic / qmux)、経路ごとの TLS 設定、
  `MoqConnection` と `split`。QMux は仮想クライアントごとに TCP を張るため、QUIC の UDP
  ソケット共有 (`EndpointState.clients`) には相乗りさせない。
- `zakuro-moq/src/moq_client/session.rs`: 経路非依存のセッション駆動。`Transport::Qmux` を
  `Session::new_client` に渡す以外は、ストリームのアダプタ越しに同じ処理をする。
- `zakuro-moq/src/args.rs` / `main.rs`: `--transport` と JSONC の `transport` キー。
  既定は `quic`。既存の QUIC 経路の挙動は変えない。

## 3. 制約と未対応

- QMux を終端できる実 relay は確認できていない (moqt-rs の E2E も QMux を対象外にしている)。
  このため実 relay に対する E2E は未実施である。ストリームアダプタは qmux-rs の client と
  server をメモリ上の duplex で両端に立てて検証している (モックではない)。
- QMux の DATAGRAM は広告のみで、object の DATAGRAM 送受信は未対応 (QUIC 経路と同じ)。
- TCP の再送で遅延が伸びるため、遅延を重視する試験では QUIC 直接接続を使うこと。
- QMux の RESET_STREAM_AT は未実装のため、reset の `reliable_size` は常に `None` である。
- 実 relay の ALPN が `moqt-22` 以外を要求する場合や、control stream を individual draft の
  双方向 stream 0 で要求する場合は、relay 側の実装に合わせて見直す必要がある。

## 4. 検証

- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings`
- `cargo test --locked --workspace --features fdk-aac`
- `make smoke` (`zakuro-moq --help` に `--transport` が出ること)
- tombic (`prek run tombi-lint --all-files` / `prek run tombi-format --all-files`)
