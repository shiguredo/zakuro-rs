# エンコーダー実装指定 × `--input-mp4` の排他検証テストが偽陽性になっているのを修正する

- Created: 2026-08-25
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-input-mp4-exclusivity-test
- Polished: {YYYY-MM-DD}

## 目的

`src/args.rs` のテスト `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` が、実在しない MP4 ファイルを指定しているため排他検証を実際にはテストできていない。テストが意図した排他検証 (エンコーダー実装指定 × `--input-mp4`) を本当に検証するように修正する。

## 現状

- `src/args.rs` の `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` は `--input-mp4 video.mp4` と実在しない相対パスを指定している
- `parse_instance_args()` は `--input-mp4` のファイル存在チェックを行うため、実在しない `video.mp4` では `input-mp4: file not found` エラーになり、後段の排他検証 (エンコーダー実装指定との排他) に到達しない
- テストの assert は `msg.contains("--input-mp4")` のみで、noargs のエラー書式 (`argument '--input-mp4' has an invalid value "video.mp4": input-mp4: file not found`) に含まれる `--input-mp4` に一致して pass してしまう (偽陽性)

## 設計方針

1. `tempfile::TempDir` で実在するダミー MP4 ファイルを作成してから `parse_args_from_argv` を呼ぶ (排他検証に到達できるようにする)
2. エラーメッセージがファイル不存在エラーでなく排他検証由来であることを assert で確認する

## 完了条件

- `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` が実ファイルを使い、排他検証のエラーメッセージ (「同時に指定できません」等) を検証していること
- 全テストスイートが通過すること
