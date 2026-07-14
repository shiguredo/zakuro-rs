# zakuro-rs

Sora WebRTC SFU の負荷試験ツール `zakuro` の Rust 実装です。

仮想クライアントを複数起動し、フェイク映像・実デバイス映像・ Y4M・ MP4 パススルー・ WAV 音声を使って Sora へ接続できます。HTTP ヘルスチェック・ JSON-RPC・ DuckDB 統計出力も提供します。

より詳細な C++ 版との対応表・実装状況は `docs/ZAKURO.md`、DuckDB のスキーマは `docs/DUCKDB.md` を参照してください。

## 主な機能

- 1 プロセスで複数の Zakuro インスタンスを段階的に起動 (JSONC `instances` 配列、`--instance-hatch-rate`)
- 複数の仮想クライアントを段階的に起動 (`--vcs` / `--vcs-hatch-rate`)
- Sora への `sendonly` / `recvonly` / `sendrecv` 接続
- フェイク映像 (Raden デジタル時計)、砂嵐、Y4M 入力、実カメラ入力、MP4 パススルー送信
- フェイク音声 (映像同期ビープ)、WAV ファイル入力 (`--input-wav`)
- 映像 / 音声コーデック指定、OpenH264 エンコード (`--openh264`)
- 受信映像をデコードせず廃棄する NopVideoDecoder
- DataChannel メッセージング (`--sora-data-channels`、ZAKURO ヘッダ付き自動送信)
- サイマルキャスト / スポットライト
- 再接続シナリオ (`--scenario reconnect`)
- mTLS (`--client-cert` / `--client-key`) と TLS 検証スキップ (`--insecure`)
- DuckDB ファイルへの統計情報出力
- JSONC 設定ファイル (`--config`)
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

`--input-wav` は PCM 16bit のモノラル / ステレオに対応します。サンプルレートは自動で 48kHz にリサンプリングされ、ファイル終端に達するとループ再生します。

```bash
cargo run -- \
  --sora-signaling-url wss://sora.example.com/signaling \
  --sora-channel-id zakuro-wav \
  --sora-role sendonly \
  --input-wav ./audio.wav
```

### MP4 パススルーで送信する

`--input-mp4` 使用時は `--sora-video-codec-type` と `--sora-video-bit-rate` が必須です。

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

1 プロセスで複数の Zakuro インスタンスを起動するには JSONC `instances` 配列を使います。各要素が独立した `SoraConnectionContext` と仮想クライアント群を持ち、i 番目のインスタンスは `i / instance-hatch-rate` 秒の遅延後に起動します。最上位のキーはインスタンス共通設定 (HTTP サーバー、`--instance-hatch-rate`、mTLS、`--openh264`、`--log-level`、DuckDB) と全インスタンス向けテンプレート (`instances[i]` で上書き可) を兼ねます。

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
| `--vcs` | 仮想クライアント数 (`1` - `1000`) |
| `--vcs-hatch-rate` | 仮想クライアントの起動レート |
| `--instance-hatch-rate` | Zakuro インスタンスの起動レート (JSONC `instances` 配列と組み合わせて使用) |
| `--duration` | 接続維持秒数 |
| `--repeat-interval` | duration 経過後の再接続間隔 |
| `--max-retry` | 接続失敗時の最大リトライ回数 |
| `--retry-interval` | リトライ間隔 (秒) |
| `--video-input-device` | 映像入力デバイス名または ID |
| `--input-y4m` | Y4M ファイル入力 |
| `--input-mp4` | MP4 パススルー入力 |
| `--input-wav` | WAV ファイル音声入力 (PCM 16bit、ループ再生) |
| `--sandstorm` | 砂嵐映像を生成 |
| `--resolution` | `QVGA` / `VGA` / `HD` / `FHD` / `4K` / `WxH` |
| `--framerate` | フレームレート (`1` - `60`) |
| `--no-video-device` | 映像を無効化 |
| `--no-audio-device` | 音声を無効化 |
| `--sora-video-codec-type` | `vp8` / `vp9` / `av1` / `h264` / `h265` |
| `--sora-video-bit-rate` | 映像ビットレート (kbps) |
| `--sora-audio` | 音声の有効 / 無効 (`true` / `false`) |
| `--sora-audio-codec-type` | 現状は `opus` |
| `--sora-audio-bit-rate` | 音声ビットレート (kbps) |
| `--openh264` | OpenH264 共有ライブラリのパス |
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
{"jsonrpc":"2.0","result":{"name":"zakuro","version":"2026.1.0"},"id":1}
```

## 注意点

- `--http-host` と `--http-port` は両方指定が必要です
- `--client-cert` と `--client-key` は両方指定が必要です
- `--sandstorm` は `--input-y4m` / `--video-input-device` / `--input-mp4` と同時指定できません
- `--input-mp4` は `--video-input-device` / `--input-y4m` / `--sandstorm` と同時指定できません
- `--input-wav` は `--no-audio-device` / `--sora-audio=false` と同時指定できません
- `--openh264` を使う場合は共有ライブラリのパスを指定します (H.264 エンコード用)
