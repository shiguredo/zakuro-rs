# Recording Composition Tool Zakuro

[![ci](https://github.com/shiguredo/zakuro-rs/actions/workflows/ci.yml/badge.svg?branch=develop)](https://github.com/shiguredo/zakuro-rs/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

## About Shiguredo's open source software

We will not respond to PRs or issues that have not been discussed on Discord. Also, Discord is only available in Japanese.

Please read <https://github.com/shiguredo/oss/blob/master/README.en.md> before use.

## 時雨堂のオープンソースソフトウェアについて

利用前に <https://github.com/shiguredo/oss> をお読みください。

## Recording Composition Tool Zakuro について

Sora WebRTC SFU の負荷試験ツール `zakuro` の Rust 実装です。仮想クライアントを複数起動し、フェイク映像・実デバイス映像・ Y4M・ MP4 パススルー・ WAV 音声を使って Sora へ接続できます。HTTP ヘルスチェック・ JSON-RPC・ DuckDB 統計出力も提供します。

C++ 版との対応表・実装状況は `docs/ZAKURO.md`、DuckDB のスキーマは `docs/DUCKDB.md` を参照してください。

## 主な機能

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

- Rust toolchain (`Cargo.toml` の `rust-version` は 1.98)
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
| ubuntu-24.04 (x86_64) | 確認済み | 212 件成功 | CI |
| ubuntu-26.04 (x86_64) | 確認済み | 212 件成功 | CI |
| macOS (Apple Silicon) | 確認済み | 210 件成功 | 開発マシン |
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
| `--duckdb-output-dir` | DuckDB ファイルの出力ディレクトリ |
| `--duckdb-interval` | DuckDB への統計書き込み間隔 (秒、デフォルト: 1.0) |
| `--no-duckdb-output` | DuckDB への統計情報出力を無効化 |

すべてのオプションは `--help` でも確認できます。

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
