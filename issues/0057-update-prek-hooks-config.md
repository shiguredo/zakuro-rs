# prek のフック設定をリポジトリの実態と `shiguredo-rust` の要求に合わせる

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/update-prek-hooks-config
- Polished: {YYYY-MM-DD}

## 目的

`prek.toml` のフック設定がリポジトリの実態と `shiguredo-rust` の要求からずれている。発火しないフックを置き、`cargo test` を `pre-push` で走らせ、テスト実行を機械的に担保できるようにする。

## 現状

- `prek.toml` のフックは cargo-fmt / ruff-format / cargo-clippy / ruff-check / ty-check の 5 件のみ
- `ruff-format` / `ruff-check` / `ty-check` は `files = '^e2e-tests/.*\\.py$'` を対象にする設定だが、リポジトリに `e2e-tests/` ディレクトリも Python ファイルも存在しないため、これらは一度も発火しない
- `shiguredo-rust` は「Git フックは prek で管理し、最低限 `cargo fmt` / `cargo clippy` / `cargo test` と tombi の lint / format をフックすること」「`cargo test` は `pre-push` ステージだけで実行すること」を要求するが、`cargo test` のフックが無い。テスト実行は完全な手動依存であり、`shiguredo-git` の「全てのテストが通らない限りコミットしない」を機械的に担保できていない
- `tombi` の lint / format フックも無い (`Cargo.toml` / `prek.toml` / `.cargo/config.toml` と TOML を扱うリポジトリである)
- 現在インストールされているのは `.git/hooks/pre-commit` のみで、`pre-push` は未設置である
- `prek.toml` の先頭コメント 2 行は英語で、`AGENTS.md` の「コメントは全て日本語にすること」に抵触する

## 設計方針

1. 発火しない Python 向けフック 3 件と、そのために置かれている ruff / ty 設定を削除する (`e2e-tests/` を追加するときは別 issue で復活させる)
2. `prek.toml` に `cargo test` を `pre-push` ステージで実行するフックを追加する。entry は CI と同じ `cargo test --workspace` とし、`pass_filenames = false` を付ける。あわせて `default_install_hook_types` を指定して `prek install --prepare-hooks` で `pre-commit` と `pre-push` の両方が入るようにする
3. tombi の lint / format フックを追加する。対象は TOML ファイルとし、自動生成ファイルである `Cargo.lock` は除外する。`shiguredo-rust` スキル同梱の参考設定 (`skills/shiguredo-rust/prek.toml`) に倣い、外部 repo は rev をタグで固定して追加する
4. 先頭コメントを日本語化する。`#:schema` 行は機能設定なので残す
5. 設定変更後は `prek validate-config` と `prek run --all-files` で確認する

## 完了条件

- `prek validate-config` と `prek run --all-files` が通過すること
- `.git/hooks` に `pre-commit` と `pre-push` の両方が入り、push 時に `cargo test --workspace` が走ることを実測で確認できること
- 存在しないファイル種別を指すフックが残っていないこと
- `prek.toml` のコメントがすべて日本語になっていること

## 変更対象

- `prek.toml`
- `.git/hooks` (`prek install --prepare-hooks` の実行結果。リポジトリにはコミットしない)
