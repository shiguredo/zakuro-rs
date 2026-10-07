# Recording Composition Tool Zakuro

[![ci](https://github.com/shiguredo/zakuro-rs/actions/workflows/ci.yml/badge.svg?branch=develop)](https://github.com/shiguredo/zakuro-rs/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

## About Shiguredo's open source software

We will not respond to PRs or issues that have not been discussed on Discord. Also, Discord is only available in Japanese.

Please read <https://github.com/shiguredo/oss/blob/master/README.en.md> before use.

## 時雨堂のオープンソースソフトウェアについて

利用前に <https://github.com/shiguredo/oss> をお読みください。

## Recording Composition Tool Zakuro について

負荷試験ツール `zakuro` の Rust 実装です。次の 2 つのバイナリを 1 つのリポジトリで管理しています。

| バイナリ | ディレクトリ | 対象 | 主な依存 |
|---|---|---|---|
| `zakuro` | `zakuro/` | Sora WebRTC SFU | libwebrtc (Sora Rust SDK)・DuckDB |
| `zakuro-moq` | `zakuro-moq/` | Sora MoQ (Media over QUIC 実装) のリレー | s2n-quic・moqt-rs (`shiguredo_moqt`) |

両者で共有する基盤 (統計・HTTP サーバー・JSON-RPC) は `zakuro-core/` に置いています。

`zakuro` は仮想クライアントを複数起動し、フェイク映像・実デバイス映像・ Y4M・ MP4 パススルー・ WAV 音声を使って Sora へ接続できます。HTTP ヘルスチェック・ JSON-RPC・ DuckDB 統計出力も提供します。
`zakuro-moq` は仮想クライアントごとに QUIC 接続を 1 本確立し、複数のトラックへ MOQT で object を送り続けます。libwebrtc も DuckDB も使いません。

C++ 版との対応表・実装状況は `docs/ZAKURO.md`、DuckDB のスキーマは `docs/DUCKDB.md` を参照してください。

## 主な機能 (`zakuro`)

- 複数の Zakuro インスタンス / 仮想クライアントを段階的に起動
- Sora への `sendonly` / `recvonly` / `sendrecv` 接続
- フェイク映像 (Raden デジタル時計)・砂嵐・Y4M・実カメラ・MP4 パススルー送信
- フェイク音声・WAV 入力・MP4 内の音声トラック送信 (Opus / AAC)
- 映像 / 音声コーデック・パラメータ・エンコーダー実装の指定
- OpenH264 エンコード・NopVideoDecoder
- DataChannel メッセージング・サイマルキャスト・スポットライト
- 再接続シナリオ
- mTLS・TLS 検証スキップ
- DuckDB 統計出力・JSONC 設定ファイル・lint / fmt サブコマンド
- HTTP API (`GET /.ok` / `POST /rpc` / `GetVersion`)・ログレベル制御

## 必要環境

- Rust toolchain (`Cargo.toml` の `rust-version` は 1.99)
- `rustfmt` と `clippy`
- ビルド時に GitHub Releases へのネットワークアクセス
- Linux: `libpulse-dev` と `libx11-dev` (AAC デコードを使う場合は `libfdk-aac-dev` も)
- `make cover` には `cargo-llvm-cov`

### ビルド時に取得されるネイティブライブラリ

| 依存 | 取得元 | リンク方法 | 実行時依存 |
|---|---|---|---|
| DuckDB | prebuilt の**共有ライブラリ**をダウンロード | 共有 | あり |
| libwebrtc | 時雨堂の prebuilt の**静的ライブラリ**をダウンロード | 静的 | なし |
| Opus | 時雨堂の prebuilt の**静的ライブラリ**をダウンロード | 静的 | なし |

- DuckDB は `.cargo/config.toml` の `DUCKDB_DOWNLOAD_LIB` で prebuilt をダウンロードしてリンクします。cargo はリポジトリ内の config を読むため、**clone したディレクトリ内でのビルド**でしか効きません
- libwebrtc の prebuilt は ubuntu 22.04 / 24.04 / 26.04 と Raspberry Pi OS 向けです。それ以外は `WEBRTC_C_TARGET` にターゲット名を設定してください
- macOS は Apple Silicon のみ対応です。Intel Mac 向けの prebuilt はありません

### 実行時にユーザー側で用意するライブラリ

| 機能 | 必要なもの | 指定方法 |
|---|---|---|
| H.264 エンコード | OpenH264 の共有ライブラリ | `--openh264 <PATH>` |
| MP4 内の AAC 音声デコード | libfdk-aac (Linux + `--features fdk-aac`) | 実行時に `libfdk-aac.so.2` を動的ロード |

いずれも zakuro 側では取得・同梱しません。

## インストール

GitHub Releases から環境に合うアーカイブを展開します。DuckDB の共有ライブラリは同梱されているため、追加の環境変数は不要です。アーカイブ名は `zakuro-<バージョン>-<プラットフォーム>-<アーキテクチャ>.tar.gz` です。

```bash
# 例: ubuntu-24.04 x86_64
curl -LO https://github.com/shiguredo/zakuro-rs/releases/download/<バージョン>/zakuro-<バージョン>-ubuntu-24.04-x86_64.tar.gz
tar -xzf zakuro-<バージョン>-ubuntu-24.04-x86_64.tar.gz
./zakuro-<バージョン>-ubuntu-24.04-x86_64/zakuro --help
```

配布対象は ubuntu-24.04 / ubuntu-26.04 (x86_64 / aarch64) と macOS 15 以降 (Apple Silicon) です。SHA256 チェックサムは同じリリースに `*.sha256` として添付されます。

## ビルドと実行

```bash
cargo build                   # debug ビルド
cargo build --release         # release ビルド
cargo run -- --help           # cargo 経由で実行する
./target/debug/zakuro --help  # ビルドした実行ファイルを直接実行する
```

ビルドした実行ファイルには DuckDB 共有ライブラリの探索パス (rpath) が埋め込まれるため、追加の環境変数なしで直接起動できます。

## 動作確認済み環境

| 環境 | ビルド | テスト | 確認経路 |
|---|---|---|---|
| ubuntu-24.04 (x86_64) | 確認済み | 264 件成功 | CI |
| ubuntu-26.04 (x86_64) | 確認済み | 264 件成功 | CI |
| macOS (Apple Silicon) | 確認済み | 262 件成功 | 開発マシン |
| ubuntu (arm64) | 未確認 | 未確認 | CI に job なし |
| Windows | 未確認 | 未確認 | CI に job なし |
| Intel Mac | 不可 | 不可 | prebuilt なし |

Linux でテスト件数が 2 件多いのは、AAC デコードの検証 2 件が Linux + feature `fdk-aac` 限定のためです。

## 対応 Sora

- WebRTC SFU Sora 2025.2.0 以降

## 開発コマンド

```bash
make ci      # 整形確認・clippy・テスト・実行ファイル起動確認 (CI と同じ入口)
make fmt     # 整形して書き換える
make test    # テストのみ
make cover   # カバレッジ付きでテスト
```

## 実行例

### 最小構成で送信する

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-test \
  --sora-role sendonly
```

### 仮想クライアントを 50 個、毎秒 10 個ずつ起動する

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-load \
  --sora-role sendonly \
  --vcs 50 \
  --vcs-hatch-rate 10
```

### MP4 パススルーで送信する

`--input-mp4` はエンコード済み映像を再エンコードせずに送信します。`--sora-video-codec-type` と `--sora-video-bit-rate` が必須です。

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-mp4 \
  --sora-role sendonly \
  --input-mp4 ./video.mp4 \
  --sora-video-codec-type h264 \
  --sora-video-bit-rate 2000
```

### 受信専用で HTTP API を有効にする

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-rpc \
  --sora-role recvonly \
  --http-host 127.0.0.1 \
  --http-port 8080
```

## JSONC 設定ファイル

`--config` で JSONC 設定ファイルを読み込めます。CLI 引数は設定ファイルより優先されます。

```jsonc
{
  "sora-signaling-url": "wss://sora.example.com/signaling",
  "sora-channel-id": "zakuro-config",
  "sora-role": "sendonly",
  "vcs": 10,
  "vcs-hatch-rate": 5,
  "resolution": "HD",
  "framerate": 30,
  "http-host": "127.0.0.1",
  "http-port": 8080
}
```

1 プロセスで複数の Zakuro インスタンスを起動するには `instances` 配列を使います。

```jsonc
{
  "instance-hatch-rate": 1.0,
  "instances": [
    {
      "vcs": 50,
      "sora": {
        "signaling-url": "wss://sora.example.com/signaling",
        "channel-id": "zakuro-send",
        "role": "sendonly"
      }
    },
    {
      "vcs": 100,
      "sora": {
        "signaling-url": "wss://sora.example.com/signaling",
        "channel-id": "zakuro-recv",
        "role": "recvonly"
      }
    }
  ]
}
```

`zakuro lint <FILE.jsonc>` は設定ファイルを検証し、`zakuro fmt <FILE.jsonc>` は整形します (`--check` で書き戻さず確認)。

最上位の `"log-suppress"` は文字列の配列でも指定できます (`--log-suppress` の値に変換されます)。

## MOQ 版 (`zakuro-moq`)

Sora MoQ (Media over QUIC 実装) のリレーに対する publish 負荷試験を行います。
1 仮想クライアント = 1 QUIC 接続 = 1 MOQT セッションで、指定した複数トラックを同時に
publish します。Track Name は `<トラック名>-<インスタンス>-<仮想クライアント>` として
仮想クライアントごとに一意化します。

```bash
cargo run -p zakuro-moq -- \
  --url moqt://relay.example.com:4433 \
  --namespace zakuro \
  --tracks video:30:1000,audio:50:200 \
  --vcs 50 \
  --vcs-hatch-rate 10 \
  --duration 60
```

TLS は既定で WebPKI のルート証明書を使って検証します。relay の CA 証明書を指定する場合は
`--ca-cert <PEM>`、検証をスキップする場合は `--insecure` を使います。

MOQT では、購読者がいないトラックの送信を止めるよう relay から `REQUEST_UPDATE`
(FORWARD=0) が届くことがあります。zakuro-moq はこれに従います (FORWARD はトラック単位で
扱います)。送受信の状況は 5 秒ごとに
`[stats] objects-sent=... send-rate=.../s objects-received=... recv-rate=.../s bytes-received=... payload-mismatches=...`
としてログに出ます。

### 購読する (`--subscribe-tracks`)

`--subscribe-tracks` を指定すると、そのトラックを購読して object を受信します。
Full Track Name は publish と同じく `<トラック名>-<インスタンス>-<仮想クライアント>` なので、
publish 側と subscribe 側で同じ `--vcs` を指定すると 1 対 1 で対応します。

```bash
# 配信側
cargo run -p zakuro-moq -- --url moqt://relay.example.com:4433 --namespace zakuro \
  --tracks video:30:1000,audio:50:200 --vcs 10

# 視聴側 (別プロセス)
cargo run -p zakuro-moq -- --url moqt://relay.example.com:4433 --namespace zakuro \
  --subscribe-tracks video,audio --vcs 10
```

`--subscribe-tracks` を指定して `--tracks` を省略した場合は publish しません
(購読専用で起動できます)。両方を指定すると 1 つの仮想クライアントが publish と subscribe を
同時に行います。

トラック名に `{instance}` / `{vc}` を書くと仮想クライアントの値で置換します。プレースホルダを
書かない場合は publish と同じ `<名前>-<インスタンス>-<仮想クライアント>` になります。
1 つの publisher に対して複数の購読者をぶら下げる場合は `{vc}` を固定します。

```bash
# 配信側 (1 仮想クライアント = 1 トラック video-0-0)
cargo run -p zakuro-moq -- --url moqt://relay.example.com:4433 --namespace zakuro \
  --tracks video:30:1000 --vcs 1

# 視聴側 (10 仮想クライアントが同じ video-0-0 を購読する)
cargo run -p zakuro-moq -- --url moqt://relay.example.com:4433 --namespace zakuro \
  --subscribe-tracks 'video-{instance}-0' --vcs 10 --vcs-hatch-rate 5
```

`--verify-payload` を付けると、受信した payload が zakuro-moq の publisher が送る
パターン (`位置 % 251`) と一致するかを検査し、不一致数をログに出します。実メディアを配信する
relay へ接続する場合は誤検知になるため既定では無効です。

### 主なオプション (`zakuro-moq`)

| オプション | 説明 |
| --- | --- |
| `--url` | MOQ relay の URL (`moqt://host:port`)。必須 |
| `--namespace` | Track Namespace (デフォルト: `zakuro`) |
| `--tracks` | publish するトラック (`名前[:レート[:サイズ]]` のカンマ区切り、デフォルト: `video`。`--subscribe-tracks` 指定時は publish しない) |
| `--subscribe-tracks` | 購読するトラック名のカンマ区切り。`{instance}` / `{vc}` を書くと置換し、書かなければ `<名前>-<インスタンス>-<仮想クライアント>` になる |
| `--verify-payload` | 受信 payload が zakuro-moq の publisher のパターンと一致するかを検査する |
| `--ca-cert` | relay の CA 証明書 (PEM、未指定なら WebPKI のルート証明書) |
| `--vcs` | 仮想クライアント数 (1-1000、デフォルト: 1) |
| `--vcs-hatch-rate` | 仮想クライアントの起動レート (毎秒、デフォルト: 1.0) |
| `--duration` | 接続維持秒数 (未指定なら無制限) |
| `--repeat-interval` | `--duration` 経過後の再接続間隔 (秒) |
| `--max-retry` / `--retry-interval` | 接続失敗時のリトライ回数と間隔 (デフォルト: 0 / 60.0) |
| `--insecure` | TLS 証明書の検証をスキップ |
| `--log-level` | ログレベル (`verbose` / `info` / `warning` / `error` / `none`、デフォルト: `info`) |
| `--http-host` / `--http-port` | HTTP API (`GET /.ok` / `POST /rpc`) を有効化 |
| `--config` | JSONC 設定ファイル |
| `--instance-hatch-rate` | インスタンスの起動レート (毎秒、デフォルト: 1.0) |

### JSONC 設定ファイル (`zakuro-moq`)

```jsonc
{
  "instance-hatch-rate": 1.0,
  "url": "moqt://relay.example.com:4433",
  "namespace": "zakuro",
  "tracks": [
    { "name": "video", "object-rate": 30, "object-size": 1000 },
    { "name": "audio", "object-rate": 50, "object-size": 200 }
  ],
  "vcs": 50,
  "vcs-hatch-rate": 10,
  "duration": 60,
  "subscribe-tracks": ["video", "audio"]
}
```

`instances` 配列で複数のインスタンスを起動できます (最上位のキーはテンプレートとして
各インスタンスへ継承されます)。`zakuro-moq lint` / `zakuro-moq fmt` はありません。

## シナリオ

シナリオは仮想クライアントが接続確立後に切断・再接続などを順次実行する機能です。CLI では `--scenario reconnect`、JSONC では `"scenario": "reconnect"` と指定します (現状 `reconnect` のみ)。

reconnect シナリオは「切断してすぐ再接続 → 1-5 秒のランダムスリープ × 9 回」を繰り返します。

## 主なオプション

| オプション | 説明 |
| --- | --- |
| `--config` | JSONC 設定ファイル |
| `--sora-signaling-url` | Sora の WebSocket シグナリング URL。カンマ区切りで複数指定可能 |
| `--sora-channel-id` | Sora のチャネル ID |
| `--sora-role` | `sendonly` / `recvonly` / `sendrecv` |
| `--sora-client-id` | Sora のクライアント ID |
| `--sora-bundle-id` | Sora のバンドル ID |
| `--sora-metadata` | connect メッセージのメタデータ (JSON) |
| `--sora-signaling-notify-metadata` | シグナリング通知メタデータ (JSON) |
| `--sora-ignore-disconnect-websocket` | WebSocket 切断を無視する (`true` / `false`) |
| `--sora-disconnect-wait-timeout` | 切断待ちタイムアウト (秒) |
| `--vcs` | 仮想クライアント数 (`1` - `1000`) |
| `--vcs-hatch-rate` | 仮想クライアントの起動レート |
| `--instance-hatch-rate` | Zakuro インスタンスの起動レート (JSONC `instances` 配列と組み合わせて使用) |
| `--duration` | 接続維持秒数 |
| `--repeat-interval` | duration 経過後の再接続間隔 |
| `--max-retry` | 接続失敗時の最大リトライ回数 |
| `--retry-interval` | リトライ間隔 (秒) |
| `--video-input-device` | 映像入力デバイス名または ID |
| `--input-y4m` | Y4M ファイル入力 |
| `--input-mp4` | MP4 パススルー入力 (映像・音声、ループ再生) |
| `--input-wav` | WAV ファイル音声入力 (PCM 16bit、ループ再生) |
| `--sandstorm` | 砂嵐映像を生成 |
| `--resolution` | `QVGA` / `VGA` / `HD` / `FHD` / `4K` / `WxH` |
| `--framerate` | フレームレート (`1` - `60`) |
| `--no-video-device` | 映像を無効化 |
| `--no-audio-device` | 音声を無効化 |
| `--sora-video-codec-type` | `vp8` / `vp9` / `av1` / `h264` / `h265` |
| `--sora-video-bit-rate` | 映像ビットレート (kbps) |
| `--sora-video-vp9-params` / `--sora-video-av1-params` / `--sora-video-h264-params` / `--sora-video-h265-params` | コーデックパラメータ (JSON) |
| `--vp8-encoder` / `--vp9-encoder` / `--av1-encoder` / `--h264-encoder` / `--h265-encoder` | エンコーダー実装指定 |
| `--show-video-codec-capability` | 利用可能な映像コーデック能力を表示して終了 |
| `--sora-audio` | 音声の有効 / 無効 (`true` / `false`) |
| `--sora-audio-codec-type` | 現状は `opus` |
| `--sora-audio-bit-rate` | 音声ビットレート (kbps) |
| `--openh264` | OpenH264 共有ライブラリのパス |
| `--fdk-aac-lib` | FDK AAC 共有ライブラリのパス (Linux + feature `fdk-aac`) |
| `--sora-data-channels` | DataChannel 設定 JSON |
| `--sora-data-channel-signaling` | DataChannel 経由シグナリング (`true` / `false`) |
| `--sora-simulcast` | サイマルキャスト (`true` / `false`) |
| `--sora-simulcast-request-rid` | サイマルキャストで受信する rid (`r0` / `r1` / `r2`) |
| `--sora-spotlight` | スポットライト (`true` / `false`) |
| `--sora-spotlight-focus-rid` | スポットライトでフォーカス時の rid |
| `--sora-spotlight-unfocus-rid` | スポットライトでアンフォーカス時の rid |
| `--scenario` | 現状は `reconnect` |
| `--http-host`, `--http-port` | HTTP API を有効化 |
| `--client-cert`, `--client-key` | mTLS 設定 (PEM、両方必須) |
| `--insecure` | TLS 証明書検証をスキップ |
| `--log-level` | `verbose` / `info` / `warning` / `error` / `none` (デフォルト: `info`) |
| `--log-suppress` | 抑制するログの部分文字列 (カンマ区切り、メッセージ本体または発生元ファイル名に部分一致) |
| `--duckdb-output-dir` | DuckDB ファイルの出力ディレクトリ |
| `--duckdb-interval` | DuckDB への統計書き込み間隔 (秒、デフォルト: 1.0) |
| `--no-duckdb-output` | DuckDB への統計情報出力を無効化 |
| `--summary-json` | 試験全体の集計結果を書く JSON ファイルのパス |
| `--threshold-success-rate` | 成功接続率の下限 (0.0 から 1.0) |
| `--threshold-connect-time-p95-ms` | 接続確立までの所要時間 p95 の上限 (ミリ秒) |
| `--threshold-stalled` | 停止した接続数の上限 |
| `--threshold-warmup` | 集計から除外する立ち上がり期間 (秒、デフォルト: 0 で除外しない) |

`--log-suppress` は指定した文字列を部分文字列として扱い、ログのメッセージ本体または
発生元ファイル名 (`transport_feedback_adapter.cc` など) に一致した行を出力しません。
既定では何も抑制しません。

```bash
# メッセージで指定する (カンマ区切りで複数指定できる)
zakuro --config zakuro.jsonc --log-suppress "Failed to lookup send time for packet,Packet buffer fully flushed."

# 発生元のファイル名で指定する
zakuro --config zakuro.jsonc --log-suppress transport_feedback_adapter.cc
```

`--log-level` に `verbose` を指定できますが、libwebrtc のログ sink の min severity が
LS_INFO 固定であるため、verbose ログは出力されません。

すべてのオプションは `--help` でも確認できます。

## 試験結果の集計

プロセスが正常終了すると、接続単位の合否を集計した結果をログへ 1 回出力します。
`--summary-json` を指定すると、同じ内容が JSON ファイルに書かれます。

集計する内容は次のとおりです。

- 成功 / 失敗 / 判定不能の接続数
- 成功接続率 (判定不能の接続は分母から外す)
- 失敗理由ごとの接続数
- メディアが止まった接続数
- 接続確立までの所要時間の p50 / p95 / p99
- 立ち上がり期間のため集計から除外した接続数

接続の合否は、確立とメディアの観測から判定します。有効な種別のメディアが流れていない
接続は失敗とし、統計サンプルが取れなかった接続と確立直後に終了した接続は判定不能として
成功接続率の分母から外します。判定に使う観測の詳細は DuckDB の統計出力 (`connection_lifecycle`
テーブル) と同じで、`docs/DUCKDB.md` にまとめています。

```console
$ ./target/release/zakuro --summary-json summary.json ...
```

```json
{
  "success": 98,
  "failure": 2,
  "unjudged": 0,
  "judged": 100,
  "success_rate": 0.980000,
  "stalled": 1,
  "failure_reasons": [
    {"reason": "no-media-sent", "count": 2}
  ],
  "connect_time_ms": {"p50": 120.500, "p95": 300.250, "p99": 500.000}
}
```

### しきい値による合否判定

`--threshold-*` を指定すると、集計結果をしきい値と突き合わせて合否を判定します。
満たさなかった項目はログに警告として出し、終了コード 1 で終わります。しきい値を指定しない
場合は判定せず、終了コード 0 で終わります。

```console
$ ./target/release/zakuro \
    --summary-json summary.json \
    --threshold-success-rate 0.99 \
    --threshold-connect-time-p95-ms 3000 \
    --threshold-stalled 2 \
    --threshold-warmup 10 ...
```

`--threshold-warmup` を指定すると、試験開始から指定した秒数が経過する前に終了した接続を
集計から除外します。`--vcs-hatch-rate` で仮想クライアントを徐々に増やす場合、立ち上がり
期間の接続は負荷が目標に達していないため、合否がぶれます。除外した接続は
`warmup_excluded` に数え、DuckDB の `connection_lifecycle` には記録したままにします。

しきい値を指定したのに判定に必要なデータが無い場合 (接続を 1 本も判定できなかった、
確立できた接続が 1 本も無かったなど) は、満たしたとはみなしません。測定できなかった試験が
CI で成功になるのを避けるためです。

## HTTP API

```bash
# ヘルスチェック
curl -i http://127.0.0.1:8080/.ok

# JSON-RPC (現状の実装メソッドは GetVersion のみ)
curl -s http://127.0.0.1:8080/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","method":"GetVersion","id":1}'
```

レスポンス例:

```json
{"jsonrpc":"2.0","result":{"name":"zakuro","version":"2026.0.0"},"id":1}
```

## 注意点

- `--http-host` と `--http-port` は両方指定が必要です
- `--client-cert` と `--client-key` は両方指定が必要です
- `--sandstorm` は `--input-y4m` / `--video-input-device` / `--input-mp4` と同時指定できません
- `--input-mp4` は `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-wav` と同時指定できません
- `--input-wav` は `--no-audio-device` / `--sora-audio=false` と同時指定できません
- `--input-mp4` はエンコーダー実装指定 (`--vp8-encoder` 等) と同時指定できません
- `--input-mp4` は `--openh264` と同時指定できません
- `--no-video-device` と `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-mp4` は同時指定できません
- `--video-input-device` は `--input-y4m` / `--sandstorm` / `--input-mp4` / `--no-video-device` と同時指定できません
- Sora モードでは `--sora-signaling-url` / `--sora-channel-id` / `--sora-role` が必須です

## リリース

1. `python3 canary.py` を実行し、バージョン bump・コミット・タグ作成・タグ push を行う
2. タグ push を起点に `release` ワークフローが GitHub Release を作成し、プラットフォーム別のアーカイブと SHA256 チェックサムを添付する

## サポートについて

### Discord

- **サポートしません**
- アドバイスします
- フィードバック歓迎します

最新の状況などは Discord で共有しています。質問や相談も Discord でのみ受け付けています。

<https://discord.gg/shiguredo>

### バグ報告

Discord へお願いします。

## ライセンス

Apache License 2.0

```text
Copyright 2026 Shiguredo Inc.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```
