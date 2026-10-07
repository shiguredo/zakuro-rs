# MOQT publish の負荷試験 (Sora MoQ) を追加する

- Created: 2026-10-02
- Completed: 2026-10-02
- Branch: feature/add-moq-publish-load-test
- Polished: {YYYY-MM-DD}

## 目的

zakuro-rs は Sora WebRTC SFU の負荷試験ツールであり、仮想クライアントを大量に生成して SFU の性能を測る。
Sora MoQ (Media over QUIC 実装) は、MOQT のリレー機能を提供する。
MOQT 経路の負荷試験手段が無いため、zakuro-rs から MOQT で publish できるようにする。

C++ 版 zakuro に MOQ 対応は無く、zakuro-rs が最初の MOQ 負荷試験ツールになる。

## 現状

- 仮想クライアントは `src/virtual_client.rs` の `run` が sora_sdk 経由で WebRTC 接続する経路しか持たない
- `src/args.rs` の `parse_instance_args` は `--sora-signaling-url` / `--sora-channel-id` / `--sora-role` を必須引数として要求する
- MOQT のプロトコル実装は moqt-rs の `shiguredo_moqt` にある (Sans I/O、draft-ietf-moq-transport-21 準拠)。crates.io には未公開
- QUIC の実装は zakuro-rs に無い。依存は crates.io の公開クレートのみで構成されている

## 設計方針

### 依存

- MOQT は `shiguredo_moqt` を git 依存 (公開リポジトリの develop のコミットを rev 固定) で参照する。crates.io 未公開のため
- QUIC は `s2n-quic` を使う。TLS は rustls provider、datagram endpoint を有効にして `max_datagram_frame_size` を広告する (MOQT は object を datagram でも運ぶ。draft-ietf-moq-transport-21)
- TLS は `rustls`。CA 証明書の指定が無い場合は OS の信頼ストアを使う

### MOQ モード

- `--sora-moq-url` を指定したときだけ MOQ モードにする
- MOQ モードでは `--sora-signaling-url` / `--sora-channel-id` / `--sora-role` を要求しない。MOQ モードでこれらを指定した場合は起動エラーにする
- Sora モードでは従来どおり 3 つとも必須のままにする

### 接続と送信

- 1 仮想クライアント = 1 QUIC 接続。SETUP を交換したあと PUBLISH し、subgroup stream で object を送り続ける
- Track Name は仮想クライアントごとに一意にする (負荷試験では「どの仮想クライアントの object か」を区別できる必要がある)
- object の payload サイズと送信レートは引数で指定する
- `--duration` / `--repeat-interval` / `--max-retry` / `--retry-interval` と統計イベント (`src/stats.rs` の `StatsEvent`) は Sora モードと共通の仕組みを使う
- TLS 検証のスキップは既存の `--insecure` を使う

### 追加する引数 (案)

| 引数 | 説明 |
| --- | --- |
| `--sora-moq-url` | MOQ relay の URL (`moqt://host:port`)。指定すると MOQ モードになる |
| `--sora-moq-namespace` | Track Namespace (デフォルト: `zakuro`) |
| `--sora-moq-track-name` | Track Name の接頭辞 (デフォルト: `video`)。仮想クライアントごとに一意な suffix を付ける |
| `--sora-moq-object-rate` | 1 仮想クライアントあたりの object 送信レート (objects/sec、デフォルト: 30) |
| `--sora-moq-object-size` | 1 object の payload サイズ (bytes、デフォルト: 1000) |
| `--sora-moq-ca-cert` | relay の CA 証明書 (PEM)。未指定なら OS の信頼ストア |

## 完了条件

- `--sora-moq-url` を指定して起動すると、`--vcs` 個の QUIC 接続が確立し、MOQT の SETUP が完了し、PUBLISH が受理されて object が送信され続ける
- `--duration` 経過で切断し、`--repeat-interval` で再接続する既存のライフサイクルが MOQ モードでも動く
- 実際の Sora MoQ のリレーに対して疎通確認できる (relay のホストはリポジトリに書かない。実行時に引数で渡す)
- Sora モードの既存動作が変わらない (既存テストが通る)
- `make ci` が通る

## 変更対象

- `Cargo.toml`
- `src/args.rs`
- `src/main.rs`
- `src/moq_client.rs` (新規)
- `src/moq_client/transport.rs` (新規)
- `src/moq_client/session.rs` (新規)
- `src/duckdb_schema.sql` / `src/duckdb_stats/rows.rs` / `src/duckdb_stats/stats_json.rs`
- `README.md` / `docs/DUCKDB.md`

## 解決方法

### 依存

- `shiguredo_moqt` を git 依存 (公開リポジトリ `shiguredo/moqt-rs` の `develop` のコミットを rev 固定) で追加した。crates.io 未公開のため
- `s2n-quic` を `provider-tls-rustls` / `provider-address-token-default` / `unstable-provider-datagram` で追加した
- TLS は `rustls`、既定の信頼ストアは `webpki-roots` にした

### 実装

- `src/moq_client/transport.rs`: `moqt://` URL の解釈 (`MoqEndpoint::parse`)、名前解決、TLS 構築 (`--insecure` / `--sora-moq-ca-cert` / WebPKI)、s2n-quic クライアントの構築。datagram endpoint を有効にして `max_datagram_frame_size` を広告する
- `src/moq_client/session.rs`: MOQT セッションの駆動。自側制御ストリームに SETUP (PATH / AUTHORITY / MOQT_IMPLEMENTATION) を書き、peer の制御ストリームを受けて `Established` を待ち、`send_publish` で PUBLISH する。REQUEST_OK で受理を確認したら subgroup stream を開いて LOC の Timestamp / Timescale 付き object を送る。`REQUEST_UPDATE` には REQUEST_OK を返し、Forward State 0 の間は送信を止める。GOAWAY / close の終了処理も行う
- `src/moq_client.rs`: 仮想クライアントのライフサイクル。Track Name は `<接頭辞>-<instance>-<vc>` で一意にし、`--duration` / `--repeat-interval` / `--max-retry` / `--retry-interval` と `StatsEvent` は Sora モードと同じ規則で扱う
- `src/args.rs`: `--sora-moq-url` / `--sora-moq-namespace` / `--sora-moq-track-name` / `--sora-moq-object-rate` / `--sora-moq-object-size` / `--sora-moq-ca-cert` を追加した。MOQ モードでは `--sora-signaling-url` / `--sora-channel-id` / `--sora-role` を要求せず、同時指定はエラーにする。Sora モードでは従来どおり 3 つとも必須。JSONC では `sora` と同じく `sora-moq` オブジェクト (`sora-moq.url` → `--sora-moq-url`) と、最上位のみフラットな `sora-moq-*` キーの両方を受け付ける。`instances[i]` の中は `sora` と同じくオブジェクト形式のみ
- `src/main.rs`: MOQ モードのインスタンス起動 (`run_moq_instance`) を追加した。QUIC クライアントと名前解決はインスタンスで 1 回だけ行い、仮想クライアントで共有する。仮想クライアントのタスクは `spawn_local` ではなく `spawn` でランタイムのワーカースレッドに載せる (MOQ の仮想クライアントは Send な型だけで構成される。LocalSet の 1 スレッドに載せたところ 100 仮想クライアントで object の送信レートが要求値の 6 割まで落ちた)
- DuckDB の `zakuro_scenario` に `moq_*` 列を追加し、`config_json` にも MOQ 設定を出した

### レビューで見つかった問題の修正

実装後に差分の批判レビューを行い、次を修正した。

- `src/moq_client.rs` `run`: セッション future が完了した後も終了処理で再度 poll しており、
  `` `async fn` resumed after completion `` で panic していた。完了フラグを持ち、未完了のときだけ
  待つようにした (`wait_for_session_shutdown`)。確立前にセッションが終わると再接続に戻れず
  仮想クライアントが消えるバグだった
- `src/moq_client/session.rs` `accept_control_stream`: `MessageDecoder` を stream 間で共有して
  いたため、制御ストリーム以外の uni stream を先に受けると残りバイトを次の stream type として
  誤判定していた。stream ごとにデコーダを作り、制御ストリームのときはそのデコーダを返すように
  した
- `src/moq_client/session.rs` `run`: 制御ストリームの受信待ちに上限が無く、stream type を
  送らない peer で永久に止まっていた。`ESTABLISH_TIMEOUT` で打ち切るようにした
- `src/moq_client/session.rs` `run`: PUBLISH の応答が返らない場合の期限が無かった。
  `set_control_message_timeout_ms` を設定し、期限切れを `CloseSession` として受けて再接続に戻す
- `src/moq_client/session.rs` `Publisher`: subgroup stream への書き込み失敗でセッション全体を
  終了していた。書き込み失敗を stream 終端 (STOP_SENDING) とみなして `recv_data_stream_stop_sending`
  を通知し、次の Group から送り直すようにした (連続失敗には上限を設ける)
- `src/args.rs`: `--sora-moq-object-rate` の範囲を検証する (極端な値は `Duration` の変換で panic
  していた)。`--sora-moq-namespace` の予約名前空間 (`.` / `.session`) と 4096 バイト上限、
  `--sora-moq-track-name` の長さも起動時に検証する
- `src/moq_client/transport.rs`: 名前解決が v4/v6 を混在で返す場合に、最初のアドレスの
  ファミリのソケットしか作っておらず他方の候補が必ず失敗していた。ファミリごとに
  s2n-quic クライアントを作って使い分けるようにした。URL のフラグメントも取り除く
- 上記に加えて、`CloseSession` で接続を閉じる (`connection_handle.close`)、制御ストリーム
  送信の到達不能な分岐の削除、request stream 終端エラーのログ、MOQT_IMPLEMENTATION への
  バージョン付与、object 組み立てバッファの再利用、`config_json` への `moq_ca_cert` 出力を
  行った

### 確認

- `cargo fmt --all -- --check` / `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` / `cargo test --locked` (246 件成功) が通る
- 実 relay に対して 1 仮想クライアントで 20 秒実行し、SETUP 完了 → PUBLISH 受理 → 594 object 送信 (約 30 objects/sec) を確認した
- 3 仮想クライアントで 25 秒間の連続 publish が維持されること (制御メッセージのタイムアウトで切られないこと) を確認した
- 100 仮想クライアント (hatch rate 20) で PUBLISH 受理が 100 件、接続・セッション失敗と panic が 0 件であることを確認した
- 100 仮想クライアント × 30 objects/sec × 10 秒で、各仮想クライアントが 294 - 300 object を送信した (要求レートどおり。合計約 3000 objects/sec)
- 10 仮想クライアント同時接続 (hatch rate 5) と、`--duration` 経過 → `--repeat-interval` 後の再接続を確認した
- `zakuro_scenario` に MOQ 設定行が入ることを実ファイルで確認した
- 疎通確認に使った relay のホストはリポジトリに書かない (実行時に `--sora-moq-url` で渡す)


### 0025 での扱い

issue 0025 で MOQ を独立バイナリ `zakuro-moq` に分離したため、この issue で追加した
`--sora-moq-*` オプションと MOQ モード、`moq_*` 列は `zakuro` から削除した。
MOQ の実装は `zakuro-moq` へ移り、内容 (SETUP / PUBLISH / object 送信 / リトライ) は
そのまま引き継いでいる。上の「確認」は MOQ モード時代に実測した記録であり、分離後の
最終確認は `make ci` (`zakuro` 215 件 + `zakuro-moq` 41 件) と、`zakuro-moq` で複数トラックを
publish する実 relay 疎通 (issue 0026 の確認を参照) で行っている。
