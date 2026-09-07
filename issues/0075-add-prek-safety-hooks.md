# prek に builtin と meta の安全系フックを追加する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-prek-safety-hooks
- Polished: {YYYY-MM-DD}

## 目的

公開リポジトリでは、コミット段階で機械的な安全チェックが走ることを期待する。とくに秘密鍵の混入と大容量バイナリの誤コミットは、一度 history に入ると除去が高コストになる (`issues/0061` が履歴書き換えを強いられているのがまさにこれ)。`shiguredo/hisui` は既にこれらのフックを導入している。

## 現状

- zakuro-rs の `prek.toml` は `repo = "local"` の 1 リポジトリ構成で、フックは cargo-fmt / ruff-format / cargo-clippy / ruff-check / ty-check の 5 件のみである
- `repo = "builtin"` (prek 同梱の組み込みフック) と `repo = "meta"` を使っていない
- 参考: `shiguredo/hisui` の `prek.toml` は次の組み込みフックを有効化している
  - `check-added-large-files` (大容量ファイルの混入防止)
  - `detect-private-key` (秘密鍵の混入防止)
  - `check-symlinks` と `destroyed-symlinks` (シンボリックリンクの健全性)
  - `trailing-whitespace`、`end-of-file-fixer`、`fix-byte-order-marker`、`mixed-line-ending`
  - `check-toml`、`check-yaml`、`check-json`
  - `check-merge-conflict`、`check-case-conflict`、`check-executables-have-shebangs`、`check-shebang-scripts-are-executable`
  - `repo = "meta"` の `check-hooks-apply`
- zakuro-rs で実際に有効性がある候補
  - `detect-private-key`: mTLS 用の証明書・秘密鍵を手元で扱うツールであり、テストやサンプルとして紛れ込むリスクが構造的にある。いま `resource/` は `.DS_Store` だけの残骸だが、`gitignore` に守られるのとフックで検出するのは次元が違う
  - `check-added-large-files`: `testdata/` に MP4 / M4A を置き、ビルドで `target/duckdb-download/` に 100MB 級の共有ライブラリが落ちる。`target/` は gitignore 済みだが、`git add -f` 事故を防げない
  - `check-symlinks` / `destroyed-symlinks`: `CLAUDE.md` が `AGENTS.md` へのシンボリックリンクであり、`AGENTS.md` を消した場合に壊れたリンクとして残る (実測で `git ls-files -s CLAUDE.md` の mode が `120000`)
  - `check-json` / `check-yaml` / `check-toml`: `.markdownlint.jsonc` (JSON with comments)、`.github/workflows/ci.yml`、`Cargo.toml` / `prek.toml` / `rust-toolchain.toml` / `.cargo/config.toml` が追跡されている
  - `check-case-conflict`: `.gitignore` に守られた `.DS_Store` と `resource/.DS_Store` が実在し、大文字小文字だけ違う名前の混入を早期に検出できる
- `issues/0057` が tombi と `cargo test` のフック追加を扱うが、「`fail_fast` / `default_stages` / トップレベル `exclude` / `repo = "builtin"` の一組は今回追加しない (追加するなら別 issue で)」として builtin 一式を明示的に先送りしている。本 issue がその先送り分を扱う
- なお `shiguredo-rust` の「Git フックは prek で管理し、最低限 `cargo fmt` / `cargo clippy` / `cargo test` と tombi の lint / format をフックすること」の最低要求には組み込みフックは含まれていない。本 issue は最低要求の充足ではなく、公開リポジトリとしての安全策である

## 設計方針

1. `issues/0057` の完了を前提にする (0057 で発火しない Python フックが削除され、`stages` の扱いと tombi が決まる)。0057 完了後に着手する
2. `repo = "builtin"` と `repo = "meta"` を `hisui` の書き方に揃えて追加する。rev 固定の要否と `language` の指定は `hisui` に従う
3. zakuro-rs で実効性の無い組み込みフックは無理に揃えない。判断を要するのは `check-json` で、`.markdownlint.jsonc` は JSON with comments であり素の JSON パーサで弾かれる可能性がある。この 1 件は追加前に発火を確認し、通らないなら対象外にして理由をコメントに書く (コメントは日本語)
4. `check-added-large-files` の上限は `hisui` の設定値を確認して揃え、`testdata/` の実ファイルを通過させる (現状の testdata 最大サイズを実測してから決める)
5. `check-executables-have-shebangs` と `check-shebang-scripts-are-executable` は、`canary.py` が該当する。実行 bit と shebang の現状を実測してから追加可否を決める
6. `prek.toml` の既存フックに対する `stages` の扱いは `issues/0057` の決定に従う。本 issue で `default_stages` を追加しない (0057 がトップレベル設定の追加を見送っているため整合させる)
7. フック追加後は `prek validate-config`、`prek run --all-files`、`prek run --all-files --stage pre-push` を実行し、既存ファイルが大量に書き換えられないことを確認する。書き換えが出る場合は対象を絞るか、整形だけ別コミットに分ける

## 完了条件

- `prek.toml` に `repo = "builtin"` と `repo = "meta"` のフックが追加されていること
- `detect-private-key` と `check-added-large-files` が有効になっていること (公開リポジトリで最重要の 2 つ)
- `prek validate-config` と `prek run --all-files` が通過すること
- 追加したフックのうち、 zakuro-rs で一度も発火しえない対象を指すものが無いこと (発火しない物は `issues/0057` が除去した対象と同じ失敗なので繰り返さない)
- 既存ファイルの書き換えが発生していないこと、または発生した場合はその理由と対象が本 issue に記録されていること

## 変更対象

- `prek.toml`
- `.git/hooks` (`prek install --prepare-hooks` の実行結果。リポジトリにはコミットしない)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
