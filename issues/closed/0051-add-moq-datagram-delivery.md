# MOQ でトラック単位に datagram / subgroup 配送を選べるようにする

- Created: 2026-10-25
- Completed: 2026-10-25
- Branch: feature/add-moq-datagram-delivery
- Polished: {YYYY-MM-DD}

## 目的

zakuro-moq が Sora MoQ のリレーへ publish するとき、音声を OBJECT_DATAGRAM で送る経路を
負荷試験できるようにする。Sora MoQ のクライアント (moqt-js) は音声の配送方式を
Subgroup / Datagram から選べるため、リレーの datagram 経路を zakuro-moq から再現できないと
音声経路の負荷試験ができない。あわせて、購読側で外部 publisher の datagram 音声を受信できる
ようにする。

## 現状

- `--tracks` は `名前[:レート[:サイズ]]` のみで、配送方式を指定できない。
- `Publisher` は全 object を subgroup stream で送る。Session の `send_object_datagram` は
  呼んでいない。datagram は QUIC endpoint と QMux で広告するだけで、送受信していない。
- 購読側は uni data stream だけを読み、datagram は WebTransport over HTTP/2 で受信しても
  破棄している。
- Delivery Mode は Original Publisher が object ごとに決める値で、購読側とは交渉しない
  (draft-ietf-moq-transport-22 §2.1.1)。zakuro-moq の publish 設定として持つ必要がある。
- 音声は 1 object が小さく、遅れた object を届けるより落とす方がよいため、LOC は音声を
  datagram でも送れるとしている (draft-ietf-moq-loc-04 §4.1)。

## 設計方針

- トラック単位で配送方式 (`subgroup` / `datagram`) を選ぶ。既定は `subgroup` で、従来の
  挙動を変えない。
- `--tracks` の 4 番目に配送方式を追加し、JSONC では `delivery-mode` キーで指定する。
- `--datagram-max-size` (既定 1160 バイト) を追加し、起動時に object サイズとヘッダが
  datagram に収まるかを検証する。上限を超えた datagram は通知なく破棄されるため
  (draft-ietf-moq-transport-22 §11.2)、試験中に黙って落ちるより起動時にエラーにする。
- publish 側は配送方式で送信処理を分岐する。datagram では subgroup stream を開かず、
  OBJECT_DATAGRAM を送る。Group の最終 object には END_OF_GROUP bit を立てる。
- 購読側は経路ごとの datagram 受信を `Session::recv_datagram` へ渡し、subgroup 配送と同じ
  計測 (`--verify-payload` を含む) に合算する。
- datagram の送受信は s2n-quic、ngtcp2、WebTransport over HTTP/3、WebTransport over
  HTTP/2、QMux の全経路で実装する。
- Group 長の変更、subgroup / datagram の混在比率、datagram の冗長送信、欠落 / 順序の計測は
  今回のスコープ外とする。

## 完了条件

- `--tracks video:30:1000,audio:50:200:datagram` で audio だけを datagram 配送にできること
- JSONC の `delivery-mode` で同じ設定ができること
- 全経路で datagram の送受信ができ、購読側で payload を検査できること
- 既定 (配送方式を省略) の挙動が従来と変わらないこと
- `cargo fmt` / `cargo clippy` / `cargo test` / `make smoke` / tombi が通ること
- 実 relay の E2E で audio datagram の publish / subscribe が成立すること

## 解決方法

- `zakuro-moq/src/args.rs` に `DeliveryMode`、`--datagram-max-size`、`--tracks` の 4 番目と
  JSONC の `delivery-mode` の解釈、起動時のサイズ検証を追加した。既定は `subgroup` で、
  datagram 配送のトラックは `object-size + ヘッダ 48 バイト` が上限に収まることと、
  ngtcp2 経由では 1200 バイト (WebTransport over HTTP/3 は 1199 バイト) 以下であることを
  確認する。
- `zakuro-moq/src/moq_client/stream.rs` に `MoqConnectionHandle::send_datagram` と
  `MoqDatagramReceiver` を追加した。受信は s2n-quic の datagram provider、ngtcp2 と
  WebTransport over HTTP/2 の駆動タスクのチャネル、QMux の受信キューから取り出す。
- s2n-quic の datagram endpoint は送受信のキューの容量が既定で 0 のため、QUIC 直接接続の
  クライアントに送受信 64 件の容量を設定した。設定しないと datagram を 1 つも送れない。
- `zakuro-moq/src/moq_client/ngtcp2.rs` と `ngtcp2/webtransport.rs` に QUIC DATAGRAM と
  HTTP Datagram の送受信を追加した。HTTP/3 経路は `feed_datagram` で受けた datagram を
  WT イベントとして受け取り、送信は `send_datagram` でエンコードして QUIC へ渡す。
- `zakuro-moq/src/moq_client/wt_h2.rs` に `WT_DATAGRAM` capsule の送受信を追加した。
- `zakuro-moq/src/moq_client/transport.rs` の `split` が datagram 受信口も返すようにした。
- `zakuro-moq/src/moq_client/session.rs` に datagram 送信 (`send_next_datagram`) と購読側の
  datagram 受信 (`DatagramReceiveState` / `handle_received_datagram`) を追加した。payload 長は
  `ObjectDatagram::decode` で得て、`--verify-payload` の検査にも使う。
- テストを追加した。配送方式の CLI / JSONC 解釈、上限超過の起動エラー、END_OF_GROUP の
  判定、s2n-quic / ngtcp2 / WebTransport over HTTP/3 / WebTransport over HTTP/2 / QMux の
  datagram 往復を実トランスポートの loopback 接続で検証する。
- README と `docs/MOQ_QMUX.md` / `docs/MOQ_WEBTRANSPORT_H2.md` / `docs/LOADTEST_GAP.md` を
  更新し、`docs/MOQ_DELIVERY_MODE.md` に仕様と設計の検討メモを追加した。
- E2E ワークフローに subgroup 配送と audio datagram 配送の 2 ラウンドを追加した。実 relay の
  E2E (2026-10-25) で、subgroup 配送は 896 送信 / 891 受信、datagram 配送は 1492 送信 /
  1484 受信、いずれも payload-mismatches=0 だった。
