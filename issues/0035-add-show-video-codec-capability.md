# ビデオコーデック能力表示 (`--show-video-codec-capability`) を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-show-video-codec-capability
- Polished: 2026-08-24

## 目的

zakuro (C++) では `--show-video-codec-capability` で利用可能なコーデックエンジン (internal / cisco_openh264 / intel_vpl / nvidia_video_codec / amd_amf) ごとに、対応コーデックの Encoder / Decoder 対応状況とコーデックパラメータを一覧表示し、表示後に終了する (`zakuro/src/util.cpp` の `show_video_codec_capability` 処理)。環境ごとにどのエンコーダが利用可能かを確認してから負荷試験を構成するために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- 該当オプションは存在しない
- sora_sdk には C++ 版 `sora::GetVideoCodecCapability()` 相当の「全実装を列挙して利用可否を返す」API は存在しない (sora_sdk 2026.1.0-canary.13 で確認)
- zakuro-rs が把握できるコーデック能力は `SoraConnectionContextConfig::default()` が登録する capability (SDK 既定の `internal`、macOS/iOS では `internal-apple`) と、`src/main.rs` の `run_zakuro_instance()` が登録する capability (OpenH264 / NopVideoDecoder / MP4 パススルー) の合計。各 capability は `sora_sdk::VideoCodecCapability` トレイトの `get_implementation()` / `get_supported_formats()` / `is_supported()` を提供する

## 設計方針

1. `--show-video-codec-capability` は `CommonArgs` の bool フラグとして定義し、`is_common_key()` / `is_flag()` にも追加して `--help` に表示する。しかし実際の処理は通常のパース経路ではなく、`--version` / `--help` と同じく `parse_args()` の pre-parse 段階で argv を走査して検出し、表示後に終了する (通常経路だと必須の `--sora-signaling-url` / `--sora-channel-id` / `--sora-role` を要求されてしまい、C++ 版で可能な `zakuro --show-video-codec-capability` の単独起動が成立しないため)
2. 表示対象は次の capability とする:
   - `SoraConnectionContextConfig::default()` が登録する `internal` / `internal-apple`
   - `--openh264` 指定時: OpenH264 (`--openh264` は同じ pre-parse で値を取り、表示用に OpenH264 ライブラリをロードして capability を構築する。ロード失敗時は OpenH264 を表示対象から除外するがエラーにはしない)
   - `--input-mp4` 指定時: MP4 パススルー
   - 受信ロール (`--sora-role sendrecv` / `recvonly`) 指定時: NopVideoDecoder
   - role は `InstanceArgs` のため、表示は CLI の `--sora-role` のみを参照し、`--config` (JSONC) の設定は対象にしない (C++ 版も `show_video_codec_capability` を config 処理より前で終了するため一致)
3. 表示フォーマットは C++ 版を基本とし、capability ごとに次を表示する:
   - `Engine: <実装名> (<description>)` — `get_implementation()` の結果
   - `  - <CODEC> Encoder` / `  - <CODEC> Decoder` — `is_supported()` (または `get_supported_formats()`) の結果。`VideoCodecType` の各値 (Vp8 / Vp9 / H264 / H265 / Av1) を走査する
   - `    - Codec Parameters: <param>=<value> ...` — `get_supported_formats()` が返す各フォーマットのパラメータ (C++ 版は JSON 表記だが sora_sdk が返すのは key=value のためこの表記にする)
   - C++ 版の `Engine Parameters` 行は sora_sdk に相当する情報がないため出力しない (C++ 版との意図した差分)
4. 表示後は `--version` と同じく `std::process::exit(0)` で即時終了する
5. 出力先は標準出力とし、ログ (標準エラー) と混ざらないようにする
6. `issues/0033` の実装後は OpenH264 の実装名が `cisco_openh264` になるため、その場合の表示もそれに従う (0035 単独でも動作し、その時点の実装名で表示してよい)

## 完了条件

- `--show-video-codec-capability` が `--help` に表示される
- 単独起動 (`zakuro --show-video-codec-capability`) で、利用可能なコーデック実装と Encoder / Decoder 対応が標準出力に表示され、表示後に終了する
- `--openh264 <path>` と併用すると OpenH264 の能力が表示に含まれる
