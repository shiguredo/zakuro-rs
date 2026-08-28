# prek のフック設定をリポジトリの実態と `shiguredo-rust` の要求に合わせる

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/update-prek-hooks-config
- Polished: 2026-08-28

## 目的

`prek.toml` のフック設定がリポジトリの実態と `shiguredo-rust` の要求からずれている。発火しないフックを置き、`cargo test` を `pre-push` で走らせ、テスト実行を機械的に担保できるようにする。

## 現状

- `prek.toml` のフックは cargo-fmt / ruff-format / cargo-clippy / ruff-check / ty-check の 5 件のみ
- `ruff-format` / `ruff-check` / `ty-check` は `files = '^e2e-tests/.*\.py$'` を対象にする設定だが、リポジトリに `e2e-tests/` ディレクトリは存在しない。Python ファイルはルート直下の `canary.py` (Cargo.toml のバージョンを canary 版へバンプし、`cargo update` と git commit / tag / push まで行うリリース補助スクリプト) が 1 件あるのみで、この正規表現に一致しないため 3 件とも一度も発火しない
- `shiguredo-rust` は「Git フックは prek で管理し、最低限 `cargo fmt` / `cargo clippy` / `cargo test` と tombi の lint / format をフックすること」「`cargo test` は `pre-push` ステージだけで実行すること」を要求するが、`cargo test` のフックが無い。テスト実行は手動 (と push 後の CI) 依存であり、`shiguredo-git` の「全てのテストが通らない限りコミットしない」を機械的に担保できていない
- `tombi` の lint / format フックも無い。追跡されている TOML は `Cargo.toml` / `prek.toml` / `rust-toolchain.toml` / `.cargo/config.toml` / `Cargo.lock` の 5 件である (`tombi 1.2.0` で `tombi lint` と `tombi fmt --check` を 4 件に実行した範囲ではすべて通過しており、フック追加に伴って既存 TOML を整形する必要は無い)
- 現在インストールされているのは `.git/hooks/pre-commit` のみで、`pre-push` は未設置である
- `prek.toml` の先頭コメント 2 行は英語で、`AGENTS.md` の「コメントは全て日本語にすること」に抵触する
- 既存の cargo-fmt / cargo-clippy は `stages` を指定していない。prek は `stages` 未指定のフックを全ステージの対象 (`Default: all stages`) とするため、`pre-push` の shim が入ると cargo fmt / cargo clippy が push 時も走るようになる

## 設計方針

1. 発火しない Python 向けフック 3 件と、そのために置かれている ruff / ty 設定を削除する (`e2e-tests/` を追加するときは別 issue で復活させる)。`canary.py` は 1 本のリリース補助スクリプトであり、`shiguredo-rust` のフック最低要求に Python 系は無いため、今回の追加で見送る
2. `prek.toml` に `cargo test` を `pre-push` で実行するフックを追加する。entry は CI と同じ `cargo test --workspace` とし、`stages = ["pre-push"]` と `pass_filenames = false` を付ける。`stages` 未指定のフックは prek では全ステージ対象 (`Default: all stages`) なので、`stages` を付けないと「`cargo test` は `pre-push` ステージだけで実行する」に反して commit 時も走る。同じ理由で既存の cargo-fmt / cargo-clippy にも `stages = ["pre-commit"]` を明示する (参考設定のように `default_stages = ["pre-commit"]` を足す手もあるが、方針 5 のとおりトップレベル設定の追加は今回見送るためフック側で書く)。参考設定の cargo-test は `types = ["rust"]` だが、これでは push に Rust の変更が含まれないと発火せず「テスト実行を機械的に担保する」という目的に合わないので、`always_run = true` を付けて常に走らせる
3. `prek install` がインストールする shim を決めるのは `default_install_hook_types` であり、どのステージでどのフックが走るかとは別設定である。`default_install_hook_types = ["pre-commit", "pre-push"]` を追加して `prek install --prepare-hooks` で両方の shim が入るようにする
4. tombi の lint / format フックを追加する。対象は TOML ファイルとし、自動生成ファイルである `Cargo.lock` は除外する。書き方は `shiguredo-rust` スキル同梱の参考設定 (`skills/shiguredo-rust/prek.toml`) の tombi 節に倣い、外部 repo は rev をタグで固定して追加する (参考設定は `tombi-toml/tombi-pre-commit` の `v1.2.0` で、upstream にはより新しいタグがあるので導入時に最新版のタグへ更新し、その版で 4 件の TOML を通し直す)。参考設定の tombi フックは `stages` 未指定なので、方針 2 と同じ理由で `stages = ["pre-commit"]` を明示する (`default_stages` を入れないため)
5. 参考設定を倣う範囲は tombi 節の書き方と `default_install_hook_types` に限定する。`fail_fast` / `default_stages` / トップレベル `exclude` / `repo = "builtin"` の一組は今回追加しない (追加するなら別 issue で、変更範囲とその影響を切り分ける)
6. 先頭コメントを日本語化する。`#:schema` 行は機能設定なので残す
7. 設定変更後は `prek validate-config`、`prek run --all-files` (既定の pre-commit ステージ) と `prek run --all-files --stage pre-push` で確認する

## 完了条件

- `prek validate-config` / `prek run --all-files` / `prek run --all-files --stage pre-push` が通過すること
- `.git/hooks` に `pre-commit` と `pre-push` の両方が入り、push 時に `cargo test --workspace` が走ることを実測で確認できること (`prek list` 等で cargo-test が pre-push 専用になっていることも確認する)
- 存在しないファイル種別を指すフックが残っていないこと
- `cargo test` が pre-commit では走らず、`pre-push` でのみ走ること。逆に cargo-fmt / cargo-clippy / tombi が `pre-push` で走らないこと (`prek run --all-files --stage pre-push --dry-run` 等で発火するフックを確認する)
- 新規フックによる既存 TOML ファイルの書き換えが発生していないこと。tombi の追加で `prek run --all-files` が失敗する場合は、導入する版で既存 4 件を通し直して通ることを確認してから導入する (大掛かりな整形が必要なら tombi フック導入だけ別 issue へ切り出す)
- `prek.toml` のコメントがすべて日本語になっていること

## 変更対象

- `prek.toml`
- `.git/hooks` (`prek install --prepare-hooks` の実行結果。リポジトリにはコミットしない)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間はブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり、実ブランチは切らない (`issues/closed/0047` と同じ運用)。
