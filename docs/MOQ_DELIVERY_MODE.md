# MOQT の配送方式 (subgroup / datagram) 切り替えの検討メモ

このメモは、配送方式をトラック単位のオプションにした時点の検討である。
現行の zakuro-moq は、映像を framerate / bitrate / interval から Group と Subgroup に区切り、
音声を datagram で 1 object = 1 Group にする。指定方法は README の MOQ 版を参照する。

zakuro-moq で object の配送方式 (Delivery Mode) をトラック単位で選べるようにするための
検討結果と実装方針。仕様は draft-ietf-moq-transport-23 (MOQT)、draft-ietf-moq-loc-04 (LOC)、
RFC 9000、RFC 9221 を前提とする。節番号は将来の draft 改訂で変わる可能性がある。

## 結論

- `--tracks` と JSONC に配送方式を追加し、トラック単位で `subgroup` / `datagram` を選ぶ。
  既定は従来どおり `subgroup` とする。
- Delivery Mode は Original Publisher が object ごとに決める値であり
  (draft-ietf-moq-transport-23 §2.1.1 (Object Fields))、購読側とは交渉しない。したがって
  zakuro-moq の publish 設定として持つのが正しい。
- datagram 配送を成立させるには、下位トランスポート (tobi-quic / WebTransport over
  HTTP/3 / WebTransport over HTTP/2 / QMux) の datagram 送受信と、購読側の datagram 受信が
  すべて必要になる。publish 側だけにフラグを足す変更ではない。
- LOC の音声に忠実な Group 構造 (1 object = 1 Group) は今回のスコープ外とし、既存の
  「約 1 秒分を 1 Group」を維持する。配送方式だけを変えた A/B 比較を優先する。

## 1. 仕様上の位置づけ

### 1.1 Delivery Mode

- Delivery Mode は object ごとの値で、Original Publisher が最初の送信方法で確定する。
  同じ subscription では、その object を確定した方式で送らなければならない
  (draft-ietf-moq-transport-23 §2.1.1 (Object Fields))。
- datagram で届いた object は Delivery Mode = Datagram、subgroup stream で届いた object は
  Delivery Mode = Subgroup になる (§11.2.1 / §11.3.1)。
- 同じトラックで datagram と subgroup を混在させることは仕様上可能である。zakuro-moq は
  負荷条件を比較しやすくするため、トラック単位で方式を固定する。
- 購読側は PUBLISH / SUBSCRIBE のパラメータで方式を要求できない。relay は publisher が
  確定した方式で転送する。

### 1.2 datagram の制約

- datagram は分割できず、1 つが 1 つの QUIC パケット (WebTransport では HTTP Datagram) に
  収まる必要がある。経路の上限を超えた object は通知なく破棄される
  (draft-ietf-moq-transport-23 §11.2 (Datagrams))。
- datagram は再送されない (RFC 9221 §3)。QUIC の輻輳制御の制約は受ける。
- Group の終端を知らせる FIN が無いため、Group の最終 object に END_OF_GROUP bit を立てる
  (§11.2.1 (Object Datagram))。この bit は「同じ Group ID で、この Object ID より大きい
  Object は存在しない」ことを宣言する。
- LOC は音声を「1 audio frame = 1 Object = 1 Group」とし、stream でも datagram でも送れると
  している (draft-ietf-moq-loc-04 §4.1 (Application with one audio track))。音声のように
  1 object が小さく、遅れた object を届けるより落とす方がよいメディアは datagram に向く。

## 2. 設計方針

### 2.1 設定

- `--tracks` の形式を `名前[:レート[:サイズ[:配送方式]]]` に拡張する。配送方式は
  `subgroup` / `datagram` で、省略時は `subgroup` とする。
  - 例: `--tracks video:30:1000,audio:50:200:datagram`
- JSONC のトラック設定に `"delivery-mode"` を追加する。
  - 例: `{ "name": "audio", "object-rate": 50, "object-size": 200, "delivery-mode": "datagram" }`
- `--datagram-max-size` を追加する。既定は 1160 バイトである。RFC 9000 §14 (Datagram Size) が
  経路に要求する 1200 バイト (QUIC パケットヘッダと AEAD タグを含む UDP payload) から、
  QUIC と WebTransport のヘッダ分を引いた値である。
- 起動時に、datagram 配送のトラックについて `object サイズ + 最大ヘッダ` が
  `--datagram-max-size` に収まるかを検証する。datagram のサイズ超過は通知なく破棄されるため、
  試験中に黙って落ちるより起動時にエラーにする方がよい。

### 2.2 publish 側

- `Publisher` は配送方式で送信処理を分岐する。datagram では subgroup stream を開かず、
  object ごとに OBJECT_DATAGRAM を組み立てて送る。
- LOC の Timestamp / Timescale プロパティは subgroup 配送と共通にする。
- Group ID と Object ID の進め方は subgroup 配送と同じにする。Group の最終 object には
  END_OF_GROUP bit を立てる。
- Session のフィルタ評価に落ちた object はワイヤへ送らない (draft-ietf-moq-transport-23
  §3.3.3 (Combining Filters))。subgroup 配送と同じ扱いである。
- 送信キューが満杯の場合は空きができるまで待つ。待たずに捨てると、音声の欠落として現れる
  うえ送信数の計測もずれる。

### 2.3 購読側

- 購読しているセッションでは、経路ごとの datagram 受信を `Session::recv_datagram` へ渡す。
  publish 専用のセッションでは受信しない。
- 受理された object は payload 長と `--verify-payload` のパターン検査を行い、subgroup 配送と
  同じ計測に合算する。
- datagram の順序 / 欠落 / 重複の計測は今回のスコープ外である。

### 2.4 I/O 層

経路ごとに datagram の送受信を追加する。

| 経路 | 送信 | 受信 | 特性 |
| --- | --- | --- | --- |
| `quic` | QUIC DATAGRAM | tobi-quic の受信 API | 再送なし。送信キューが満杯なら待つ |
| `wt-h3` | HTTP Datagram | tobi-webtransport の受信キュー | QUIC DATAGRAM 上で運ぶ。再送なし |
| `wt-h2` | `WT_DATAGRAM` capsule | WT イベント | TCP 上で信頼的。遅延特性は QUIC と異なる |
| `qmux` | QMux DATAGRAM | 接続の受信キュー | TCP 上で信頼的。バッファ溢れによる破棄はあり得る |

tobi-quic の DATAGRAM は `DatagramConfig` で `max_datagram_frame_size` を広告し、送信は
`send_datagram_wait` で送信キューの空きを待つ。これにより負荷試験では「送信できた object 数」を
正しく数えられる。

## 3. 実装構造

- `zakuro-moq/src/args.rs`: 配送方式と `--datagram-max-size` の解釈、起動時のサイズ検証。
- `zakuro-moq/src/moq_client/session.rs`: `Publisher` の datagram 送信、購読側の datagram 受信、
  END_OF_GROUP の決定。
- `zakuro-moq/src/moq_client/stream.rs`: 接続ハンドルの datagram 送信と、経路ごとの
  datagram 受信口 (`MoqDatagramReceiver`)。
- `zakuro-moq/src/moq_client/transport.rs`: `split` で datagram 受信口を返す。
- `zakuro-moq/src/moq_client/quic.rs` / `wt_h3.rs`: tobi-quic の QUIC 直接接続と
  tobi-webtransport の WebTransport over HTTP/3 の datagram 送受信。
- `zakuro-moq/src/moq_client/wt_h2.rs`: WebTransport over HTTP/2 の datagram 送受信。
- `README.md` と各トランスポートのドキュメント: 配送方式の指定方法と制約。

## 4. 制約と未対応

- 同じトラックでの subgroup / datagram の混在比率は指定できない。
- Group 長 (1 Group あたりの object 数) は指定できない。LOC の音声に合わせて 1 object =
  1 Group にすることも未対応である。
- datagram の冗長送信 (同じ object を複数回送る) は未対応である。
- datagram の欠落 / 順序 / 遅延の計測は未対応である。datagram は再送されないため、
  送信数と購読側の受信数の差で判断する。
- 実リレーが publisher 起点の datagram を転送できることは実 relay の E2E で確認済みである
  (第 5 章)。

## 5. 検証

- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings`
- `cargo test --locked --workspace --features fdk-aac`
- `make smoke` (`zakuro-moq --help` に `--datagram-max-size` が出ること)
- tombi (`prek run tombi-lint --all-files` / `prek run tombi-format --all-files`)
- 経路ごとの datagram 往復テスト: QUIC 直接接続 (`stream.rs`)、WebTransport over HTTP/3
  (`wt_h3.rs`)、WebTransport over HTTP/2 (`wt_h2/tests.rs`)、QMux (`stream.rs`) を
  実トランスポートの loopback 接続で検証する。
- 実 relay の E2E (2026-10-25 に確認。接続先のホスト名はリポジトリに書かない):
  - subgroup 配送: `--tracks video:30:1000 --subscribe-tracks video --verify-payload` で
    publish=1/1、subscribe=1/1、896 送信 / 891 受信、payload-mismatches=0
  - datagram 配送: `--tracks audio:50:200:datagram --subscribe-tracks audio --verify-payload`
    で publish=1/1、subscribe=1/1、1492 送信 / 1484 受信、payload-mismatches=0
  - 件数の差は停止時の in-flight object による。publisher → relay → subscriber の datagram
    経路が実 relay で成立した。
