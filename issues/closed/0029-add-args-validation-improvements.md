# 引数パースのバリデーション不足と境界値問題を修正する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-06-28
- Model: DeepSeek V4 Pro
- Branch: feature/add-args-validation-improvements
- Polished: 2026-06-28

## 目的

`src/args.rs` の引数パース・JSONC 設定ファイル処理に存在するバリデーション不足と境界値問題を修正する。本 issue は 5 件の独立した修正を含む複合 issue であり、同一ファイルに対する変更のため 1 ブランチで対応する。

## 優先度根拠

排他チェック漏れや空値のサイレント通過は、ユーザーが意図しない状態で zakuro が起動し、実行時エラーや無意味な動作に繋がる。使用頻度の高いオプションのバリデーションほど影響が大きい。

## 現状

### 1. signaling-url 空配列で空文字列が生成される

`flatten_sora_object` (`src/args.rs:215-241`): JSONC の `signaling-url` が空配列 `[]` の場合、`joined` は空文字列 `""` になり、`--sora-signaling-url ""` が生成される。さらに `parse_instance_args` (`:714-718`) で `"".split(',')` → `[""]` となり、有効な URL 無しで起動する。

### 2. split_cli_argv が次トークンを無条件に値として消費する

`split_cli_argv` (`:449-457`): 非フラグキーに続くトークンが `--` 始まりかをチェックせず、常に値として消費する。`--vcs --sandstorm` のような指定で `--sandstorm` が vcs の値として消費され消失する。

### 3. no_video_device と映像ソース系の排他チェック欠落

`parse_instance_args` (`:1021-1047`): `no_video_device` と `video_input_device` / `input_y4m` / `sandstorm` / `input_mp4` の排他チェックがない。全 4 種の映像ソースとの排他が漏れている。

### 4. JSONC の boolean false がテンプレートの true を上書きできない

`push_kv` (`:164-173`): boolean `false` は何も出力しない。テンプレートに `"sandstorm": true` がある場合、インスタンスで `"sandstorm": false` を指定しても CLI 引数が全く生成されず、`dedupe_argv_last_wins` (`:468-522`) に上書きの機会が与えられない。

### 5. `${...}` 環境変数置換がサイレントドロップされる

`resolve_env_in_string` (`:293-300`): 警告ログは出るがキー全体が `continue` で無視される。必須キー (`sora-signaling-url` 等) が消失し、ユーザーが原因に気づきにくい。

## 設計方針

| # | 問題 | 修正方針 |
|---|------|---------|
| 1 | 空配列で空 URL | signaling-url が空配列または空文字列要素を含む場合、エラーで起動を拒否する |
| 2 | 次トークンを無条件消費 | 非フラグキーの直後のトークンが `--` で始まる場合はエラーにする |
| 3 | no_video_device 排他漏れ | `no_video_device` と `video_input_device` / `input_y4m` / `sandstorm` / `input_mp4` の排他チェックを追加する |
| 4 | boolean false が上書き不可 | `push_kv` と `dedupe_argv_last_wins` の挙動は変更せず、テンプレート `true` が残った場合に警告ログを出力する |
| 5 | `${...}` がサイレントドロップ | `${...}` を含む値はエラーで起動を拒否する |

## 完了条件

- 上記 5 件すべてのバリデーションが追加されていること
- 既存の args.rs のテストが通過すること
- 新規に以下のテストを追加すること（テストのログメッセージは日本語にすること）:
  - 空配列/空文字列要素の signaling-url がエラーになることのテスト
  - `--vcs --sandstorm` がエラーになることのテスト
  - `no_video_device` と各映像ソースの排他エラーのテスト
  - テンプレート `true` + インスタンス `false` で警告ログが出ることのテスト
  - `${...}` がエラーになることのテスト

## 解決方法

5 件の修正を実施した。

### 1. signaling-url 空配列の拒否
`flatten_sora_object` で signaling-url 配列が空の場合にエラーを返す。

### 2. split_cli_argv の次トークン検証
非フラグキーの直後のトークンが `--` で始まる場合にエラーを返す。戻り値を `Result` に変更した。

### 3. no_video_device 排他チェック
`no_video_device` と `video_input_device` / `input_y4m` / `sandstorm` / `input_mp4` の排他チェックを 4 件追加した。

### 4. boolean false の警告ログ
`push_kv` で boolean `false` の場合に警告ログを出力する。

### 5. ${...} 環境変数置換のエラー化
`parse_jsonc_config`、`expand_instances`、`push_kv`、`flatten_sora_object` で `${...}` をエラーとして起動を拒否する。

### テスト追加
- `empty_signaling_url_array_is_rejected`
- `vcs_followed_by_flag_is_rejected`
- `no_video_device_excludes_all_sources`
- `sandstorm_false_warns_about_template_override`
- `env_var_substitution_is_rejected`

### 変更ファイル
- `src/args.rs`
