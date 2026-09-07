# LICENSE ファイルを追加する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-license-file
- Polished: {YYYY-MM-DD}

## 目的

OSS として公開するうえでライセンス本文の同梱は必須条件である。現在は `Cargo.toml` のメタデータに宣言があるだけで本文ファイルが存在せず、利用者が再配布・改変・特許の条件を特定できない状態になっている。

## 現状

- `Cargo.toml` の `[package]` に `license = "Apache-2.0"` の宣言がある
- リポジトリ直下に `LICENSE` / `LICENSE-APACHE` / `COPYING` / `NOTICE` のいずれも存在しない (`git ls-files` で該当 0 件)
- 同じ組織の公開リポジトリ (`shiguredo/hisui`、`shiguredo/sora-rust-sdk`、`shiguredo/webrtc-rs`) はいずれも `LICENSE` を追跡しており、`GitHub API` の `license.spdx_id` が `Apache-2.0` として検出されている
- `README.md` にもライセンスを説明するセクションが無い (`## 注意点` で終わっている)
- 依存クレートやビルド時にダウンロードするネイティブライブラリの第三者ライセンス表記は本 issue の対象外 (`issues/0060` で扱う)

## 設計方針

1. `hisui` と同じ形式に揃えるため、まず対象リポジトリの `LICENSE` の先頭 (条項本文以外: 著作権表記行の有無や年表記) を確認し、組織の実態に合わせる
2. Apache-2.0 の全文をリポジトリ直下の `LICENSE` として追加する。条項本文は標準テキストを使用し、独自の言い換えをしない
3. 著作権表記は組織の公開リポジトリと同一の表現に揃える (個人名ではなく組織名とする)
4. `Cargo.toml` の `license` フィールドは変更しない (宣言と本文が一致するようになるだけ)
5. 追加後、リモートに反映したうえで GitHub がライセンスを検出することを確認する (`GitHub API` の `license.spdx_id` が `Apache-2.0` になること)

## 完了条件

- リポジトリ直下に `LICENSE` が追跡され、Apache-2.0 の全文が含まれていること
- `Cargo.toml` の `license` 宣言と本文ファイルのライセンスが一致していること
- 著作権表記が `hisui` など組織の既存公開リポジトリと表現を揃えていること
- GitHub 上でライセンスが `Apache-2.0` として検出されること (反映後に確認)

## 変更対象

- `LICENSE` (新規)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
