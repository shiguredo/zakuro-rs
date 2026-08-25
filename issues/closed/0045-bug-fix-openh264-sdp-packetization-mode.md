# OpenH264 の SDP 広告に packetization-mode を追加する

- Created: 2026-08-24
- Completed: 2026-08-25
- Branch: feature/fix-openh264-sdp-packetization-mode
- Polished: 2026-08-25

## 目的

OpenH264 エンコーダーの SDP format 広告が packetization-mode 付きでなく、RFC 6184 の mode 0 (Single NAL Unit) の広告になっている。エンコーダーの出力は NonInterleaved (mode 1) のため、交渉結果との整合が保証されない。sora_sdk の既定実装が広告する packetization-mode=1 に揃える。

## 現状

- `src/openh264_video_codec.rs` の `Openh264VideoCodecCapability::get_supported_formats()` は Encoder 方向に `SdpVideoFormat::new("H264")` の bare 形式のみを返す (パラメータなし = mode 0 相当の広告)
- `Openh264Encoder::encode()` は `CodecSpecificInfo::set_h264_packetization_mode(NonInterleaved)` (mode 1) のコーデック情報を出力する
- sora_sdk の `video_codecs/openh264.rs` の `openh264_supported_formats()` は `packetization-mode=1` / `level-asymmetry-allowed=1` / `ScalabilityMode::L1T1` を広告する

## 設計方針

1. `Openh264VideoCodecCapability::get_supported_formats()` の Encoder 方向を sora_sdk の既存実装と同様に `packetization-mode=1` / `level-asymmetry-allowed=1` (可能なら `ScalabilityMode::L1T1`) を広告する形へ変更する
2. Sora サーバー側の確認方法として、シグナリング時の SDP (fmtp) に広告内容が反映されていることを確認する

## 完了条件

- OpenH264 エンコーダーの SDP format 広告に `packetization-mode=1` が含まれる
- `--sora-video-codec-type h264 --h264-encoder cisco_openh264 --openh264 <path>` のシグナリングで、answer (交渉後の SDP) の fmtp に `packetization-mode=1` が入り、映像が正常にエンコード・送信される

## 解決方法

`src/openh264_video_codec.rs` に `openh264_encoder_formats()` を新設し、`Openh264VideoCodecCapability::get_supported_formats()` の Encoder 方向の広告を、sora_sdk の `openh264_supported_formats()` と同一の `packetization-mode=1` / `level-asymmetry-allowed=1` / `ScalabilityMode::L1T1` 付きに変更した。エンコーダ出力 (RFC 6184 §8.1 の NonInterleaved = mode 1) と広告の整合を保証する。

### 変更ファイル

- `src/openh264_video_codec.rs`: `openh264_encoder_formats()` の追加と `get_supported_formats()` の変更

### テスト追加

- `openh264_encoder_formats_advertises_packetization_mode_1`: 広告に `packetization-mode=1` / `level-asymmetry-allowed=1` が含まれることを検証
- `openh264_encoder_formats_resolves_bare_h264_request`: 既定の `is_supported` (bare H264 要求の fuzzy match 解決) が壊れないこと、profile-level-id 付き要求も解決できることを検証
- `openh264_encoder_formats_advertises_scalability_l1t1`: `ScalabilityMode::L1T1` を広告することを検証

### 完了条件の確認状況

- 完了条件 1 (SDP format 広告に `packetization-mode=1`) は上記テストで検証済み
- 完了条件 2 (実サーバーでのシグナリング確認) は実 Sora サーバーと実 OpenH264 ライブラリが必要なため手動確認が必要 (CI では自動化不可)
