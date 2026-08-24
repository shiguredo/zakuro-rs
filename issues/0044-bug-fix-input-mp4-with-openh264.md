# `--input-mp4` と `--openh264` の併用で H.264 パススルーが壊れることを修正する

- Created: 2026-08-24
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-input-mp4-with-openh264
- Polished: {YYYY-MM-DD}

## 目的

H.264 の MP4 パススルー送信 (`--input-mp4`) を指定しながら `--openh264` を指定すると、H.264 エンコーダーの実装が `mp4-passthrough` から `cisco_openh264` に上書きされ、パススルーがエラーになる問題を修正する。ユーザーに気づかれずに壊れたままであるコーデック実装指定の組み合わせを、起動時エラーまたは正常動作のどちらかに確実に振り分ける。

## 現状

- `src/main.rs` の `run_zakuro_instance()` 内の context_config 構築ブロックでは、MP4 パススルー (mp4-passthrough) の登録・merge が先で、OpenH264 (cisco_openh264) の登録・merge が後である
- `sora_sdk` の `VideoCodecPreference::merge()` は同じ方向・コーデック種別のエントリを後勝ちで上書きするため、両方指定すると H.264 エンコーダーは `cisco_openh264` に置き換わる
- `Mp4VideoCapturer` (MP4 パススルー) はエンコード済みサンプルを送る。`Openh264Encoder` は I420 フレームを要求し、エンコード済みサンプルは `as_i420()` が `None` となり `encode` がエラーを返す。結果として映像が送信されないままになる
- `--input-mp4` は `--vp8-encoder` 等のエンコーダー実装指定とは排他にした (src/args.rs の `parse_instance_args`) が、`--openh264` は排他にしていない。`--openh264` も実質エンコーダー実装を切り替えるため対象漏れになっている

## 設計方針

1. `--input-mp4` と `--openh264` の併用を起動時エラーにする。MP4 パススルーはエンコード済み映像をそのまま送るため、OpenH264 ライブラリのロードには意味がなく、vcs 単位の失敗に比べ起動時に検知するのが安全
2. 検証の実装は src/args.rs の `parse_instance_args()` または `parse_args_from_argv()` のバリデーションに追加し、既存の排他検証と同様のエラーメッセージ形式にする

## 完了条件

- `--input-mp4` と `--openh264` を同時に指定すると、起動時にエラーメッセージ付きで終了する
- `--input-mp4` のみ、`--openh264` のみの指定では従来どおり動作する
