# fmt サブコマンドを追加する

- Created: 2026-08-26
- Completed: 2026-08-26
- Branch: feature/add-fmt-subcommand
- Polished: {YYYY-MM-DD}

## 目的

JSONC 設定ファイルを、負荷試験起動とは独立した `zakuro fmt` サブコマンドで整形できるようにする。設定ファイルの読みやすさを保ちつつ、コメントや trailing comma を落とさずに CI や手元から同じ入口で使えるようにする。`zakuro lint` (0048) と入口・パース基盤を共有する。

## 現状

- `zakuro lint <FILE.jsonc>` は実装済み (`src/cmd_lint.rs`, `src/diagnostic.rs`)
- `fmt` サブコマンドは無い
- nojson は JSONC パースとコメント位置 (`RawJson::parse_jsonc` の comment ranges) を提供する
- `DisplayJson` / `JsonFormatter` 単体での再出力はコメントを保持しない
- nojson 作者の `jcfmt` は comment ranges を辿って空白だけを整える JSONC フォーマッタ (CLI)。inline / trailing コメント・空行・trailing comma を保持する。現状はライブラリ API ではない

## 設計方針

1. トップレベルにサブコマンド `fmt` を追加する。通常の負荷試験起動 (フラグのみ) は現行どおりサブコマンドなしとする
2. 使い方は `zakuro fmt <FILE.jsonc>` (第一版は JSONC ファイル必須。CLI のみの設定は対象外)
3. 指定 JSONC を整形して書き戻す (書き戻しを既定とし、必要なら `--stdout` 等は後続で検討)
4. **コメント保持は必須**。`//` / `/* */` (inline / trailing 含む)、空行、trailing comma を落とさない
5. `DisplayJson` / `JsonFormatter` だけで値を再生成する方式は採用しない (コメントが消えるため)
6. 実現は `RawJson::parse_jsonc` の comment ranges を使い、`jcfmt` 相当の再出力を zakuro 内に実装する (`jcfmt` 外部プロセス起動や依存追加は第一版ではしない。ロジック移植で足りる)
7. 構文エラー時の診断出力は `zakuro lint` と同系統の `annotate-snippets` を使う (`src/diagnostic.rs` を再利用)
8. 変更が無いときはファイルを書き換えず exit 0 (無出力)

## 完了条件

- `zakuro fmt <FILE.jsonc>` で JSONC が整形され、整形前後でコメント・空行・trailing comma が保持される
- 構文エラー時は非 0 で、lint と同様の診断形式が stderr に出る
- 通常の負荷試験起動 (`zakuro --config ...` や CLI フラグのみ) の挙動は変えない

## 解決方法

- `zakuro fmt <FILE.jsonc>` サブコマンドを追加 (`src/cmd_fmt.rs`、`src/main.rs` の pre-parse 経路に登録)
- 整形処理は `src/jsonc_fmt.rs` に `RawJson::parse_jsonc` の comment ranges を辿る実装を追加し、`jcfmt` 相当を zakuro 内へ移植 (外部プロセス起動・依存追加なし)
- `//` / `/* */` (inline / trailing)・空行・trailing comma を保持したまま 2 スペースインデントへ正規化する
- 構文エラー時は `src/diagnostic.rs` の `annotate-snippets` 形式で stderr に診断を出して exit 1
- 整形前後で変更が無いときはファイルを書き換えず exit 0 (無出力)
