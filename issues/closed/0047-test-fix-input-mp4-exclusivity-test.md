# エンコーダー実装指定 × `--input-mp4` の排他検証テストが偽陽性になっているのを修正する

- Created: 2026-08-25
- Completed: 2026-08-28
- Branch: feature/fix-input-mp4-exclusivity-test
- Polished: 2026-08-28

## 目的

`src/args.rs` のテスト `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` が、実在しない MP4 ファイルを指定しているため排他検証を実際にはテストできていない。テストが意図した排他検証 (エンコーダー実装指定 × `--input-mp4`) を本当に検証するように修正する。

## 現状

- `src/args.rs` の `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` は `--input-mp4 video.mp4` と実在しない相対パスを指定している
- `parse_instance_args()` は `--input-mp4` のファイル存在チェックを行うため、実在しない `video.mp4` では `input-mp4: file not found` エラーになり、後段の排他検証 (エンコーダー実装指定との排他) に到達しない
- テストの assert は `msg.contains("--input-mp4")` のみで、noargs のエラー書式 (`argument '--input-mp4' has an invalid value "video.mp4": input-mp4: file not found`) に含まれる `--input-mp4` に一致して pass してしまう (偽陽性)
- 実在するファイルを渡した場合に `parse_instance_args()` の排他検証が返す文言は `--input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません` であり、このテストが実際に指定している `--h264-encoder` という文字列は含まれない

## 設計方針

1. `tempfile::TempDir` に `std::fs::write(&mp4, b"dummy")` で実在する MP4 ファイルを作成し、その絶対パスを `--input-mp4` に渡してから `parse_args_from_argv` を呼ぶ (排他検証に到達できるようにする)。`parse_instance_args()` の存在チェックは `Path::exists()` だけでファイル内容は参照しないため中身は `b"dummy"` のままでよく、書き方は同じ `mod tests` 内の `parse_args_from_argv_rejects_input_mp4_with_openh264` に揃える
2. assert は排他検証由来の文言を逐語で指定する。`assert!(msg.contains("--input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません"), "エラーメッセージに排他検証の文言が含まれていない: {msg}")` とし、既存の `msg.contains("--input-mp4")` はこれに包含されるため置き換える。排他エラーは `AppError::Message` として返り `format!("{err}")` では先頭に `AppError::Message: ` が付くため、完全一致ではなく `contains` を使う。テストが渡している `--h264-encoder` を含むことを assert してはならない (排他文言には出ないため fail する)
3. 「同時に指定できません」という部分一致だけの検証、「`file not found` を含まないこと」だけの negation 検証はどちらも認めない。`parse_instance_args()` と `parse_args_from_argv()` には `--input-mp4 と --input-y4m` / `--input-mp4 と --input-wav` / `--input-mp4 と --openh264` など「同時に指定できません」で終わる排他エラーが複数あり、エンコーダー実装指定以外が立っても pass してしまうため
4. 変更するのは `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` だけにする。`parse_instance_args()` の検証ロジックとエラー文言は変更しない (指定されたオプション名を挙げず `--vp8-encoder 等` の汎用表記にしているのは現行の仕様であり、文言を実際の指定に合わせる変更は本 issue の対象外)

## 完了条件

- `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` が実在する MP4 ファイルを使い、排他検証のエラーメッセージ (`--input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません`) を検証していること
- エンコーダー実装指定の排他検証を一時的に無効化すると当該テストが fail することを確認していること (確認後は元に戻し、コミットしない。これが偽陽性の解消そのものの確認になる)
- 全テストスイートが通過すること

## 解決方法

`src/args.rs` の `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` だけを変更した。`tempfile::TempDir` に `std::fs::write(&mp4, b"dummy")` で実在する MP4 ファイルを作成し、その絶対パスを `--input-mp4` に渡すようにしたため、`parse_instance_args()` のファイル存在チェックを通過してエンコーダー実装指定との排他検証に到達する。書き方は同じ `mod tests` 内の `parse_args_from_argv_rejects_input_mp4_with_openh264` に揃えた。

assert は `msg.contains("--input-mp4")` を排他検証の文言そのもの `msg.contains("--input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません")` に置き換えた。排他文言は `parse_instance_args()` 内に 1 箇所しか存在せず、他の「同時に指定できません」で終わる排他エラー (`--input-mp4 と --input-y4m` 等) とは区別される。`AppError::Message` 経由で先頭に `AppError::Message: ` が付くため完全一致ではなく `contains` を使っている。テストが渡している `--h264-encoder` は排他文言に現れないため assert していない。`parse_instance_args()` の検証ロジックとエラー文言は変更していない。

### 変更ファイル

- `src/args.rs`: `parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` のテスト本文のみ (本番コードは無変更。`CODEBASE.md` の規約により develop へ直接コミットし、`Branch:` のブランチは切っていない)

### 確認したこと

- 実在しない `video.mp4` を渡した場合のエラーは `argument '--input-mp4' has an invalid value "video.mp4": input-mp4: file not found` であり、新しい assert 文言を含まないことを実測で確認 (旧 assert はこの文言で pass していた)
- 実在する `b"dummy"` MP4 を渡すと `AppError::Message: --input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません` になることを実測で確認
- エンコーダー実装指定の排他検証を一時的に無効化すると当該テストが `expect_err` で fail することを確認し、確認後は元に戻してコミットしていない
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` (204 passed) がすべて通過すること
