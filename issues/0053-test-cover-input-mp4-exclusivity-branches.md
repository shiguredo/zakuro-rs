# `--input-mp4` とエンコーダー実装指定の排他検証を 5 分岐すべてに広げる

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-input-mp4-exclusivity-coverage
- Polished: {YYYY-MM-DD}

## 目的

`src/args.rs` の `parse_instance_args()` が持つエンコーダー実装指定との排他検証は 5 オプションを対象しているが、テストは `--h264-encoder` の 1 分岐しか検証していない。残りの 4 分岐が退行してもテストスイートが緑のままになる状態を解消する。

## 現状

- `parse_instance_args()` の排他検証は `vp8_encoder` / `vp9_encoder` / `av1_encoder` / `h264_encoder` / `h265_encoder` のいずれかが指定されていれば `--input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません` を返す
- `src/args.rs` の `mod tests` にある `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` は `--h264-encoder internal` だけを渡すため、たとえば `vp8_encoder.is_some()` の項が削除されても fail しない (vp8 / vp9 / av1 / h265 の 4 分岐は未検証)
- 同じ `mod tests` の `parse_args_from_argv_rejects_hardware_encoder_implementation` は複数のキーを `for` で回しており、ループ書のテストには前例がある
- 姉妹テストの `parse_args_from_argv_rejects_input_mp4_with_openh264` と `parse_args_from_argv_rejects_input_mp4_with_input_wav` は排他文言そのものではなく、含まれるオプション名 (`--input-mp4` と `--openh264` 等) のみを assert している

## 設計方針

1. `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` を `vp8-encoder` / `vp9-encoder` / `av1-encoder` / `h264-encoder` / `h265-encoder` を回すループに変更する。一時 MP4 は `tempfile::TempDir` を 1 回作成して全キーで共有する
2. 全キーで実装値は `internal` に統一する。`cisco_openh264` は `parse_instance_args()` の実装値検証で H.264 以外が拒否されるため、排他検証まで到達しない
3. assert は 5 キー共通で排他文言の逐語にする。`vp8` 以外のキーを指定しても文言は `--vp8-encoder 等` のまま (実際に指定したキー名は文言に出ない) であり、キー名での assert は行わない
4. 姉妹テスト 2 件 (`parse_args_from_argv_rejects_input_mp4_with_openh264` / `parse_args_from_argv_rejects_input_mp4_with_input_wav`) も実際に返る排他文言の逐語に揃え、他の排他エラーと混同しても pass しない形にする

## 完了条件

- `parse_instance_args()` の排他検証の条件を 1 つでも壊すと当該テストが fail すること
- `--input-mp4` 系の排他検証テストが、エンコーダー実装指定・`--openh264`・`--input-wav` のいずれについても実測の排他文言を逐語で検証していること
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` が通過すること

## 変更対象

- `src/args.rs` (`mod tests` の `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4`、`parse_args_from_argv_rejects_input_mp4_with_openh264`、`parse_args_from_argv_rejects_input_mp4_with_input_wav`)
