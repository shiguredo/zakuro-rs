# README の注意点を実装と同じ排他検証の一覧に揃える

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/update-readme-exclusivity-notes
- Polished: 2026-08-28

## 目的

README の「注意点」に列挙した同時指定不可の組み合わせが実装より少ない。起動時にエラーになる組み合わせが文書から読み取れず、ユーザーだけが試行錯誤することになっている。一覧を実装 (`src/args.rs`) と一致させる。

## 現状

- README の「注意点」は次の 3 行の排他しか書いていない
  - `--sandstorm` は `--input-y4m` / `--video-input-device` / `--input-mp4` と同時指定できない
  - `--input-mp4` は `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-wav` と同時指定できない
  - `--input-wav` は `--no-audio-device` / `--sora-audio=false` と同時指定できない
- `src/args.rs` の `parse_instance_args()` が返す排他検証 (`〜 と同時に指定できません`) は 13 検証ある。うち 1 検証がエンコーダー実装指定のもので、5 キー (`--vp8-encoder` / `--vp9-encoder` / `--av1-encoder` / `--h264-encoder` / `--h265-encoder`) を 1 つの条件で受け持っている。README の 3 行が対応するのは 7 検証 (`--sandstorm` 行の 3 と `--input-mp4` 行の 4 と `--input-wav` 行の `--no-audio-device` 1。`--sandstorm` × `--input-mp4` は 2 行の両方に載るので 1 回に数える)。README に無いのは次の 6 検証
  - `--input-mp4` × エンコーダー実装指定 (5 キーで 1 検証)
  - `--video-input-device` × `--input-y4m`
  - `--no-video-device` × `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-mp4` の 4 検証
  - `src/args.rs` の `parse_args_from_argv()` が返す `--input-mp4` × `--openh264` はこれとは別関数にあり、これも README に無い (`--openh264` は共通引数のため検証の場所が他の排他と異なる)。合計 7 検証を、設計方針 1 と 2 の 4 行で 1 回ずつ書き表す
- 本 issue が対象にする排他検証は、実装側でエラー文言が `〜 と同時に指定できません` になるものだけである。文言がそれ以外 (`指定が必須です` / `指定できません` / `指定が必要です` / `両方指定する必要があります`) の必須・依存系検証は対象外とする
  - MP4 使用時の `--sora-video-codec-type` / `--sora-video-bit-rate` 必須は README の MP4 パススルーの節に既出なので注意点で繰り返さない
  - `--h264-encoder cisco_openh264` 使用時の `--openh264` 必須 (`parse_args_from_argv()` が返す) と `--sora-video-*-params` 使用時のコーデック種別整合 (`validate_video_params_codec_type()` が返す) は、排他ではなく片方向の要件なので対象外に留める。この 2 件の文書化は本 issue では行わない
  - 既存の `--input-wav` 行にある `--sora-audio=false` も実装の文言は `--input-wav 使用時は --sora-audio=false を指定できません` であり排他検証ではない。既存行は変更しないため、この 1 件は README に載っているが対象検証には対応しない、という残り方になる
- 既存の注意点の書式は「起点となるオプション 1 行に、同時指定できないオプションを `/` 区切りで並べる」形で、起点 1 行が複数の組み合わせを兼ねている。同じ組み合わせを双方向で書いているのは `--sandstorm` 行と `--input-mp4` 行に共通して載る `--sandstorm` × `--input-mp4` の 1 件だけで、それ以外は一方向のみ (`--sandstorm` 起点の行があるため `--input-y4m` 起点の行は無い)

## 設計方針

1. 既存の 6 行 (`--http-host` / `--client-cert` / `--sandstorm` / `--input-mp4` / `--input-wav` / `--openh264`) は変更・マージせず、`--input-wav` の行と `--openh264` の行の間に新規行を追記する (`--input-mp4` 起点の排他は「映像入力との排他」と「再エンコードしないこと由来の排他」で理由が別なので、既存行へ足さず分ける)。まず次の 3 行

   ```
   - `--input-mp4` は `--vp8-encoder` / `--vp9-encoder` / `--av1-encoder` / `--h264-encoder` / `--h265-encoder` (エンコーダー実装指定) と同時指定できません
   - `--input-mp4` は `--openh264` と同時指定できません
   - `--no-video-device` と `--video-input-device` / `--input-y4m` / `--sandstorm` / `--input-mp4` は同時指定できません
   ```

2. `--video-input-device` × `--input-y4m` はどの行にも書いていない (`--no-video-device` の行が `--video-input-device` / `--input-y4m` を並べるが、あれは `--no-video-device` との排他をまとめて書いている行であり、`--video-input-device` と `--input-y4m` の組み合わせではない)。起点を `--video-input-device` とした行は存在しないので、次の行を追記する。この行が並べる 4 件のうち `--sandstorm` と `--input-mp4` は既存行が逆向きでカバー済み、`--no-video-device` は 1 の新設行がカバーするため、この行で初めて載るのは `--input-y4m` の 1 件である。既存行を伸ばさず起点別の行にまとめる書式に従って、残りの 3 件も並記する

   ```
   - `--video-input-device` は `--input-y4m` / `--sandstorm` / `--input-mp4` / `--no-video-device` と同時指定できません
   ```

3. 1 のエンコーダー実装指定の行の直後に、エラー文言が `--input-mp4 と --vp8-encoder 等のエンコーダー実装指定は同時に指定できません` のように `--vp8-encoder 等` の汎用表記になり、実際に指定したキー名は表示されないことを 1 文追記する
4. 実装側は変更しない (エラー文言・挙動は現状維持)。文言を実際のキー名に変えるのは本 issue の対象外

## 完了条件

- README の注意点に列挙した同時指定不可の組み合わせと、`parse_instance_args()` / `parse_args_from_argv()` が返す排他検証 (`〜 と同時に指定できません` のもの) が 1:1 で対応していること。1:1 の判定から除外するのは既存行のまま残る 3 件で、`--input-wav` 行の `--sora-audio=false` (実装文言は `指定できません`) と、`--http-host` / `--http-port` および `--client-cert` / `--client-key` の両方必須 (実装文言は `両方指定する必要があります`)。いずれも既存行を変更しないため残るだけで、新規の追記行に必須・依存系の検証を混ぜないこと
- 注意点に書いたキー名が、実際のエラー文言と実装側の検証条件と食い違っていないこと (`--vp8-encoder 等` の汎用表記の説明を含む)
- README 全体で矛盾した記述になっていないこと (既存の MP4 パススルーの節と `--openh264` の行を見比べる)
- README の記述スタイルが既存本文と揃っていること (半角括弧の前は全角スペース、オプション名はバッククォート)

## 変更対象

- `README.md` (「注意点」セクション)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間はブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり、実ブランチは切らない (`issues/closed/0047` と同じ運用)。
