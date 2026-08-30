# lint サブコマンドを追加する

- Created: 2026-08-26
- Completed: 2026-08-26
- Branch: feature/add-lint-subcommand
- Polished: {YYYY-MM-DD}

## 目的

JSONC 設定ファイルの妥当性を、負荷試験起動とは独立した `zakuro lint` サブコマンドで検証できるようにする。nginx の `-t` のように「起動せずに設定を確認する」用途を CI や手元で使えるようにする。C++ 版 zakuro には無い機能だが、設定が複雑化した zakuro-rs では運用上必要。

## 現状

- CLI はフラグのみ (`src/args.rs` の `parse_args()`)。サブコマンドは無い
- `parse_args()` は `--help` / `--version` / `--show-video-codec-capability` を pre-parse で早期終了できるが、設定ファイル単体を検証して終わる経路はない
- 引数・排他・コーデックパラメータ等の検証は `parse_common_args` / `parse_instance_args` / `parse_args_from_argv` で行われる
- JSONC の読み込みは `load_jsonc_config` 経由で `--config` から行い、通常起動ではマージ後に負荷試験へ進む (`RawJson::parse_jsonc`)
- noargs はサブコマンドに対応している

## 設計方針

1. トップレベルにサブコマンド `lint` を追加する。通常の負荷試験起動 (フラグのみ) は現行どおりサブコマンドなしとする
2. 使い方は `zakuro lint <FILE.jsonc>` (第一版は JSONC ファイル必須。CLI のみの設定は対象外)
3. 指定 JSONC を通常起動時と同じ規則でパース・検証する (必須欠落・未知キー・排他・コーデックパラメータ等)
4. 構文は `RawJson::parse_jsonc`、意味検証は既存の `parse_jsonc_config` / `parse_args_from_argv` 経路を再利用する
5. 成功時は exit 0、失敗時は非 0。Sora 接続・VC 起動・HTTP / DuckDB は行わない
6. 起動前検証のうち OpenH264 ロードや PEM 読み込みは第一版では含めない (通常起動と同じ失敗を手元で再現する範囲に合わせる)
7. 診断出力は `annotate-snippets` を使う
8. `--check-config` フラグや nginx 風の短形 `-t` は採用しない

## 完了条件

- `zakuro lint <FILE.jsonc>` で、正しい設定は exit 0、不正な設定は非 0 になり、Sora に接続しない
- 通常の負荷試験起動 (`zakuro --config ...` や CLI フラグのみ) の挙動は変えない

## 解決方法

- `zakuro lint <FILE.jsonc>` サブコマンドを追加 (`src/cmd_lint.rs`)
- 構文エラーは `RawJson::parse_jsonc` の位置情報付きで報告する
- 意味検証は `validate_jsonc_config_str` 経由で `parse_jsonc_config` / `parse_args_from_argv` を再利用する
- 診断出力は `annotate-snippets` を使う (`src/diagnostic.rs`)
- 成功時は無出力 exit 0、失敗時は stderr に診断を出して exit 1
