# MP4 パススルー映像入力機能を追加する

Created: 2026-03-27
Completed: 2026-03-27

## 概要

`--input-mp4` オプションで MP4 ファイルからエンコード済み映像をパススルー送信する機能を追加する。

## 背景・根拠

負荷試験時にエンコーダの CPU 負荷がボトルネックとなり、同時に起動できる仮想クライアント数が制限される。
MP4 ファイルから事前にエンコードされた映像データを再エンコードせずにそのまま WebRTC で送信することで、
エンコーダの CPU 負荷を完全に排除し、より大規模な負荷試験を実現できる。

## 対応内容

- `shiguredo_mp4` クレートを依存に追加
- MP4 ファイルのデマルチプレクスとビデオサンプル抽出 (`Mp4SampleReader`)
- H.264 AVCC→Annex B 変換、H.265 HVCC→Annex B 変換
- VP8/VP9/AV1 はそのまま使用
- パススルーエンコーダ (`Mp4PassthroughEncoder`) の実装
  - `VideoEncoderHandler` トレイトを実装
  - `has_trusted_rate_controller = true` で BWE 干渉を防止
- パススルーコーデック能力 (`Mp4PassthroughVideoCodecCapability`) の実装
  - `VideoCodecCapability` トレイトを実装
  - MP4 から検出したコーデックのエンコーダのみを提供
- MP4 映像キャプチャ (`Mp4VideoCapturer`) の実装
  - 専用 OS スレッドでフレームペーシング
  - 累積タイミングテーブルによる絶対時刻ベースのペーシング（ドリフト防止）
  - EOF で自動ループ
- CLI オプション `--input-mp4` の追加
  - `--video-input-device`、`--fake-video-capture`、`--sandstorm` と排他
  - `--sora-video-codec-type` と `--sora-video-bit-rate` の指定を必須化

## 対応コーデック

- H.264 (AVC1)
- H.265 (HEV1, HVC1)
- VP8 (VP08)
- VP9 (VP09)
- AV1 (AV01)

## 制約

- 映像のみ対応（音声はパススルーしない）
- sendonly でのみ動作
- B フレームのある MP4 はジッタの原因となる
- RTCP の再送・キーフレーム要求は無視

## 解決方法

`src/mp4_video_capturer.rs` を新規作成し、以下のコンポーネントを実装した:

- `Mp4SampleReader`: MP4 ファイルの読み込みとデマルチプレクス
- `Mp4VideoCapturer`: 専用スレッドでのフレームペーシング
- `Mp4PassthroughEncoder`: VideoEncoderHandler によるパススルー送信
- `Mp4PassthroughVideoCodecCapability`: VideoCodecCapability の実装

`src/args.rs` に `--input-mp4` オプションと排他バリデーションを追加。
`src/main.rs` で MP4 パススルー時に SoraClientContextConfig をカスタマイズしてパススルーコーデック能力を登録する処理を追加。
