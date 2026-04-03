# zakuro (C++ 実装) 調査結果と zakuro-rs との差分

## 概要

zakuro は Sora WebRTC SFU の負荷試験ツール。
仮想クライアントを大量に生成し、フェイク映像・音声を送受信して SFU の性能を検証する。

zakuro-rs は zakuro の Rust 再実装であり、互換性を維持しつつ最高性能を目指す。

## zakuro (C++) アーキテクチャ

### ビルドシステム

- CMake 4.2.1 以上
- C++17
- Python ビルドスクリプト (`buildbase.py`)

### 主要な依存ライブラリ

| ライブラリ | バージョン | 用途 |
|-----------|-----------|------|
| libwebrtc | m144.7559.2.1 | WebRTC 通信 |
| Sora C++ SDK | 2025.7.0-canary.3 | SFU 連携 |
| Boost | 1.89.0 | JSON, filesystem |
| CLI11 | 2.6.1 | コマンドライン引数 |
| Blend2D | 0.20.0 | グラフィックス描画 |
| OpenH264 | - | H.264 コーデック (オプション) |

### コンポーネント構成

| コンポーネント | 役割 |
|--------------|------|
| Zakuro | メインコントローラー、設定管理、実行制御 |
| VirtualClient | WebRTC/Sora 接続クライアント |
| FakeVideoCapturer | フェイク映像生成 (Safari UI、砂嵐、Y4M) |
| ZakuroAudioDeviceModule | 音声デバイス抽象化層 |
| ScenarioPlayer | シナリオベースの動作シミュレーション |
| HttpServer | HTTP API サーバー (ヘルスチェック、JSON-RPC) |
| GameKeyCore | キーボード入力監視 |
| GameAudioManager | ゲーム音声管理 |
| Y4MReader | Y4M 動画ファイル読込 |
| WavReader | WAV 音声ファイル読込 |
| NopVideoDecoder | 受信映像廃棄用デコーダ (CPU 効率化) |
| EmbeddedBinary | リソースファイルのメモリ埋め込み |
| JsonRpcHandler | JSON-RPC 2.0 API 処理 |

### 処理フロー

```
main()
  → ファイルディスクリプタ制限チェック (最小 1024)
  → 設定解析 (CLI 引数 or JSONC 設定ファイル)
  → GameKeyCore 初期化 (キーボード監視スレッド)
  → HttpServer 起動 (オプション)
  → stats 集計スレッド起動 (オプション)
  → 各 Zakuro インスタンス用スレッド生成 (instance-hatch-rate に従い段階的起動)
  → スレッド群の終了待機

Zakuro::Run()
  → ビデオキャプチャ初期化 (Fake or Camera)
  → AudioDeviceModule 設定
  → SoraClientContext 作成
  → VirtualClient 生成 (vcs 個分)
  → ScenarioPlayer 作成
  → メイン io_context.run()
  → 全クライアント切断・リソース解放
```

### 映像処理

3 つの動画モード:

- **Safari** (デフォルト): Blend2D で UI スタイルの描画。時刻、フレーム番号、統計情報、カラーパターン
- **Sandstorm**: ランダムピクセルによる砂嵐。符号化負荷が高い
- **Y4MFile**: YUV4MPEG2 形式ファイルからの読込

フレーム生成:
1. Blend2D で BGRA32 イメージ描画
2. libyuv で ABGR → I420 変換
3. WebRTC フレーム作成

### 音声処理

5 つの音声モード:

- **NoAudio**: ダミー ADM で無音出力
- **Device**: システムマイク使用
- **AutoGenerateFakeAudio**: BIP/BOP/HUM/ノイズ自動生成 (48kHz, モノラル)
- **SpecifiedFakeAudio**: WAV ファイル指定
- **External**: GameAudioManager 経由のゲーム音声

処理単位は 10ms フレーム。

### DataChannel メッセージ構造

```
Bytes 0-5:   "ZAKURO" (シグネチャ)
Bytes 6-13:  現在時刻 (マイクロ秒 UNIX Time)
Bytes 14-21: ラベルごとのカウンター
Bytes 22-47: Connection ID (最大 26 文字)
+ ペイロード: ランダムバイナリ (48-256KB)
```

### シナリオ操作

- OpSleep: 待機
- OpPlayVoiceNumberClient: 数字音声再生
- OpSendDataChannelMessage: メッセージ送信
- OpDisconnect: 切断
- OpReconnect: 再接続
- OpExit: 終了

### HTTP API

```
GET  /.ok          → 200 OK (ヘルスチェック)
POST /rpc          → JSON-RPC 2.0

メソッド:
  - GetVersion: バージョン情報取得
```

### コマンドライン引数 (主要なもの)

```
# 接続設定
--sora-signaling-url <URL>          シグナリング URL (複数指定可)
--sora-channel-id <ID>              チャネル ID
--sora-role {sendonly,recvonly,sendrecv}

# 仮想クライアント
--vcs <N>                           仮想クライアント数 (1-1000)
--vcs-hatch-rate <F>                1 秒間に起動する VC 数
--instance-hatch-rate <F>           インスタンス生成レート

# 映像
--resolution {QVGA,VGA,HD,FHD,4K,WxH}
--framerate <N>                     フレームレート (1-60)
--fake-capture-device               フェイク映像使用
--sandstorm                         砂嵐パターン
--fake-video-capture <FILE>         Y4M 動画ファイル
--video-device <NAME>               実デバイス指定
--fixed-resolution                  解像度固定

# 音声
--fake-audio-capture <FILE>         WAV 音声ファイル
--no-audio-device                   音声無効化

# コーデック
--sora-video-codec-type {VP8,VP9,AV1,H264,H265}
--sora-audio-codec-type {OPUS}
--sora-video-bit-rate <kbps>
--sora-audio-bit-rate <kbps>
--openh264 <PATH>

# 制御
--duration <SEC>                    実行時間
--repeat-interval <SEC>             再接続間隔
--max-retry <N>                     最大リトライ
--retry-interval <SEC>              リトライ間隔

# 高度な設定
--degradation-preference {disabled,maintain_framerate,maintain_resolution,balanced}
--sora-simulcast                    シミュルキャスト
--sora-spotlight                    スポットライト
--sora-data-channels <JSON>         DataChannel 設定
--scenario {reconnect}              シナリオ選択

# HTTP API
--http-host <ADDR>
--http-port <PORT>

# その他
--config <FILE>                     JSONC 設定ファイル
--log-level {verbose,info,warning,error,none}
--client-cert <FILE>                mTLS 証明書
--client-key <FILE>                 mTLS 秘密鍵
```

## zakuro-rs 実装 TODO

### コア機能

- [x] コマンドライン引数パース (noargs)
- [x] Sora SDK 連携 (映像・音声送受信)
- [x] 複数仮想クライアント管理
- [x] hatch-rate による段階的起動
- [x] 統計収集・レポート
- [x] リトライロジック (max-retry, retry-interval)
- [x] Duration + repeat-interval
- [x] Ctrl+C グレースフルシャットダウン

### 映像

- [x] フェイク映像生成 (Raden: デジタル時計、パイチャート、カラーボックス)
- [x] 砂嵐映像生成
- [x] 解像度指定 (QVGA/VGA/HD/FHD/4K/WxH)
- [x] フレームレート指定 (1-60)
- [x] Y4M 動画ファイル読込 (`--fake-video-capture`)
- [x] 実デバイスキャプチャ (`--video-input-device`)
- [ ] 解像度固定モード (`--fixed-resolution`)

### 音声

- [x] 音声無効化 (`--no-audio-device`)
- [ ] フェイク音声自動生成 (BIP/BOP/HUM/ノイズ, 48kHz)
- [ ] WAV 音声ファイル読込 (`--fake-audio-capture`)

### コーデック

- [x] ビデオコーデック指定 (VP8/VP9/AV1/H264/H265)
- [x] オーディオコーデック指定 (Opus)
- [x] ビットレート指定 (映像・音声)
- [x] OpenH264 外部ライブラリ (`--openh264`)

### 接続設定

- [x] シグナリング URL (複数指定可)
- [x] チャネル ID
- [x] クライアント ID (`--sora-client-id`)
- [x] バンドル ID (`--sora-bundle-id`)
- [x] ロール (sendonly/recvonly/sendrecv)
- [x] メタデータ (`--sora-metadata`)
- [x] シグナリング通知メタデータ (`--sora-signaling-notify-metadata`)
- [x] DataChannel シグナリング
- [x] 切断待ちタイムアウト (`--sora-disconnect-wait-timeout`)
- [x] mTLS (`--client-cert`, `--client-key`)
- [x] TLS 証明書検証スキップ (`--insecure`)
- [x] サイマルキャスト (`--sora-simulcast`, `--sora-simulcast-request-rid`)
- [x] スポットライト (`--sora-spotlight`, `--sora-spotlight-focus-rid`, `--sora-spotlight-unfocus-rid`)
- [ ] degradation-preference
- [x] DataChannel メッセージング (`--sora-data-channels`)

### HTTP API

- [x] ヘルスチェック (`GET /.ok`)
- [x] JSON-RPC 2.0 (`POST /rpc`)
- [x] GetVersion メソッド

### シナリオ

- [ ] ScenarioPlayer (Sleep, Disconnect, Reconnect, Exit)
- [x] DataChannel メッセージ自動送信 (ZAKURO ヘッダ付き)
- [x] instance-hatch-rate (`--vcs-hatch-rate`)

### その他

- [x] JSONC 設定ファイル (`--config`)
- [ ] ログレベル制御 (`--log-level`)
- [x] NopVideoDecoder (受信映像廃棄)
- [ ] 埋め込みリソース (フォント・音声)
### sora-rust-sdk 未対応のため未実装の機能

- スポットライト数指定 (`--sora-spotlight-number`)
- シグナリング URL ランダム化無効 (`--sora-disable-signaling-url-randomization`)
- DataChannel シグナリングタイムアウト (`--sora-data-channel-signaling-timeout`)
- コーデック個別エンコーダ指定 (`--vp8-encoder` 等)
- コーデックパラメータ (`--sora-video-vp9-params` 等、Params 構造体のフィールドが private)
- ビデオコーデック能力表示 (`--show-video-codec-capability`)
- connection ID ファイル出力 (`--output-file-connection-id`)

### 実装しない機能

- GameKeyCore (キーボード入力制御)
- GameAudioManager (ゲーム音声)

### 設計差分

| 項目 | zakuro (C++) | zakuro-rs (Rust) |
|------|-------------|-----------------|
| 非同期 | boost::asio io_context | tokio マルチスレッドランタイム |
| 描画 | Blend2D | Raden |
| 引数パース | CLI11 | noargs |
| JSON | Boost.JSON | nojson |
| シグナル処理 | SIGINT/SIGTERM | tokio::signal (Ctrl+C) |
| 統計通知 | コールバック | mpsc + watch チャネル |
| シャットダウン | io_context 停止 | CancellationToken |
