# CI の MSRV job を rust-version 1.99 に追随させる

- Created: 2026-10-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-ci-msrv-job-rust-version
- Polished: {YYYY-MM-DD}

## 目的

develop の CI で MSRV job が失敗し続けている。検証に使うツールチェーンが
`Cargo.toml` の `rust-version` とずれており、MSRV の検証が成立していない。

## 現状

- `Cargo.toml` の `[workspace.package] rust-version` は 1.99 (直前の rustup コミットで
  1.98 から 1.99 に更新された)
- `.github/workflows/ci.yml` の msrv job は 1.98 のままツールチェーンをインストールして
  おり、実行時に `zakuro-core requires rustc 1.99` で失敗する
- `README.md` の必要環境の表記も 1.98 のまま

## 設計方針

- msrv job が使うツールチェーンと `README.md` の表記を `rust-version` と同じ 1.99 に揃える
- ジョブの構成 (runner、依存パッケージ、キャッシュ) は変えない

## 完了条件

- msrv job が 1.99 でビルドを検証し、develop の CI が成功すること
- `README.md` の必要環境が `rust-version` と一致すること

## 変更対象

- `.github/workflows/ci.yml`
- `README.md`
