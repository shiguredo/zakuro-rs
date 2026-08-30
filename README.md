# zakuro-rs

Sora WebRTC SFU の負荷試験ツール `zakuro` の Rust 実装です。

仮想クライアントを複数起動し、フェイク映像・実デバイス映像・ Y4M・ MP4 パススルー・ WAV 音声を使って Sora へ接続できます。HTTP ヘルスチェック・ JSON-RPC・ DuckDB 統計出力も提供します。

より詳細な C++ 版との対応表・実装状況は `docs/ZAKURO.md`、DuckDB のスキーマは `docs/DUCKDB.md` を参照してください。

## 主な機能

- 1 プロセスで複数の Zakuro インスタンスを段階的に起動 (JSONC `instances` 配列、`--instance-hatch-rate`)
- 複数の仮想クライアントを段階的に起動 (`--vcs` / `--vcs-hatch-rate`)
- Sora への `sendonly` / `recvonly` / `sendrecv` 接続
- フェイク映像 (Raden デジタル時計)、砂嵐、Y4M 入力、実カメラ入力、MP4 パススルー送信
- フェイク音声 (BIP / BOP / HUM / ノイズの連続自動生成。旧映像同期ビープは廃止)、WAV ファイル入力 (`--input-wav`)、MP4 内の音声トラック送信 (Opus / AAC、`--input-mp4` 時)
- 映像 / 音声コーデック指定、OpenH264 エンコード (`--openh264`)、コーデックパラメータ / エンコーダー実装指定
- `--show-video-codec-capability` による映像コーデック能力表示
- 受信映像をデコードせず廃棄する NopVideoDecoder
- DataChannel メッセージング (`--sora-data-channels`、ZAKURO ヘッダ付き自動送信)
- サイマルキャスト / スポットライト
- 再接続シナリオ (`--scenario reconnect`)
- mTLS (`--client-cert` / `--client-key`) と TLS 検証スキップ (`--insecure`)
- DuckDB ファイルへの統計情報出力
- JSONC 設定ファイル (`--config`)
- 設定ファイルの検証・整形サブコマンド (`zakuro lint` / `zakuro fmt`)
- HTTP API (`GET /.ok`, `POST /rpc` / `GetVersion`)
- ログレベル制御 (`--log-level`)

## 必要環境

- Rust stable
- `rustfmt`, `clippy`

このリポジトリは `rust-toolchain.toml` で stable toolchain を使用します。

## ビルド

```bash
cargo build
```

DuckDB は `bundled` せず、`.cargo/config.toml` の `DUCKDB_DOWNLOAD_LIB=1` により prebuilt バイナリをダウンロードしてリンクする。取得物は `target/duckdb-download/` にキャッシュされる。

確認用コマンド:

```bash
make check
make test
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

### Y4M ファイルを使って送信する

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-y4m \
  --sora-role sendonly \
  --input-y4m ./video.y4m
```

### WAV ファイルから音声を流す

WAV 未指定時は Safari 相当の BIP / BOP / HUM / ノイズ連続 PCM (48kHz モノラル・2 秒ループ) が自動生成されます。旧来の映像同期ビープはありません。`--input-wav` を指定すると自動生成の代わりに WAV をループ再生します。

`--input-wav` は PCM 16bit のモノラル / ステレオに対応します。サンプルレートは自動で 48kHz にリサンプリングされ、ファイル終端に達するとループ再生します。

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-wav \
  --sora-role sendonly \
  --input-wav ./audio.wav
```

### MP4 パススルーで送信する

`--input-mp4` はエンコード済み映像を再エンコードせずにパススルー送信します。ファイル終端に達するとループ再生します。使用時は `--sora-video-codec-type` と `--sora-video-bit-rate` が必須です。H.264 / AV1 では、CLI で未指定のコーデックパラメータ (`profile_level_id` や `profile` / `level_idx` / `tier`) を MP4 の実値から connect へ自動で載せます（Sora の offer と bitstream を揃えるため。明示指定があればそれを優先します）。自動載せには Sora 側で `signaling_h264_params` / `signaling_av1_params` が有効である必要があります。

MP4 内の音声トラックがある場合はデコードして送信します (Opus / AAC、モノラル / ステレオ)。Opus は 20ms パケット、AAC は 1 サンプル = 1 フレームの mp4a を前提とします (通常のエンコード済み MP4 はこの構成です)。音声トラックが無い・未対応コーデックの MP4 は従来どおり映像のみになります。音声送信を有効にしている場合 (デフォルト)、音声トラックが 2 本以上ある MP4 は起動時エラーになります。

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-mp4 \
  --sora-role sendonly \
  --input-mp4 ./video.mp4 \
  --sora-video-codec-type h264 \
  --sora-video-bit-rate 2000
```

AAC 音声は Linux で feature `fdk-aac` を有効にしたビルド (`cargo build --features fdk-aac`) でのみ利用できます。libfdk-aac 共有ライブラリの動的ロードが必要で、`--fdk-aac-lib` でパスを指定します (Opus 音声のみの MP4 では指定は不要です)。Linux 上で libfdk-aac をロードできない環境 (ライブラリ未導入・`--fdk-aac-lib` 未指定) では、音声送信を有効にしている場合 (デフォルト) に AAC 音声を含む MP4 を使用すると起動時エラーになります。feature 無し / 非 Linux では AAC は未対応として映像のみで続行します。`--no-audio-device` / `--sora-audio=false` では MP4 音声も送信しません。また AAC のサンプルは 1 サンプル = 1 フレームの mp4a を前提としています (通常のエンコード済み MP4 はこの構成です)。

```bash
cargo run --features fdk-aac -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-mp4-aac \
  --sora-role sendonly \
  --fdk-aac-lib /usr/lib/x86_64-linux-gnu/libfdk-aac.so.2 \
  --input-mp4 ./video-with-aac.mp4 \
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

### 再接続シナリオとログレベル

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-reconnect \
  --sora-role sendonly \
  --scenario reconnect \
  --log-level warning
```

## JSONC 設定ファイル

`--config` で JSONC 設定ファイルを読み込めます。CLI 引数は設定ファイルより優先されます。

```jsonc
{
  // Sora 接続先
  "sora-signaling-url": "wss://sora.example.com/signaling",
  "sora-channel-id": "zakuro-config",
  "sora-role": "sendonly",

  // 仮想クライアント
  "vcs": 10,
  "vcs-hatch-rate": 5,

  // 映像
  "resolution": "HD",
  "framerate": 30,
  "sandstorm": true,

  // HTTP API
  "http-host": "127.0.0.1",
  "http-port": 8080
}
```

実行例:

```bash
cargo run -- --config ./config.jsonc
```

### 複数の Zakuro インスタンスを起動する

1 プロセスで複数の Zakuro インスタンスを起動するには JSONC `instances` 配列を使います。各要素が独立した `SoraConnectionContext` と仮想クライアント群を持ち、i 番目のインスタンスは `i / instance-hatch-rate` 秒の遅延後に起動します。最上位のキーはインスタンス共通設定 (HTTP サーバー、`--instance-hatch-rate`、mTLS、`--openh264`、`--fdk-aac-lib`、`--log-level`、DuckDB) と全インスタンス向けテンプレート (`instances[i]` で上書き可) を兼ねます。

```jsonc
{
  // 全インスタンス共通設定
  "instance-hatch-rate": 1.0,
  "http-host": "127.0.0.1",
  "http-port": 8080,

  // インスタンスごとの設定
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

`instances` が無い JSONC や CLI 単独起動は従来通り単一インスタンスとして動作します。

### 設定ファイルを検証・整形する (lint / fmt)

`zakuro lint <FILE.jsonc>` は負荷試験を起動せずに設定ファイルを検証します。通常起動と同じ規則で構文パースと意味検証 (必須キーの欠落、未知のキー、排他指定など) を行い、Sora へは接続しません。OpenH264 / FDK AAC 共有ライブラリや PEM などのファイル実体の読み込み検証は行いません。成功時は無出力で exit 0、失敗時はソース注釈付きの診断を stderr に出して exit 1 になります。検証のみで、ファイルの書き換えや `--fix` はありません。

`zakuro fmt <FILE.jsonc>` は設定ファイルをその場で整形します。2 スペースインデントに正規化し、`//` / `/* */` コメント・空行・trailing comma は保持します。変更が無い場合はファイルを書き換えません。構文エラー時は非 0 で、ファイルは書き換えられません。`--check` を付けると書き戻さず、未整形なら exit 1 になります (CI 向け)。

```bash
# 検証 (負荷試験は起動しない)
cargo run -- lint ./config.jsonc

# 整形 (ファイルを書き換え)
cargo run -- fmt ./config.jsonc

# 整形済みか確認するだけ (書き換えない)
cargo run -- fmt --check ./config.jsonc
```

## シナリオ

シナリオは仮想クライアント 1 つ 1 つが接続確立後に切断・再接続などの動作を順次実行する機能です。

### 指定の書き方

CLI では `--scenario reconnect`、JSONC では `"scenario": "reconnect"` と書きます。インスタンスごとの設定なので、最上位テンプレート (全インスタンス共通) と `instances[i]` (インスタンスごとに上書き) の両方で指定できます。現状種別は `reconnect` のみで、未指定ならシナリオは動きません (接続したまま維持される)。

reconnect シナリオは接続確立後「切断してすぐ再接続 → 1-5 秒のランダムスリープ × 9 回 (合計 9-45 秒)」を繰り返します。2 接続目以降の持続時間は各 9-45 秒です。実行例は「実行例」節の再接続シナリオを参照してください。

```jsonc
{
  "sora": {
    "signaling-url": "wss://sora.example.com/signaling",
    "channel-id": "zakuro-reconnect",
    "role": "sendonly"
  },
  "vcs": 10,
  "scenario": "reconnect"
}
```

### 新しいシナリオを追加するには (ソース改変)

- `src/scenario.rs` の `ScenarioType` に種別を追加し (`ScenarioType::parse` の文字列も含む)、`build_scenario` でシナリオ定義を返す
- シナリオ定義は `ScenarioOp` の列とループ開始位置 `loop_index` で構成する。利用可能な操作は `Sleep` (ランダムスリープ)、`Reconnect` / `Disconnect` (切断 → 再接続)、`Exit` (切断して仮想クライアントタスクを終了)、`SendDataChannelMessage` (指定ラベルで 1 回送信)
- 再接続をまたいでも実行位置は続きから再開される (1 接続で全操作をやり直さない)
- C++ 版との対応表や未実装方針 (数字音声再生など) は `docs/ZAKURO.md` のシナリオ節を参照

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
| `--fdk-aac-lib` | FDK AAC 共有ライブラリのパス (feature `fdk-aac` の Linux ビルドで AAC 音声をデコードするときに使用) |
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
| `--duckdb-output-dir` | DuckDB ファイルの出力ディレクトリ (デフォルト: カレントディレクトリ) |
| `--duckdb-interval` | DuckDB への統計書き込み間隔 (秒、デフォルト: 1.0) |
| `--no-duckdb-output` | DuckDB への統計情報出力を無効化 |

すべてのオプションは `--help` でも確認できます。

## HTTP API

### ヘルスチェック

```bash
curl -i http://127.0.0.1:8080/.ok
```

### JSON-RPC

現状の実装メソッドは `GetVersion` のみです。

```bash
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
- `--input-mp4` は `--vp8-encoder` / `--vp9-encoder` / `--av1-encoder` / `--h264-encoder` / `--h265-encoder` (エンコーダー実装指定) と同時指定できません
- エンコーダー実装指定との排他エラーは、実際に指定したキー名ではなく `--vp8-encoder 等` の汎用表記になります
- `--input-mp4` は `--openh264` と同時指定できません
- `--no-video-device` と `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-mp4` は同時指定できません
- `--video-input-device` は `--input-y4m` / `--sandstorm` / `--input-mp4` / `--no-video-device` と同時指定できません
- `--openh264` を使う場合は共有ライブラリのパスを指定します (H.264 エンコード用)
