# ツールチェーンを固定し MSRV を検証する job を追加する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/update-pinned-toolchain-msrv
- Polished: {YYYY-MM-DD}

## 目的

`Cargo.toml` は `rust-version` で MSRV を宣言しているが、その値を実際に検証する仕組みが無い。宣言が実態とズレたまま公開すると、外部ユーザーは「動くはずのバージョン」でビルドに失敗する。

## 現状

- `rust-toolchain.toml` は `channel = "stable"` の浮動参照で、正確なバージョンを固定していない。`targets` は `aarch64-unknown-linux-gnu` と `x86_64-unknown-linux-gnu` の 2 つのみで、macOS / Windows の交叉ビルドには `rustup target add` が必要になる
- `Cargo.toml` の `[package]` は `rust-version = "1.97"`、`edition = "2024"`
- `.github/workflows/ci.yml` は `rustup update stable` を走らせるだけで、`1.97` 相当のツールチェーンでビルドできることの検証をしていない。つまり CI が通っても MSRV は保証されない
- 主要な依存の MSRV と本クレートの宣言の関係を説明した文書が無い。例えば上流の `sora_sdk` は `rust-version = "1.93"` を宣言しており、`1.97` より低い値が依存側に要求されていることがリポジトリ内から読み取れない
- `shiguredo-rust` の規約は MSRV を 1.93 とし `Cargo.toml` の `rust-version` に明記することを求めている。本リポジトリは 1.97 を書いており、規約との関係 (引き上げの可否・理由) が記録されていない
- 参考: `shiguredo/hisui` の `ci.yml` も `rustup update stable` を使う浮動運用で、ツールチェーン固定は組織慣習ではない

## 設計方針

1. まず `rust-version = "1.97"` が正しい値なのかを実測で確定させる。`cargo +1.97 build` が通るか、より低い版で通るかを確認し、MSRV を引き下げられるなら引き下げる (下がる場合は `shiguredo-rust` 規約の 1.93 に近づける)。引き上げが必要ならその理由を `Cargo.toml` にコメントで残す
2. CI に MSRV job を 1 つ追加する。`rust-toolchain.toml` の `channel` を浮動に置いたまま、job 内で `rustup toolchain install <正確なバージョン>` を使い分ける形にする (ワークスペース全体の channel を固定すると開発者の作業環境に直接影響するため、CI と開発環境で方針を分ける)
3. `rust-toolchain.toml` の `targets` は配布対象マトリクスと整合させる。配布で macOS arm64 や Windows を対象にするなら、必要となるターゲットを追加する (CI の交叉ビルドで `--target` を使う場合は `targets` に記載しないと `std` が無いエラーになる)
4. MSRV と依存の要求バージョンの関係を README の必要環境に書く (文書化自体は本 issue の完了条件に含める)

## 完了条件

- `Cargo.toml` の `rust-version` が実測で検証された値になっていること (通ることを確認した最小バージョンであること)
- CI に、そのバージョンのツールチェーンでビルドを確認する job が追加され、失敗を止められること
- `rust-toolchain.toml` の `targets` と、配布・CI で使うターゲットの必要が一致していること
- MSRV の根拠 (なぜその値か、上流依存の要求いくつか) がリポジトリ内から読み取れること

## 変更対象

- `.github/workflows/ci.yml`
- `rust-toolchain.toml`
- `Cargo.toml` の `[package] rust-version`
- `README.md` の必要環境の説明

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない。
