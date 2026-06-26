# zakuro-rs

Sora WebRTC SFU の負荷試験ツール `zakuro` の Rust 実装です。

仮想クライアントを複数起動し、フェイク映像・実デバイス映像・ Y4M・ MP4 パススルーを使って Sora へ接続できます。HTTP ヘルスチェックと JSON-RPC も提供します。

## 主な機能

- 1 プロセスで複数の Zakuro インスタンスを段階的に起動 (JSONC `instances` 配列、`--instance-hatch-rate`)
- 複数の仮想クライアントを段階的に起動
- Sora への `sendonly` / `recvonly` / `sendrecv` 接続
- フェイク映像、砂嵐映像、Y4M 入力、実カメラ入力、MP4 パススルー送信
- 音声の有効 / 無効切り替え
- DataChannel 設定
- JSONC 設定ファイルの読み込み
- HTTP API (`GET /.ok`, `POST /rpc`)
- 再接続シナリオ (`--scenario reconnect`)

## 必要環境

- Rust stable
- `rustfmt`, `clippy`

このリポジトリは `rust-toolchain.toml` で stable toolchain を使用します。

## ビルド

```bash
cargo build
```

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
  --fake-video-capture ./video.y4m
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

1 プロセスで複数の Zakuro インスタンスを起動するには JSONC `instances` 配列を使います。各要素が独立した `SoraConnectionContext` と仮想クライアント群を持ち、i 番目のインスタンスは `i / instance-hatch-rate` 秒の遅延後に起動します。最上位のキーはインスタンス共通設定 (HTTP サーバー、`--instance-hatch-rate`、mTLS、`--openh264`) と全インスタンス向けテンプレート (`instances[i]` で上書き可) を兼ねます。

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
| `--sora-signaling-url` | Sora の WebSocket シグナリング URL。カンマ区切りで複数指定可能 |
| `--sora-channel-id` | Sora のチャネル ID |
| `--sora-role` | `sendonly` / `recvonly` / `sendrecv` |
| `--vcs` | 仮想クライアント数 (`1` - `1000`) |
| `--vcs-hatch-rate` | 仮想クライアントの起動レート |
| `--instance-hatch-rate` | Zakuro インスタンスの起動レート (JSONC `instances` 配列と組み合わせて使用) |
| `--duration` | 接続維持秒数 |
| `--repeat-interval` | 再接続間隔 |
| `--video-input-device` | 映像入力デバイス名または ID |
| `--fake-video-capture` | Y4M ファイル入力 |
| `--input-mp4` | MP4 パススルー入力 |
| `--sandstorm` | 砂嵐映像を生成 |
| `--resolution` | `QVGA` / `VGA` / `HD` / `FHD` / `4K` / `WxH` |
| `--framerate` | フレームレート (`1` - `60`) |
| `--no-video-device` | 映像を無効化 |
| `--no-audio-device` | 音声を無効化 |
| `--sora-video-codec-type` | `vp8` / `vp9` / `av1` / `h264` / `h265` |
| `--sora-video-bit-rate` | 映像ビットレート (kbps) |
| `--sora-audio` | 音声の有効 / 無効 (`true` / `false`) |
| `--sora-audio-codec-type` | 現状は `opus` |
| `--sora-data-channels` | DataChannel 設定 JSON |
| `--scenario` | 現状は `reconnect` |
| `--http-host`, `--http-port` | HTTP API を有効化 |
| `--client-cert`, `--client-key` | mTLS 設定 |
| `--insecure` | TLS 証明書検証をスキップ |

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
- `--sandstorm` は `--fake-video-capture` / `--video-input-device` / `--input-mp4` と同時指定できません
- `--input-mp4` は `--video-input-device` / `--fake-video-capture` / `--sandstorm` と同時指定できません

## 関連資料

- `docs/ZAKURO.md`
