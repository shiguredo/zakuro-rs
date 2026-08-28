# README の注意点に `--input-mp4` と `--no-video-device` の排他指定を補完する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/update-readme-exclusivity-notes
- Polished: {YYYY-MM-DD}

## 目的

README の「注意点」に列挙した同時指定不可の組み合わせが実装より少ない。起動時にエラーになる組み合わせが文書から読み取れず、ユーザーだけが試行錯誤することになっている。一覧を実装 (`src/args.rs`) と一致させる。

## 現状

- README の「注意点」は次の排他しか書いていない
  - `--sandstorm` は `--input-y4m` / `--video-input-device` / `--input-mp4` と同時指定できない
  - `--input-mp4` は `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-wav` と同時指定できない
  - `--input-wav` は `--no-audio-device` / `--sora-audio=false` と同時指定できない
- `src/args.rs` の `parse_instance_args()` は上記に加えて次の排他を返すのに、README に無い
  - `--input-mp4` とエンコーダー実装指定 (`--vp8-encoder` / `--vp9-encoder` / `--av1-encoder` / `--h264-encoder` / `--h265-encoder`)
  - `--video-input-device` と `--input-y4m`
  - `--no-video-device` と `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-mp4`
- `src/args.rs` の `parse_args_from_argv()` は `--input-mp4` と `--openh264` の排他を返す (`--openh264` は共通引数のため検証の場所が他の排他と異なる) が、README に無い
- エンコーダー実装指定との排他だけ、エラー文言が `--input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません` という汎用表記になり、実際に指定したキー名が表示されない。この癖は文書に書いておかないとユーザーに伝わらない
- `--input-mp4` 使用時の `--sora-video-codec-type` / `--sora-video-bit-rate` 必須は README の MP4 パススルーの節に既出であり、注意点側で繰り返す必要は無い

## 設計方針

1. README の「注意点」に、実装が返す排他検証を 1 組み合わせ 1 行の既存書式で追記する (`--input-mp4` × エンコーダー実装指定、`--input-mp4` × `--openh264`、`--video-input-device` × `--input-y4m`、`--no-video-device` × 映像入力系 4 項目)
2. エンコーダー実装指定との排他は、エラー文言が `--vp8-encoder 等` の汎用表記になり指定したキー名は表示されないことを 1 文添える
3. 実装側は変更しない (エラー文言・挙動は現状維持)。文言を実際のキー名に変えるのは本 issue の対象外

## 完了条件

- README の注意点に列挙した同時指定不可の組み合わせと、`parse_instance_args()` / `parse_args_from_argv()` が返す排他検証が 1:1 対応していること
- 注意事項に書いた文言が実際のエラーメッセージと食い違っていないこと (`--vp8-encoder 等` の表記を含む)
- README 全体で重複・矛盾した記述になっていないこと (既存の MP4 パススルーの節と見比べる)

## 変更対象

- `README.md` (「注意点」セクション)
