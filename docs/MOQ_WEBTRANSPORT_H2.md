# WebTransport over HTTP/2 対応の検討メモ

zakuro-moq が WebTransport over HTTP/2 (`--transport wt-h2`) 経由で MOQT relay へ接続する
ための検討結果と実装方針。仕様は draft-ietf-webtrans-http2-15、依存は `shiguredo_http2`
`2026.1.0-canary.16` と `shiguredo_moqt` rev `49c711e` を前提とする。本文中の節番号は
draft 由来であり、改訂や RFC 化で変わりうる。

## 結論

- `--transport wt-h2` を追加する。TCP + TLS (ALPN `h2`) の 1 本の HTTP/2 接続の上で
  Extended CONNECT と Capsule Protocol を使い、MOQT のストリームと DATAGRAM を運ぶ。
- UDP が不要である。reset を `WT_RESET_STREAM` capsule で行うため `RESET_STREAM_AT` も
  不要であり、wt-h3 と違って ngtcp2 を必要としない。
- relay が WebTransport over HTTP/2 を終端できる必要がある experimental な経路である。
- QUIC を使わないため `--quic-backend` は指定できない (qmux と同じ扱い)。
- 実装は `zakuro-moq/src/moq_client/wt_h2.rs` に閉じ、HTTP/2 と WebTransport の状態機械は
  `shiguredo_http2` (Sans I/O) に委譲する。駆動タスクが I/O とセッションを所有し、
  `Handle` / ストリーム / acceptor がチャネルで操作する ngtcp2 経路と同形式にする。

## 1. 仕様上の位置づけと判断

### 1.1 ALPN / SETTINGS / Extended CONNECT

HTTP/2 の ALPN は `h2` である (RFC 9113 §3.3)。MOQT のプロトコル識別子 `moqt-22` は
TLS の ALPN ではなく、Extended CONNECT の `WT-Available-Protocols` / `WT-Protocol` で
交渉する (draft-ietf-moq-transport-22 §6.2 (Session establishment))。

- クライアントは `SETTINGS_ENABLE_CONNECT_PROTOCOL=1` と `SETTINGS_WT_ENABLED=1`、
  WT の初期フロー制御値を SETTINGS で広告する (draft-ietf-webtrans-http2-15 §3.1 / §4.3.1)。
  初期値は QUIC 直接接続と同じ値にし、経路の違いでフロー制御の性質を変えない。
- ピアが両方の SETTINGS を広告するまで CONNECT を送らない (§3.1 の MUST NOT)。
- CONNECT は `:method=CONNECT`、`:protocol=webtransport`、`:scheme=https`、
  `:authority`、`:path` を設定する (§3.1 / §3.2)。`WT-Available-Protocols` には
  RFC 8941 の sf-string として `"moqt-22"` を 1 要素の List で載せる。
- 2xx 応答と `WT-Protocol: moqt-22` を検証してからストリームを開く (§3.3)。

### 1.2 MOQT の SETUP

draft-ietf-moq-transport-22 §9.1.1 (AUTHORITY) / §9.1.2 (PATH) は WebTransport での送信を
MUST NOT とする。wt-h3 と同じく `Transport::WebTransport` を `Session` に渡し、SETUP に
AUTHORITY / PATH を載せない。

### 1.3 ストリーム ID と種別

WT のストリーム ID は QUIC のストリーム ID を 2 bit の規約ごと写したものである
(draft-ietf-webtrans-http2-15 §5.2)。MOQT の `DataStreamId` には WT のストリーム ID を
そのまま使う。control stream と subgroup stream は単方向、request stream は双方向である。

ピア起動ストリームは `WT_MAX_STREAMS` でクレジット管理し (§6.7)、双方向・単方向それぞれ
両方向が閉じた時点で上限を 1 増やす。

### 1.4 reset と STOP_SENDING

- 送信方向の reset は `WT_RESET_STREAM` capsule で行う (§6.2)。MOQT のストリームリセット
  エラーコード (draft-ietf-moq-transport-22 §12.5) をそのまま載せる。
- 受信した reset は読み込みエラーに [`StreamReset`](../zakuro-moq/src/moq_client.rs) を
  載せ、`session.rs` が downcast して MOQT のストリーム reset として扱う。
- `WT_STOP_SENDING` を受けた送信ストリームは、以降の write / finish をエラーにする (§6.3)。
  エラーにはピアが載せたアプリケーションエラーコードを保持する。
- 送信側も同じコードを失わない。データストリームの書き込みが STOP_SENDING に由来する
  エラーで失敗した場合、`session.rs` は [`StreamReset`](../zakuro-moq/src/moq_client.rs) から
  コードを取り出して MOQT の `recv_data_stream_stop_sending` に渡す。request stream の
  書き込みが STOP_SENDING で失敗した場合はセッションを終了せず、受信方向で観測する peer の
  終端 (FIN / reset) を待つ (MOQT crate は同じ request の二重 close を
  PROTOCOL_VIOLATION とするため、即時に close を通知しない)。

### 1.5 フロー制御

- 受信は `WT_MAX_DATA` / `WT_MAX_STREAM_DATA` を、アプリが読んだ分に応じて credit 方式で
  拡張する (§6.5 / §6.6)。読んでいない分を超えて広げないため、アプリの停滞がメモリの
  増加にならない。
- `WT_STOP_SENDING` 後に届いたデータは破棄する。ストリーム単位のウィンドウは
  STOP_SENDING 後に広告してはならない (§6.6 の MUST NOT) が、破棄した分も接続全体の
  受信クレジットとして返す (§6.5)。返さないと、飛行中のデータを破棄し続けたときに
  セッションの受信ウィンドウが枯渇して受信が恒久停止する。
- HTTP/2 の接続・ストリームの受信ウィンドウは、受信の都度 WINDOW_UPDATE で返す
  (RFC 9113 §6.9.1)。WT のデータは CONNECT ストリーム 1 本に集約されるため、
  HTTP/2 の接続ウィンドウがスループットの上限になる。既定値 (65535) では
  WINDOW_UPDATE の往復で頭打ちになるため、接続・初期ストリームとも 16 MiB に広げる。

### 1.6 close

WT_CLOSE_SESSION を送った直後に CONNECT ストリームを END_STREAM で half-close する
(draft-ietf-webtrans-http2-15 §6.12 の MUST)。ピアからの WT_CLOSE_SESSION に対しても
END_STREAM で応答する。CONNECT ストリームの reset / 終了はセッション終了として扱う
(§3.4)。

WT_CLOSE_SESSION の Application Error Code は 32 ビットである。MOQT のローカル専用コード
(`SESSION_LOCAL_*` = `0xFFFF_FFFF_FFFF_FFxx`) は wire に載せられないため NO_ERROR として
送り、警告ログを出す。

### 1.7 DATAGRAM

zakuro-moq は datagram 配送を指定したトラックの object を `WT_DATAGRAM` capsule で送受信する
(draft-ietf-webtrans-http2-15 §6.13)。capsule は HTTP/2 の DATA 上を流れるため、QUIC
DATAGRAM と違い経路損失では失われない。

### 1.8 TLS

`--ca-cert` / `--insecure` の扱いは QMux と同じである。WebPKI のルート証明書を使い、
`--ca-cert` を指定した場合はその PEM だけを信頼する。TLS 1.3 のみを使う。

### 1.9 keep-alive

MOQT には PING が無く、wt-h2 は HTTP/2 の上で動くため QUIC の idle timeout に相当する
機構が無い。無通信の接続 (Forward State 0 など) が relay や中間装置の idle timeout で
切られないよう、HTTP/2 PING を 5 秒間隔で送る。間隔は ngtcp2 経路の
`keep_alive_timeout=5s` に合わせる。

## 2. 実装構造

- `zakuro-moq/src/moq_client/wt_h2.rs`: 接続 (TCP + TLS + HTTP/2 + Extended CONNECT)、
  HTTP/2 と WT を駆動するタスク、`Handle` / `SendStream` / `RecvStream` / acceptor。
- `zakuro-moq/src/moq_client/wt_h2/tests.rs`: 実 TLS の in-process HTTP/2 サーバー
  (`shiguredo_http2` のサーバー接続 + rcgen の自己署名証明書) による結合テスト。
  モック・スタブは使わない。
- `stream.rs`: 経路ごとの enum に `WtH2` を追加。
- `transport.rs`: `MoqTransport::WtH2`、ALPN `h2` の TLS 設定、接続先候補ごとの
  タイムアウト付き接続。
- `session.rs`: `Transport::WebTransport`、読み込み・書き込みエラーの `StreamReset`
  downcast (request stream の STOP_SENDING ではセッションを落とさない)。
- `args.rs`: `--transport wt-h2` の解釈と `--quic-backend` 併用の拒否。

## 3. 制約と未対応

- relay が WebTransport over HTTP/2 の SETTINGS、Extended CONNECT、capsule
  (`WT_STREAM` / `WT_RESET_STREAM` / `WT_STOP_SENDING` / `WT_MAX_DATA` など) を実装して
  いる必要がある。experimental な経路として扱う。
- TCP の再送遅延に加え、すべての MOQT ストリームが HTTP/2 の CONNECT ストリーム 1 本に
  多重化されるため、経路でパケット損失が起きると全ストリームが影響を受ける (HTTP/2 の
  head-of-line blocking)。遅延を重視する試験では QUIC 直接接続を使うこと。
- relay が WT の初期ストリーム上限を SETTINGS で広告せず `WT_MAX_STREAMS` も送らない場合、
  クライアントのストリーム open は空きを待ち続ける。接続確立 (SETTINGS と CONNECT) には
  タイムアウトを設けているが、ストリーム open 自体のタイムアウトは無い (QUIC 経路と同じ性質)。
- `WT_DATAGRAM` は TCP 上で信頼的に届く。QUIC DATAGRAM のような経路損失は無いが、遅延特性は
  QUIC 直接接続と異なる。
- WT_CLOSE_SESSION のコードが 32 ビットのため、MOQT のローカル専用クローズコードは
  wire に載せられない。
- 実リレー E2E: リレー再構築後の 2026-10-25 に、テスト用リレー (接続先のホスト名は
  リポジトリに書かない) へ `--transport wt-h2` で接続し、publish + subscribe の往復を
  確認した。
  - VC 1 本 20 秒: publish=1/1、subscribe=1/1、600 objects 送信 / 599 受信、
    payload-mismatches=0
  - VC 5 本 15 秒 (video + audio の 2 トラック): 5990 送信 / 5937 受信、
    payload-mismatches=0
  - VC 2 本 15 秒 (100 KB object): 64 送信 / 63 受信 (6.3 MB)、payload-mismatches=0
  件数の差は停止時の in-flight object による。relay の SETTINGS / Extended CONNECT /
  WT-Protocol / capsule / WT フロー制御は実装相互で動作した。
- なお再構築前の moqt-rs 側 E2E は、`examples/tokio-moq/src/moqt_client.rs` の
  `establish_wt_h2` が `take_uni_receiver` の後に `accept_uni_stream` を呼ぶため
  `StreamClosed` になるクライアント側のバグで失敗しており、relay 非対応によるものでは
  なかった。zakuro-moq 側の実装で capsule 段まで確認している。

## 4. 検証

- `cargo fmt --all -- --check`
- `cargo clippy --locked -p zakuro-moq --all-targets -- -D warnings`
- `cargo test --locked -p zakuro-moq`: 全テスト成功 (wt_h2 関連 28 件)。
  wt_h2 のテストは 20 回反復して flaky が無いことを確認した。
- wt_h2 のテストは実 TLS の in-process サーバーで、Extended CONNECT のヘッダー /
  SETTINGS / WT-Protocol の検証、単方向・双方向ストリームの往復、ピア起動ストリームの
  accept、reset / STOP_SENDING の surfacing、小ウィンドウでの blocked write の再開、
  18 MiB 受信による HTTP/2 ウィンドウ拡張、WT_MAX_STREAMS による open の再開、
  WT_STOP_SENDING 後の破棄データによるセッションクレジット回復、keep-alive PING、
  WT_CLOSE_SESSION の送受信と END_STREAM 応答、接続断で acceptor が `Ok(None)` を返す
  ことを検証する。WT_DATAGRAM capsule の送受信は `received_datagram_is_forwarded` と
  `send_datagram_over_http2` で検証する。
- 実リレー E2E (2026-10-25 に確認):

  ```bash
  ./target/debug/zakuro-moq \
    --url moqt://relay.example.com/ \
    --transport wt-h2 \
    --namespace zakuro-wt-h2-e2e \
    --tracks video:30:1000 \
    --subscribe-tracks video \
    --verify-payload \
    --vcs 1 \
    --duration 20
  ```
