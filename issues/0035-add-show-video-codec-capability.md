# ビデオコーデック能力表示 (`--show-video-codec-capability`) を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-show-video-codec-capability
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) では `--show-video-codec-capability` で利用可能なコーデックエンジン (internal / cisco_openh264 / intel_vpl / nvidia_video_codec / amd_amf) ごとに、対応コーデックの Encoder / Decoder 対応状況とコーデックパラメータを一覧表示し、表示後に終了する (`zakuro/src/util.cpp` の `show_video_codec_capability` 処理)。環境ごとにどのエンコーダが利用可能かを確認してから負荷試験を構成するために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- 該当オプションは存在しない
- sora_sdk に C++ 版 `sora::GetVideoCodecCapability()` 相当の「全実装を列挙して利用可否を返す」API があるかは未確認
- zakuro-rs が把握できるコーデック能力は `src/main.rs` で登録した capability (OpenH264 / NopVideoDecoder / MP4 パススルー) のみ。各 capability は `sora_sdk::VideoCodecCapability` トレイトの `get_implementation()` / `get_supported_formats()` / `is_supported()` を実装している

## 設計方針

1. sora_sdk の API を調査し、C++ 版相当の列挙 API があればそれを利用する
2. 列挙 API がない場合は、`--show-video-codec-capability` 指定時に登録予定の capability (`--openh264` 指定時は OpenH264、受信ロール時は NopVideoDecoder 等) を構築して、`get_implementation()` と `get_supported_formats()` の結果を C++ 版と同じフォーマットで表示する
3. 表示後は `--version` と同じく `std::process::exit(0)` で即時終了する
4. 出力先は標準出力とし、ログと混ざらないようにする

## 完了条件

- `--show-video-codec-capability` が `--help` に表示される
- 実行すると利用可能なコーデック実装と Encoder / Decoder 対応が標準出力に表示され、表示後に終了する
- `--openh264` と併用すると OpenH264 の能力が表示に含まれる
