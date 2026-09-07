# Makefile から実体のない target を削除する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/remove-dead-makefile-targets
- Polished: {YYYY-MM-DD}

## 目的

Makefile は外部貢献者が最初に見る開発コマンドの一覧である。存在しないディレクトリ・パッケージ・feature を参照する target が並んでおり、`AGENTS.md` の「Don't live with broken windows」に反する。README は `make check` と `make test` を案内しているため、混在したままではどれが本物かは読み取れない。

## 現状

次の target と `.PHONY` の指定は、このリポジトリでは実行できないか、参照先が存在しない。

| 対象 | 問題 |
|---|---|
| `fuzzing` / `fuzzing-parallel` / `fuzzing-list` | `fuzz/` ディレクトリが存在しない。`cargo fuzz` は導入されていない (`Cargo.toml` に fuzz workspace の定義が無い)。`fuzzing-parallel` は `mkdir -p fuzz/logs` で作ろうとする。`.PHONY` には `fuzz` と `fuzzing-list` が並ぶが `fuzz:` target 自体は無い |
| `pbt-with-cover` | `cargo llvm-cov -p pbt --tests` を実行するが、`pbt` パッケージは存在しない (`Cargo.toml` は単一 package で `[workspace]` / `members` の定義が無い)。`.PHONY` に列挙されているのは `pbt` 側で target 名と一致しない |
| `sysroot-raspberry-pi` / `sysroot-ubuntu-24.04_arm64` / `sysroot-ubuntu-22.04_arm64` と `sysroot-clippy-*` / `sysroot-build-*` の一式 | `sysroot/` ディレクトリが存在しない。`cargo shiguredo-sysroot` を呼ぶが、これは組織内で配布される cargo サブコマンドでリポジトリ側に定義も説明も無い。中間成果物を `git checkout -- .cargo/config.toml` で戻す社内前提の手順を含む |
| `sysroot-clippy-raspberry-pi` ほか | `--features raspberrypi` を渡すが、`Cargo.toml` の `[features]` は `fdk-aac` のみで `raspberrypi` は未定義 |
| `cover` / `pbt-with-cover` | `cargo llvm-cov` を前提とするが、README の必要環境は Rust stable と rustfmt / clippy のみで導入案内が無い |

`Makefile` の `.PHONY` には `test cover pbt pbt-cover fuzz fuzzing fuzzing-parallel fuzzing-list check clippy fmt clean` が並び、うち `pbt` と `fuzz` と `pbt-cover` は target 実体がない名前の列挙になっている。

## 設計方針

1. 実行できない target を削除する。削除とは別に PBT と Fuzzing の基盤自体が欲しい場合は `issues/0071` / `issues/0072` / `issues/0073` が別途扱うため、本 issue は「動かない物を並べたままにしない」までとする
2. `sysroot-*` 一式は、ラズベリーパイ向けクロスビルドをこのリポジトリで支援する意思があるかを確認してから扱う。意思が無いなら削除し、上流のクロスビルド手順を `docs/` か README で案内する (社内ツール `cargo shiguredo-sysroot` への依存を公開リポジトリに残さない)
3. `.PHONY` のリストを実在 target と一致させ、名前だけの項目を消す
4. README が案内するコマンド (`make check`、`make test`) と Makefile 実体の整合は本 issue の対象外 (`issues/0076` で扱う)。削除作業がこれらの target を巻き込まないことを確認する
5. `cargo llvm-cov` に依存する target を残す場合、その前提ツールを README の必要環境へ追記する (`issues/0068` と重複しないよう、残す範囲だけ本 issue で決める)

## 完了条件

- `Makefile` に、存在しないディレクトリ・パッケージ・feature・社内ツールを参照する target が残っていないこと
- `.PHONY` に列挙された名前がすべて実在する target と一致すること
- 残した target だけを順に実行して失敗しないこと (実行結果を本 issue の「## 解決方法」に記録する)
- README が案内するコマンドが消えていないこと (削除した場合は `issues/0068` 側の修正としてではなく、本 issue で整合を取った旨を書く)

## 変更対象

- `Makefile`
- 削除方針に伴って参照が外れる場合の `docs/` または `README.md` の該当記述 (最小限)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
