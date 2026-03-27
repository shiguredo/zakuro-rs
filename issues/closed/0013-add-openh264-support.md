# OpenH264 外部ライブラリ対応 (`--openh264`) を追加する

Created: 2026-03-27
Completed: 2026-03-27
Model: Opus 4.6

## 概要

`--openh264` オプションで OpenH264 共有ライブラリのパスを指定し、H.264 ソフトウェアエンコード/デコードを利用できるようにする。

## 根拠

zakuro (C++) では `--openh264` で OpenH264 ライブラリを動的ロードし、ハードウェアエンコーダが利用できない環境でも H.264 を使用できる。CI 環境やヘッドレスサーバーでの負荷試験に必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. コマンドライン引数

- `--openh264 <PATH>` オプションを追加する

### 2. OpenH264 連携

- 指定されたパスから OpenH264 共有ライブラリを動的ロードする
- H.264 エンコーダ/デコーダとして登録する

## 解決方法

`src/openh264_video_codec.rs` を新規作成し、以下のコンポーネントを実装した:

- `load_openh264_library()`: `shiguredo_openh264::Openh264Library` の読み込みとバージョン確認
- `Openh264Encoder`: `VideoEncoderHandler` トレイトを実装した OpenH264 エンコーダラッパー
  - `init_encode` で解像度・ビットレート・フレームレートを取得してエンコーダを初期化
  - `encode` で I420 フレームを OpenH264 に渡し、Annex B 形式で WebRTC コールバックに返す
  - `set_rates` でビットレート・フレームレートの動的変更に対応
  - 解像度変更時はエンコーダを再初期化
- `Openh264VideoCodecCapability`: `VideoCodecCapability` トレイトを実装し、H.264 エンコーダとして登録

`src/args.rs` に `--openh264 <PATH>` オプションを追加。
`src/main.rs` で OpenH264 ライブラリのロードと `SoraClientContextConfig` へのコーデック能力登録を追加。
