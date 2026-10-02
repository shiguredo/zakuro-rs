# zakuro-moq を独立バイナリとして分離する

- Created: 2026-10-02
- Completed: {YYYY-MM-DD}
- Branch: feature/change-split-zakuro-moq-binary
- Polished: {YYYY-MM-DD}

## 目的

MOQ (Media over QUIC Transport) の負荷試験機能を `zakuro` のモードとして実装したが、
Sora (WebRTC) と MOQ では設定スキーマ・必要な依存・利用者がまったく異なるため、
独立したバイナリ `zakuro-moq` に分ける。

現在のモード方式には次の問題がある。

- `zakuro` のビルドには libwebrtc の prebuilt ダウンロードが必須で、配布対象が
  macOS (Apple Silicon) と ubuntu に限られる。MOQ は libwebrtc を必要としないのに
  この制約を引き継いでしまう
- `--sora-moq-url` と `--sora-signaling-url` の相互排他、`instances` 配列でのモード混在
  ルールなど、モードが同じ設定ファイルに同居することによる検証が増えている
- MOQ の設定はトラックごとの指定 (複数トラック・レート・サイズ) へ広がるため、
  Sora の設定キーと同居させる利点が無い
- `zakuro --help` に Sora 向けの 50 個のオプションと MOQ の 6 個が混在する

## 現状

- 単一パッケージ (ルート `Cargo.toml`) に `src/main.rs` があり、MOQ モードは
  `src/moq_client.rs` / `src/moq_client/transport.rs` / `src/moq_client/session.rs` と
  `src/args.rs` の `--sora-moq-*`、`src/main.rs` の `run_moq_instance` で実装されている
- MOQ 側の webrtc 依存はログマクロ (`rtc_log_*`) のみである (実測: `src/moq_client.rs` と
  `src/moq_client/session.rs` の 2 箇所。`shiguredo_webrtc::log::print(severity, file, line, message)`
  という低レベル API があるため、ログの転送は file/line を保ったまま実装できる)
- 共有インフラ (`src/stats.rs` / `src/http_server.rs` / `src/json_rpc.rs` /
  `src/duckdb_stats/writer.rs`) の webrtc 依存もログマクロだけである
- `Makefile` は既に `--workspace` を付けて cargo を呼んでいる
- `src/args.rs` は 4000 行で大半が Sora 固有 (映像・音声・コーデック・DataChannel・シナリオ)
- libwebrtc を使わない `cargo check --lib` は存在しない (lib ターゲットが無い)

## 設計方針

### 構成

バイナリごとにディレクトリを分け、ルートはワークスペースの仮想マニフェストだけにする。
ビルド成果物は `./target/{debug,release}` に集約される。

```
Cargo.toml             ワークスペース定義と [profile.release]
zakuro/Cargo.toml      Sora 版 (バイナリ zakuro)
zakuro-moq/Cargo.toml  MOQ 版 (バイナリ zakuro-moq)
zakuro-core/Cargo.toml 共有基盤 (ライブラリ)
```

- `zakuro/`: Sora の負荷試験バイナリ。MOQ モードを削除する
- `zakuro-moq/`: MOQ の負荷試験バイナリ。依存は `shiguredo_moqt` / `s2n-quic` /
  `rustls` / `webpki-roots` / `tokio` / `zakuro-core` のみ (libwebrtc を引かない)
- `zakuro-core/`: 両者で共有する webrtc 非依存の基盤 (ログの受け口、統計集計、
  HTTP サーバー / JSON-RPC)

### ログ

`zakuro-core` はログ基盤として `log` クレートのファサードを使う (ライブラリ側は
ファサード、出力先はバイナリ側が決める)。

- `zakuro` は `log::Log` 実装から `shiguredo_webrtc::log::print` へ転送し、現在の出力形式
  (`[000:123][456] (zakuro::main.rs:403): message`) を保つ
- `zakuro-moq` は `tracing` + `tracing-subscriber` を使い、`log` の記録は
  `tracing-log` 経由で取り込む
- これにより `zakuro-core` は shiguredo_webrtc を参照しない

### CLI と JSONC

専用バイナリになるため、MOQ のオプションは接頭辞を外す (`--url` / `--namespace` /
`--tracks` / `--object-rate` / `--object-size` / `--ca-cert` / `--insecure`)。
JSONC もトップレベルに同じ名前のキーを書く (`sora` / `sora-moq` の入れ子は使わない)。

### 完了条件

- `cargo tree -p zakuro-moq` に shiguredo_webrtc / sora_sdk / duckdb が出てこないこと
- `cargo build -p zakuro-moq` が libwebrtc の prebuilt ダウンロード無しで成功すること
- `zakuro` (Sora) の既存動作が変わらないこと (既存テストと起動確認)
- `zakuro` から MOQ モードの実装・引数・検証が消えていること
- `make ci` が通ること
- `zakuro-moq` が実際の sora-moq relay に接続して publish できること (relay のホストは
  リポジトリに書かず実行時に渡す)

## 変更対象

- `Cargo.toml` (ワークスペース化)
- `zakuro-core/` (新規)
- `zakuro-moq/` (新規)
- `src/` (MOQ モードの削除と共有部分の委譲)
- `.github/workflows/ci.yml` / `release.yml`
- `README.md` / `docs/DUCKDB.md`

## 解決方法

### ワークスペース構成

バイナリごとのディレクトリに分け、ルートは仮想ワークスペースのマニフェストにした。

- ルート `Cargo.toml`: `[workspace] members = ["zakuro", "zakuro-core", "zakuro-moq"]` と
  `resolver = "3"`、`[profile.release]` のみを置く (profile はワークスペースルートでのみ有効)
- `zakuro/`: 従来の `src/` `build.rs` `testdata/` とパッケージの `Cargo.toml` を移動した。
  テストの fixture は `CARGO_MANIFEST_DIR` 基準で解決されるため `testdata/` も一緒に移した
- `zakuro-core/`: `stats` (接続数の集計)・`http_server`・`json_rpc` を `zakuro/src/` から移動。
  `shiguredo_webrtc` を参照しないライブラリにする
- `zakuro-moq/`: MOQ の負荷試験バイナリ。`zakuro/src/moq_client.rs` と `zakuro/src/moq_client/{transport,session}.rs`
  を移動し、CLI / JSONC / ログ / エントリポイントを新規に書いた

### ログ

`zakuro-core` は `log` クレートのファサードを使う。出力先はバイナリ側が決める。

- `zakuro` は `zakuro/src/log_bridge.rs` で `log::Log` を実装し、`shiguredo_webrtc::log::print` へ転送する。
  ファイル表示は `rtc_log_*` マクロと同じ `crate::file.rs` 形式に整えるため、`log` の Record が持つ
  モジュールパス (target) の先頭とファイル名を組み合わせる。`--log-level` は `log::set_max_level` にも
  反映する
- `zakuro-moq` は `tracing` + `tracing-subscriber` を使い、`log` の記録は `tracing-log` で取り込む

### HTTP API のサービス名

`DefaultHandler` はクレート定数の `env!("CARGO_PKG_NAME")` を使えなくなった (共有クレート名が
返るため)。`DefaultHandler::new(name, version)` としてバイナリから渡す形にした。

### zakuro (Sora) からの削除

- `src/main.rs`: `run_moq_instance` と MOQ モード分岐、`mod moq_client` を削除
- `src/args.rs`: `--sora-moq-*` の 6 オプション、`InstanceArgs` の MOQ フィールド、MOQ モードの
  相互排他検証、`sora-moq` オブジェクトの JSONC 展開、関連テストを削除
- `src/duckdb_stats/{rows,stats_json}.rs` と `src/duckdb_schema.sql`: `moq_*` 列を削除
- `Cargo.toml`: `shiguredo_moqt` / `s2n-quic` / `rustls` / `webpki-roots` を削除

### zakuro-moq の CLI

専用バイナリのため接頭辞を外した (`--url` / `--namespace` / `--tracks` / `--object-rate` 相当は
`--tracks` に統合 / `--ca-cert` / `--insecure`)。JSONC もトップレベルに同じ名前のキーを書く。
`instances` 配列による複数インスタンスと、最上位キーのテンプレート継承は `zakuro` と同じ規則にした。

### 確認

- `cargo tree -p zakuro-moq` に `shiguredo_webrtc` / `sora_sdk` / `duckdb` が出てこない (0 件)
- `zakuro-moq` のビルドは libwebrtc の prebuilt ダウンロードを行わない
- `make ci` が通る (`zakuro` 215 件 + `zakuro-moq` 41 件 + `zakuro-core` 0 件)
- `./target/debug/zakuro-moq --help` が共通オプションとインスタンスオプションの両方を表示する
- 実 relay に対して複数トラックの publish が受理される (issue 0026 の確認を参照)
- `zakuro` の HTTP API (`/.ok` / `GetVersion`) が `zakuro` を返す (共有クレート化で `zakuro-core` に
  ならない) ことを確認した
- 疎通確認に使った relay のホストはリポジトリに書かない
