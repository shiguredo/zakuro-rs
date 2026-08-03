# コーデック個別エンコーダー指定 (`--vp8-encoder` 等) を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-video-codec-implementation-selection
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) では `--vp8-encoder` / `--vp9-encoder` / `--av1-encoder` / `--h264-encoder` / `--h265-encoder` でコーデックごとにエンコーダ実装 (`internal` / `cisco_openh264` / `intel_vpl` / `nvidia_video_codec` / `amd_amf`) を選択できる (`zakuro/src/util.cpp` の `video_codec_implementation_map`)。特定のハードウェアエンコーダを強制して負荷試験を行うために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/args.rs` の `InstanceArgs` にはコーデック実装を指定するオプションがなく、`--openh264` で OpenH264 ライブラリのパスを指定するのみ
- `src/main.rs` の `run_zakuro_instance()` は `SoraConnectionContextConfig::video_codec_capabilities` に登録した capability (OpenH264 / NopVideoDecoder / MP4 パススルー) から `VideoCodecPreference` を構築するだけで、実装の選択は SDK 側の優先順位に任されている
- sora_sdk には `VideoCodecImplementation` (name / description) と `VideoCodecPreference` (`PreferenceCodec::set_implementation()` / `get_or_add()`) があり、コーデックごとの実装指定 API は存在する

## 設計方針

1. `src/args.rs` の `InstanceArgs` に `vp8_encoder` / `vp9_encoder` / `av1_encoder` / `h264_encoder` / `h265_encoder` (`Option<String>`) を追加し、`is_flag()` 相当の値付きオプションとして定義する
2. 許容値は C++ 版と同じ `internal` / `cisco_openh264` / `intel_vpl` / `nvidia_video_codec` / `amd_amf` とする
3. sora_sdk に C++ 版 `sora::GetVideoCodecCapability()` 相当の「全実装列挙 API」があるかを調査し、ある場合はその API で選択肢を解決する。ない場合は zakuro-rs が登録した capability (`is_supported()` / `get_implementation()` / `create_video_encoder()`) を対象に、指定実装と一致する capability のみを `video_codec_capabilities` に登録する方式で実装する
4. `VideoCodecPreference::get_or_add()` + `set_implementation()` で指定実装を設定し、`src/main.rs` の capability 登録に反映する

## 完了条件

- 5 つのオプションが `--help` に表示され、C++ 版と同じ許容値を取る
- `--h264-encoder cisco_openh264` のような指定で、指定した実装のエンコーダが利用される (ログで確認可能)
- 指定実装が利用できない場合は起動時にエラーまたは警告で通知される
