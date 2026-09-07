# Makefile と prek と CI のチェックコマンドを統一する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/update-check-command-parity
- Polished: {YYYY-MM-DD}

## 目的

README は開発者に `make check` と `make test` を案内しているが、CI が実際に通すコマンドは Makefile のどれとも一致していない。外部貢献者は「ローカルでは通ったのに CI で落ちる」を繰り返し、維持側は再現の手順を説明できなくなる。ローカル・フック・CI の 3 系統を 1 つの入口に揃える。

## 現状

同じ「整形確認」「clippy」「テスト」に対して、3 系統が別々のコマンドを実行している。

| 観点 | Makefile | prek.toml | ci.yml |
|---|---|---|---|
| fmt | `make fmt` は `cargo fmt --all` (**書き込み実行**。`--check` を使う確認専用 target は存在しない) | `cargo fmt --all -- --check` | `cargo fmt --all --check` |
| clippy | `make clippy` は `cargo clippy --workspace -- -D warnings` (`--all-targets` なし、`--features` なし) | `cargo clippy --workspace --all-targets -- -D warnings` (`--features` なし) | `cargo clippy --workspace --features fdk-aac -- -D warnings` (`--all-targets` なし) |
| test | `make test` は `cargo test --workspace` (`--features` なし) | (cargo test のフックが無い。`issues/0057` が対象) | `cargo test --workspace --features fdk-aac` |
| check | `make check` は `cargo check --workspace` | 該当なし | 該当なし |

結果として生じている具体的な空白:

1. `--features fdk-aac` は CI でしか検証されない。Linux で AAC 機能を有効にした状態でビルド・テストする経路がローカルに存在しない
2. `--all-targets` によるテストコードの clippy 検証は prek でしか走らない。CI は `--all-targets` を付けないため、テストコード内の警告が CI を通る
3. README の案内する `make check` は型チェックのみで、fmt 確認も clippy も含まれない。`make fmt` は名前に反してファイルを書き換えるため、「確認」の意図で実行すると意図せず差分が出る
4. CI は `make` を一切呼ばない (`ci.yml` に make の記載が無い)。Makefile が壊れても CI は緑のまま検出できない
5. fmt 確認の entry も 3 系統で文字列が違う (`cargo fmt --all --check` と `cargo fmt --all -- --check`)。結果は同じだが、比較して整合を確認できない形になっている

## 設計方針

1. Makefile に CI と同じ内容を実行する単一入口を追加する (例: `make ci`)。`ci` は fmt 確認・clippy・test を CI と同一のフラグ付きで順に実行する。CI 側はステップを `make ci` 1 つに畳むか、CI から呼ぶコマンドを Makefile 経由に統一する。**CI が Makefile を呼ぶ形にすれば、コマンドの二重定義が起きない**
2. 既存の `make fmt` は書き込み実行として残してよいが、確認専用 target (`make fmt-check` など) を別名で追加し、README から「確認」に書き込み系を案内しないようにする
3. clippy は `--workspace --all-targets --features fdk-aac -- -D warnings` に統一する方向で検討する。`fdk-aac` feature は Linux 限定の依存を含む (`Cargo.toml` の target 節で `cfg(target_os = "linux")` に紐づいている) ため、macOS ランナーで同じフラグを付けたときに解決されるかを確認してから決める。解決できないなら matrix ごとに feature を切り替える設計にし、Makefile 側に素の target を残す
4. test も `--features fdk-aac` を CI と同じ入口で実行できるようにするが、3 と同じ OS 依存の判断を先に済ませる
5. `prek.toml` のフック entry は `issues/0057` の決定に従い、本 issue でフック定義を増やさない。Makefile が用意する単一入口と同じコマンド文字列になることだけを確認する
6. README の「確認用コマンド」を、採用した入口 1 つに絞って書き直す (`issues/0068` とは書き換える行が重なるため、着手順をそろえる)
7. fmt 確認の entry 文字列はどちらかに統一し、3 系統で一致させる

## 完了条件

- Makefile に CI と同じチェックを実行する単一入口があり、その名前で README が案内していること
- fmt 確認 / clippy / test のコマンド文字列が Makefile ・prek・CI の 3 系統で一致していること (差分表を本 issue に残す)
- `--all-targets` によるテストコードの clippy 検証が CI でも走るようになっていること
- `--features fdk-aac` の検証経路が CI 専有でなくなるか、CI 専有と判断した場合はその理由が Makefile か README にコメントされていること
- CI が Makefile を経由する構成にした場合、Makefile の target を 1 つ壊したときに CI が赤くなること (意図的に壊して確認し、元に戻す)

## 変更対象

- `Makefile`
- `.github/workflows/ci.yml`
- `README.md` の「確認用コマンド」

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
