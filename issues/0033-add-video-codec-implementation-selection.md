# コーデック個別エンコーダー指定 (`--vp8-encoder` 等) を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-video-codec-implementation-selection
- Polished: 2026-08-24

## 目的

zakuro (C++) では `--vp8-encoder` / `--vp9-encoder` / `--av1-encoder` / `--h264-encoder` / `--h265-encoder` でコーデックごとにエンコーダ実装 (`internal` / `cisco_openh264` / `intel_vpl` / `nvidia_video_codec` / `amd_amf`) を選択できる (`zakuro/src/util.cpp` の `video_codec_implementation_map`)。特定のハードウェアエンコーダを強制して負荷試験を行うために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/args.rs` の `InstanceArgs` にはコーデック実装を指定するオプションがなく、`--openh264` で OpenH264 ライブラリのパスを指定するのみ
- `src/main.rs` の `run_zakuro_instance()` は `SoraConnectionContextConfig::video_codec_capabilities` に登録した capability (OpenH264 / NopVideoDecoder / MP4 パススルー) から `VideoCodecPreference::new_from_capability()` で preference を構築して `merge()` するだけで、コーデックごとの実装指定は行えない (実装の決定は SDK 既定の internal と merge 順に依存する)
- sora_sdk には `VideoCodecImplementation` (name / description) と `VideoCodecPreference` (`PreferenceCodec::set_implementation()` / `get_or_add()`) があり、コーデックごとの実装指定 API は存在する。ただし C++ 版 `sora::GetVideoCodecCapability()` 相当の「全実装列挙 API」は存在しない (sora_sdk 2026.1.0-canary.13 で確認)

## 設計方針

1. `src/args.rs` の `InstanceArgs` に `vp8_encoder` / `vp9_encoder` / `av1_encoder` / `h264_encoder` / `h265_encoder` (`Option<String>`) を追加し、`noargs::opt()` の値付きオプションとして定義する。`is_common_key()` と `is_flag()` には追加しない (`is_flag()` は bool フラグ専用で、値付きオプションを追加すると `split_cli_argv()` と `dedupe_argv_last_wins()` が値を取り込まず誤動作するため)
2. 許容値は C++ 版と同じ `internal` / `cisco_openh264` / `intel_vpl` / `nvidia_video_codec` / `amd_amf` とする。C++ 版は CLI11 の `ignore_case` で大文字小文字を許容するが、zakuro-rs は `--sora-video-codec-type` 等と同じく小文字のみ受理する
3. sora_sdk の capability 実装名は C++ 版の値と一致しないため (例: `cisco_openh264` → `openh264`)、次の対応表で解決する。このうち `cisco_openh264` は zakuro-rs の OpenH264 capability の `get_implementation()` の実装名を `"cisco_openh264"` に変更して対応する (C++ 版の実装名と揃える。変更の影響は `validate_video_codec_preference` の照合とログ・DuckDB 等の出力名に及ぶため影響範囲を確認すること)

   | C++ 版の値 | 解決後の実装名 | 利用可否 |
   |---|---|---|
   | `internal` | `internal` | 利用可 (SoraConnectionContextConfig の既定 capability が担うため、指定は機能上実質無変更) |
   | `cisco_openh264` | `cisco_openh264` | 利用可 (`--openh264` 指定時のみ。OpenH264 ライブラリ未ロード時はエラー) |
   | `intel_vpl` | `vpl` | 利用不可 (sora_sdk の `vpl` feature 未有効化) |
   | `nvidia_video_codec` | `nvcodec` | 利用不可 (sora_sdk の `nvcodec` feature 未有効化) |
   | `amd_amf` | `amf` | 利用不可 (sora_sdk の `amf` feature 未有効化) |

   ハードウェア系 3 値の「利用不可」は、sora_sdk の feature 有効化 (ネイティブ実装依存の追加) を伴うため本 issue では対象外とし、指定された場合は起動時にエラーで通知する (C++ 版との差分として明記する)
4. `src/main.rs` の `run_zakuro_instance()` で、指定されたコーデックの Encoder 方向のみ `VideoCodecPreference::get_or_add(CodecDirection::Encoder, <指定コーデック>, <解決済み実装名の VideoCodecImplementation>)` でエントリを取得し `set_implementation()` で実装を差し替える。Decoder 方向は変更しない (C++ 版も encoder のみ指定し、decoder は NopVideoDecoder 登録のまま)。capability 登録は現状 (OpenH264 / NopVideoDecoder / MP4 パススルー) を維持する
5. 対応する capability が `video_codec_capabilities` に登録されていない場合 (ハードウェア系 3 値と `--openh264` 未指定時の `cisco_openh264`) は、起動時にエラーメッセージを出して終了する (sora_sdk の `validate_video_codec_preference()` が `SoraConnectionContext::new_with_config()` で失敗するのを利用してよい)

## 完了条件

- 5 つのオプションが `--help` に表示され、C++ 版と同じ許容値 (小文字のみ) を取る
- `--h264-encoder cisco_openh264 --openh264 <path>` のような指定で、指定した実装のエンコーダが利用される (ログで確認可能)
- `--vp8-encoder intel_vpl` のような利用不可の値、または `--openh264` 未指定時の `--h264-encoder cisco_openh264` を指定すると、起動時にエラーメッセージ付きで終了する
