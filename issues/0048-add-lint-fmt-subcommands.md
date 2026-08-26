# lint / fmt サブコマンドを追加する

- Created: 2026-08-26
- Completed: {YYYY-MM-DD}
- Branch: feature/add-lint-fmt-subcommands
- Polished: {YYYY-MM-DD}

## 目的

JSONC 設定ファイルに対する lint (妥当性検証) と fmt (整形) を、負荷試験起動とは独立したサブコマンドとして提供する。nginx の `-t` のように「起動せずに設定を確認する」用途と、設定ファイルの読みやすさを保つ整形を、CI や手元で同じ入口から使えるようにする。C++ 版 zakuro には無い機能だが、設定が複雑化した zakuro-rs では運用上必要。

## 現状

- CLI はフラグのみ (`src/args.rs` の `parse_args()`)。サブコマンドは無い
- `parse_args()` は `--help` / `--version` / `--show-video-codec-capability` を pre-parse で早期終了できるが、設定ファイル単体を検証・整形して終わる経路はない
- 引数・ファイル存在・排他・コーデックパラメータ等の検証は `parse_common_args` / `parse_instance_args` / `parse_args_from_argv` で行われる
- JSONC の読み込みは `load_jsonc_config` 経由で `--config` から行い、通常起動ではマージ後に負荷試験へ進む
- noargs はサブコマンドに対応している

## 設計方針

1. トップレベルにサブコマンド `lint` / `fmt` を追加する。通常の負荷試験起動 (フラグのみ) は現行どおりサブコマンドなしとする
2. 使い方は次のとおりとする (第一版は JSONC ファイル必須。CLI のみの設定は対象外)
   - `zakuro lint <FILE.jsonc>`
   - `zakuro fmt <FILE.jsonc>`
3. `lint`
   - 指定 JSONC を通常起動時と同じ規則でパース・検証する (必須欠落・未知キー・ファイル存在・排他・コーデックパラメータ等)
   - 成功時は exit 0、失敗時は通常起動時と同様のエラーで非 0
   - Sora 接続・VC 起動・HTTP / DuckDB は行わない
   - 起動前検証のうち OpenH264 ロードや PEM 読み込みまで含めるかは、通常起動と同じ失敗を手元で再現できる範囲に合わせる (実装時に `async_main` の起動前検証と揃える)
4. `fmt`
   - 指定 JSONC を整形して書き戻す (または標準出力。書き戻しを既定とし、必要なら `--stdout` 等は後続で検討)
   - コメント保持の可否は nojson の能力に合わせて決める。保持できない場合は issue 本文とヘルプに差分として明記する
5. `lint` と `fmt` は同一 issue で実装する (入口・パース基盤を共有するため)。ヘルプは `zakuro lint --help` / `zakuro fmt --help` およびトップレベルから辿れるようにする
6. `--check-config` フラグや nginx 風の短形 `-t` は採用しない

## 完了条件

- `zakuro lint <FILE.jsonc>` で、正しい設定は exit 0、不正な設定は非 0 になり、Sora に接続しない
- `zakuro fmt <FILE.jsonc>` で JSONC が整形される (書き戻しまたは合意した出力先)
- 通常の負荷試験起動 (`zakuro --config ...` や CLI フラグのみ) の挙動は変えない
