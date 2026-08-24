# OpenH264 の SDP 広告に packetization-mode を追加する

- Created: 2026-08-24
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-openh264-sdp-packetization-mode
- Polished: {YYYY-MM-DD}

## 目的

OpenH264 エンコーダーの SDP format 広告が packetization-mode 付きでなく、RFC 6184 の mode 0 (Single NAL Unit) の広告になっている。エンコーダーの出力は NonInterleaved (mode 1) のため、交渉結果との整合が保証されない。sora_sdk の既定実装が広告する packetization-mode=1 に揃える。

## 現状

- `src/openh264_video_codec.rs` の `Openh264VideoCodecCapability::get_supported_formats()` は Encoder 方向に `SdpVideoFormat::new("H264")` の bare 形式のみを返す (パラメータなし = mode 0 相当の広告)
- `Openh264Encoder::encode()` は `CodecSpecificInfo::set_h264_packetization_mode(NonInterleaved)` (mode 1) のコーデック情報を出力する
- sora_sdk の `video_codecs/openh264.rs` の `openh264_supported_formats()` は `packetization-mode=1` / `level-asymmetry-allowed=1` / `ScalabilityMode::L1T1` を広告する
- Sora サーバーは SDP の fmtp に含まれるパラメータを確認して映像を受信するため、広告と実際の出力の不整合は最悪の場合パケット化モードの解釈に影響する

## 設計方針

1. `Openh264VideoCodecCapability::get_supported_formats()` の Encoder 方向を sora_sdk の既存実装と同様に `packetization-mode=1` / `level-asymmetry-allowed=1` (可能なら `ScalabilityMode::L1T1`) を広告する形へ変更する
2. Sora サーバー側の確認方法として、シグナリング時の SDP (fmtp) に広告内容が反映されていることを確認する

## 完了条件

- OpenH264 エンコーダーの SDP format 広告に `packetization-mode=1` が含まれる
- `--h264-encoder cisco_openh264 --openh264 <path>` のシグナリングで、offer の SDP (fmtp) に `packetization-mode=1` が入り、映像が正常にエンコード・送信される
