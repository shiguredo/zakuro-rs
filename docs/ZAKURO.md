# zakuro (C++ 実装) 調査結果と zakuro-rs との差分

## 概要

zakuro は Sora WebRTC SFU の負荷試験ツール。
仮想クライアントを大量に生成し、フェイク映像・音声を送受信して SFU の性能を検証する。

zakuro-rs は zakuro の Rust 再実装であり、互換性を維持しつつ最高性能を目指す。

## zakuro (C++) アーキテクチャ

### ビルドシステム

- CMake 3.23 以上 (DEPS で取得する CMake は 4.3.2)
- C++20
- Python ビルドスクリプト (`buildbase.py`)

### 主要な依存ライブラリ

| ライブラリ | バージョン | 用途 |
|-----------|-----------|------|
| libwebrtc | m150.7871.0.0 | WebRTC 通信 |
| Sora C++ SDK | 2026.2.0-canary.14 | SFU 連携 |
| Boost | 1.91.0 | JSON, filesystem |
| CLI11 | 2.6.2 | コマンドライン引数 |
| Blend2D | 0.21.2 | グラフィックス描画 |
| OpenH264 | v2.6.0 | H.264 コーデック (オプション) |

### コンポーネント構成

| コンポーネント | 役割 |
|--------------|------|
| Zakuro | メインコントローラー、設定管理、実行制御 |
| VirtualClient | WebRTC / Sora 接続クライアント |
| FakeVideoCapturer | フェイク映像生成 (Safari UI、砂嵐、Y4M) |
| ZakuroAudioDeviceModule | 音声デバイス抽象化層 |
| ScenarioPlayer | シナリオベースの動作シミュレーション |
| HttpServer | HTTP API サーバー (ヘルスチェック、JSON-RPC) |
| HttpProxy | UI リバースプロキシ (`--ui` / `--ui-remote-url`) |
| GameKeyCore | キーボード入力監視 |
| GameAudioManager | ゲーム音声管理 |
| Y4MReader | Y4M 動画ファイル読込 |
| WavReader | WAV 音声ファイル読込 |
| BinaryPool | DataChannel 送信用ランダムバイナリのプール |
| NopVideoDecoder | 受信映像廃棄用デコーダー (CPU 効率化) |
| EmbeddedBinary | リソースファイルのメモリ埋め込み |
| JsonRpcHandler | JSON-RPC 2.0 API 処理 |
| ZakuroStats | 接続統計の集計 (`--output-file-connection-id` 向け) |

### 処理フロー

```
main()
  → ファイルディスクリプタ制限チェック (最小 1024)
  → 設定解析 (CLI 引数 or JSONC 設定ファイル)
  → GameKeyCore 初期化 (キーボード監視スレッド)
  → HttpServer 起動 (オプション、UI プロキシ併用可)
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
- **AutoGenerateFakeAudio**: BIP / BOP / HUM / ノイズ自動生成 (48kHz, モノラル)
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
- OpSendDataChannelMessage: メッセージ送信
- OpDisconnect: 切断
- OpReconnect: 再接続
- OpExit: 終了
- OpPlaySubScenario: サブシナリオ再生 (実装しない)
- OpPlayVoiceNumberClient: 数字音声再生 (実装しない)

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
--instance-hatch-rate <F>           インスタンス生成レート (JSONC `instances` 配列との併用)

# 映像
--resolution {QVGA,VGA,HD,FHD,4K,WxH}
--framerate <N>                     フレームレート (1-60)
--fake-capture-device               フェイク映像使用
--sandstorm                         砂嵐パターン
--fake-video-capture <FILE>         Y4M 動画ファイル
--video-device <NAME>               実デバイス指定
--no-video-device                   映像無効化
--fixed-resolution                  解像度固定
--priority {BALANCE,FRAMERATE,RESOLUTION}
--degradation-preference {disabled,maintain_framerate,maintain_resolution,balanced}

# 音声
--fake-audio-capture <FILE>         WAV 音声ファイル
--no-audio-device                   音声無効化
--initial-mute-video <BOOL>
--initial-mute-audio <BOOL>

# コーデック
--sora-video-codec-type {vp8,vp9,av1,h264,h265}
--sora-audio-codec-type {opus}
--sora-video-bit-rate <kbps>
--sora-audio-bit-rate <kbps>
--openh264 <PATH>
--vp8-encoder / --vp9-encoder / --av1-encoder / --h264-encoder / --h265-encoder
--sora-video-vp9-params / --sora-video-av1-params / --sora-video-h264-params / --sora-video-h265-params

コーデックパラメータは JSON 文字列で指定する (例: `--sora-video-vp9-params '{"profile_id": 0}'`)。JSONC 設定ファイルのオブジェクト値 (`"video-vp9-params": {...}`) にコメントや末尾カンマを書くとパースエラーになる。サポートするキーと範囲:

- `--sora-video-vp9-params`: `profile_id` (0-3)
- `--sora-video-av1-params`: `profile` (0-2) / `level_idx` (0-31) / `tier` (0-1)
- `--sora-video-h264-params`: `profile_level_id` (文字列) / `b_frame` (true/false)
- `--sora-video-h265-params`: `profile_id` (0-31) / `tier_flag` (0-1) / `tx_mode` (SRST/MRST/MRMT) / `b_frame` (true/false)。`level_id` は Sora サーバーの検証と一致しないため未対応

コーデックパラメータの送信は Sora サーバー側の sora.conf 設定 (`signaling_vp9_params` / `signaling_av1_params` / `signaling_h264_params` / `signaling_h265_params`) が有効である必要がある。`b_frame` はさらに `h264_b_frame` / `h265_b_frame` 設定が必要。無効な状態で指定すると Sora サーバーが検証エラーを返す。

# 制御
--duration <SEC>                    実行時間
--repeat-interval <SEC>             再接続間隔
--max-retry <N>                     最大リトライ
--retry-interval <SEC>              リトライ間隔

# 高度な設定
--sora-simulcast                    サイマルキャスト
--sora-spotlight                    スポットライト
--sora-spotlight-number <N>
--sora-data-channels <JSON>         DataChannel 設定
--sora-data-channel-signaling <BOOL>
--sora-data-channel-signaling-timeout <SEC>
--sora-disable-signaling-url-randomization
--scenario {reconnect}              シナリオ選択

# HTTP API / UI
--http-host <ADDR>
--http-port <PORT>
--ui
--ui-remote-url <URL>

# その他
--config <FILE>                     JSONC 設定ファイル
--log-level {verbose,info,warning,error,none}
--client-cert <FILE>                mTLS 証明書
--client-key <FILE>                 mTLS 秘密鍵
--insecure                          TLS 証明書検証スキップ
--output-file-connection-id <FILE>  connection ID 統計ファイル出力
--show-video-codec-capability       利用可能な映像コーデック能力を表示
```

## zakuro-rs 実装状況

### 主要な依存ライブラリ

| ライブラリ | バージョン | 用途 |
|-----------|-----------|------|
| shiguredo_webrtc | 0.152.1-canary.1 | libwebrtc バインディング |
| sora_sdk | 2026.2.0-canary.1 | Sora Rust SDK |
| shiguredo_http11 | 2026.6 | HTTP/1.1 サーバー |
| shiguredo_openh264 | 2026.2 | OpenH264 バインディング |
| shiguredo_video_device | 2026.3 | クロスプラットフォーム ビデオデバイス |
| shiguredo_mp4 | 2026.5 | MP4 コンテナからの音声トラック demux |
| shiguredo_opus | 2026.2 | Opus デコード (MP4 音声 → PCM) |
| shiguredo_fdk_aac | 2026.1 | AAC デコード (Linux 限定・feature `fdk-aac`・実行時動的ロード) |
| raden | 2026.2 | 2D ベクターグラフィックス (フェイク映像生成) |
| annotate-snippets | 0.12 | CLI 診断メッセージのソース注釈表示 |
| nojson | 0.3 | JSON / JSONC パース |
| noargs | 0.4 | CLI 引数パース |
| aws-lc-rs | 1.18 | 暗号ライブラリ (乱数生成) |
| jiff | 0.2 | UTC タイムスタンプ整形 (DuckDB ファイル名生成用) |
| tokio | 1.53 | 非同期ランタイム |
| tokio-stream | 0.1 | Stream ラッパー (ReceiverStream, IntervalStream) |
| tokio-util | 0.7 | CancellationToken, DelayQueue |
| duckdb | 1.10505 | DuckDB バインディング (統計記録に利用) |

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
- [x] Y4M 動画ファイル読込 (`--input-y4m`)
- [x] 実デバイスキャプチャ (`--video-input-device`)
- [x] MP4 パススルー送信 (`--input-mp4`)
- [ ] 解像度固定モード (`--fixed-resolution`)

### 音声

- [x] 音声無効化 (`--no-audio-device`)
- [x] フェイク音声フル実装 (BIP / BOP / HUM / ノイズ自動生成、48kHz モノラル・2 秒ループ)
- [x] WAV 音声ファイル読込 (`--input-wav`)
- [x] MP4 音声トラック送信 (`--input-mp4` 内の Opus / AAC をデコードして送信。音声トラック無し・未対応の MP4 は映像のみで続行)
  - AAC は Linux + feature `fdk-aac` で libfdk-aac の動的ロードが必要 (`--fdk-aac-lib`)。Linux 上でライブラリをロードできない環境で AAC 音声を含む MP4 を指定した場合は起動時エラー (feature 無し / 非 Linux では AAC は未対応扱い)

### コーデック

- [x] ビデオコーデック指定 (VP8/VP9/AV1/H264/H265)
- [x] オーディオコーデック指定 (Opus)
- [x] ビットレート指定 (映像・音声)
- [x] OpenH264 外部ライブラリ (`--openh264`)
- [x] コーデック個別エンコーダー指定 (`--vp8-encoder` 等)
- [x] コーデックパラメータ (`--sora-video-vp9-params` 等)
- [x] ビデオコーデック能力表示 (`--show-video-codec-capability`)

### 接続設定

- [x] シグナリング URL (複数指定可)
- [x] チャネル ID
- [x] クライアント ID (`--sora-client-id`)
- [x] バンドル ID (`--sora-bundle-id`)
- [x] ロール (sendonly/recvonly/sendrecv)
- [x] メタデータ (`--sora-metadata`)
- [x] シグナリング通知メタデータ (`--sora-signaling-notify-metadata`)
- [x] DataChannel シグナリング (`--sora-data-channel-signaling`)
- [x] WebSocket 切断無視 (`--sora-ignore-disconnect-websocket`)
- [x] 切断待ちタイムアウト (`--sora-disconnect-wait-timeout`)
- [x] mTLS (`--client-cert`, `--client-key`)
- [x] TLS 証明書検証スキップ (`--insecure`)
- [x] サイマルキャスト (`--sora-simulcast`, `--sora-simulcast-request-rid`)
- [x] スポットライト (`--sora-spotlight`, `--sora-spotlight-focus-rid`, `--sora-spotlight-unfocus-rid`)
- [x] DataChannel メッセージング (`--sora-data-channels`, ZAKURO ヘッダ付き自動送信)
- [ ] DataChannel カスタム `data` フィールド送信
- [ ] degradation-preference
- [ ] シグナリング URL ランダム化無効 (`--sora-disable-signaling-url-randomization`)
- [ ] DataChannel シグナリングタイムアウト (`--sora-data-channel-signaling-timeout`)

### HTTP API

- [x] ヘルスチェック (`GET /.ok`)
- [x] JSON-RPC 2.0 (`POST /rpc`)
- [x] GetVersion メソッド
- [ ] Query メソッド (DuckDB へのクエリー実行)
- [ ] UI リバースプロキシ (`--ui` / `--ui-remote-url`)

### シナリオ

- [x] ScenarioPlayer (Sleep, Disconnect, Reconnect 操作)
- [x] reconnect シナリオ (Reconnect → [Sleep(1-5s)] × 9 → ループ先頭 (Reconnect) に戻る。C++ 版の PlayVoiceNumberClient は実装しない)
- [x] DataChannel メッセージ自動送信 (ZAKURO ヘッダ付き)
- [x] vcs-hatch-rate (段階的起動)
- [x] シナリオ操作 SendDataChannelMessage
- [x] シナリオ操作 Exit
- [x] シナリオ操作 Reconnect
- [x] instance-hatch-rate (JSONC `instances` 配列と組み合わせて使用)

### その他

- [x] JSONC 設定ファイル (`--config`)
- [x] 設定ファイル検証サブコマンド (`zakuro lint`。検証のみで `--fix` は無し・今後も予定しない)
- [x] 設定ファイル整形サブコマンド (`zakuro fmt` / `zakuro fmt --check`)
- [x] NopVideoDecoder (受信映像廃棄)
- [x] DuckDB ファイルへの統計情報出力 (`--duckdb-output-dir` / `--duckdb-interval` / `--no-duckdb-output`)
- [x] ログレベル制御 (`--log-level`)
- [ ] 埋め込みリソース (フォント)
- [ ] connection ID ファイル出力 (`--output-file-connection-id`, DuckDB で代替可能)

### sora-rust-sdk / webrtc-rs 側の制約により未実装の機能

- シグナリング URL ランダム化無効: sora-rust-sdk 未実装 (デフォルトのランダム化のみ)
- DataChannel シグナリングタイムアウト: sora-rust-sdk 未実装

コーデックパラメータ (`VideoVP9Params` 等) は CLI 配線済み (`--sora-video-*-params`)。`DegradationPreference` は各 SDK / バインディングに API がある。zakuro-rs の CLI 未配線が残っている。

### 実装しない機能

- GameKeyCore (キーボード入力制御)
- GameAudioManager (ゲーム音声)
- シナリオ操作 PlaySubScenario (実用例がなく DataChannel 連続送信は別実装で実現済みのため)
- シナリオ操作 PlayVoiceNumberClient (数字音声再生。GameAudioManager 非実装方針に合わせ、C++ 版との差分として許容する)
- スポットライト数指定 (`--sora-spotlight-number`, Sora で非推奨のため sora-rust-sdk も対象外)

### 設計差分

| 項目 | zakuro (C++) | zakuro-rs (Rust) |
|------|-------------|-----------------|
| 非同期 | boost::asio io_context | tokio マルチスレッドランタイム |
| 描画 | Blend2D | Raden |
| 引数パース | CLI11 | noargs |
| JSON | Boost.JSON | nojson |
| HTTP | 自前実装 | shiguredo_http11 |
| 暗号 / 乱数 | OpenSSL / std | aws-lc-rs |
| シグナル処理 | SIGINT/SIGTERM | tokio::signal (Ctrl+C) |
| 統計通知 | コールバック | mpsc + watch チャネル |
| 統計永続化 | connection ID ファイル (`--output-file-connection-id`) | DuckDB (`--duckdb-output-dir` 等) |
| シャットダウン | io_context 停止 | CancellationToken |
| インスタンス起動 | シングルプロセス・マルチスレッド (`std::thread`) で instance-hatch-rate を実装 | シングルプロセス内の tokio タスクで instance-hatch-rate を実装 (DelayQueue + JoinSet) |
| 映像ファイル入力 | `--fake-video-capture` | `--input-y4m` |
| 音声ファイル入力 | `--fake-audio-capture` | `--input-wav` |
| WAV 未指定時のデフォルト音源 | External / GameAudio | Safari ループ (GameAudioManager 非実装のため) |
| カメラ指定 | `--video-device` | `--video-input-device` |
| MP4 パススルー | なし | `--input-mp4` |
| MP4 パススルー音声 | なし | `--input-mp4` 内の Opus / AAC をデコードして送信 (AAC は Linux + feature `fdk-aac`・`--fdk-aac-lib` が必要) |
| 設定ファイル検証 | なし | `zakuro lint <FILE.jsonc>` (負荷試験を起動せず構文・意味検証。`--fix` は無し) |
| 設定ファイル整形 | なし | `zakuro fmt <FILE.jsonc>` (コメント / 空行 / trailing comma を保持。`--check` で書き戻しなし確認) |
