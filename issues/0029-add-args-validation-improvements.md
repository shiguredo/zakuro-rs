# 引数パースのバリデーション不足と境界値問題を修正する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/add-args-validation-improvements
- Polished: 2026-00-00

## 目的

`src/args.rs` の引数パース・JSONC 設定ファイル処理に存在するバリデーション不足と境界値問題を修正する。

## 優先度根拠

排他チェック漏れや空値のサイレント通過は、ユーザーが意図しない状態で zakuro が起動し、実行時エラーや無意味な動作に繋がる。使用頻度の高いオプションのバリデーションほど影響が大きい。

## 現状

### 1. signaling-url 空配列で空文字列が生成される

`src/args.rs:217-241`: JSONC の `signaling-url` が空配列 `[]` の場合、空文字列が `--sora-signaling-url` の値になり、有効な URL 無しで起動する。

### 2. split_cli_argv が次トークンを無条件に値として消費する

`src/args.rs:449-457`: `--vcs --sandstorm` のような指定で、`--sandstorm` が vcs の値として消費され消失する。

### 3. no_video_device と映像ソース系の排他チェック欠落

`src/args.rs:1012-1047`: `no_video_device` と `video_input_device` / `input_y4m` / `sandstorm` / `input_mp4` の排他チェックがない。

### 4. JSONC の boolean false がテンプレートの true を上書きできない

`src/args.rs:164-173`: テンプレートに `"sandstorm": true` がある場合、インスタンスで `"sandstorm": false` を指定しても何も生成されない。

### 5. `${...}` 環境変数置換がサイレントドロップされる

`src/args.rs:293-300`: 警告ログは出るがキー全体が無視されユーザーが気づきにくい。

## 設計方針

1. signaling-url 空配列をエラーにする
2. 次トークンが `--` 始まりの場合はエラーにする
3. no_video_device の排他チェックを追加する
4. JSONC の boolean false に対して警告ログを出す（上書き不可であることを通知する）
5. `${...}` を含む値はエラーで起動を拒否する

## 完了条件

- 上記 5 件すべてのバリデーションが追加されていること
- 既存の args.rs のテストが通過すること
- 追加したバリデーションに対応するテストが追加されていること

## 解決方法

`src/args.rs` の該当箇所にバリデーションを追加する。テストも合わせて追加する。
