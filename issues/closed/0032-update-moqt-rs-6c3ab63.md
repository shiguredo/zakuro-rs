# moqt-rs の rev を 6c3ab63 へ更新する

- Created: 2026-10-05
- Completed: 2026-10-05
- Branch: feature/update-moqt-rs-6c3ab63
- Polished: {YYYY-MM-DD}

## 目的

`zakuro-moq` が固定している `shiguredo_moqt` の rev を現行 `develop` の最新へ更新する。
moqt-rs は履歴が作り直され、0028 で固定した `641ebb1` は現行 `develop` の祖先ではなく
なった。固定 rev が到達できないままでは再現性のあるビルドができない。

## 現状

- `zakuro-moq/Cargo.toml` は `shiguredo_moqt` を rev
  `641ebb115e2552049ac2110332b98b36ae986790` (2026-10-04) で固定している
- 現行 `develop` の最新は `6c3ab63e89f7c196d269deaee51881dfd9d71730` (2026-10-04) であり、
  `641ebb1` はここから到達できない
- `zakuro-moq` は draft-22 の ALPN (`moqt-22`) と LOCATION_FILTER の Type-prefixed 符号化へ
  0028 で追随済みであり、今回の差に実装の変更は必要ない見込みである

## 設計方針

- rev を `6c3ab63` へ更新し、`cargo update -p shiguredo_moqt` で `Cargo.lock` を再生成する
- 実装の挙動は変えない
- 検証は rust-version 1.99 の toolchain で行う (CI の MSRV job と同じ)

## 完了条件

- `cargo check -p zakuro-moq --all-targets` が通ること
- `cargo clippy -p zakuro-moq --all-targets -- -D warnings` が通ること
- `cargo fmt --all -- --check` が通ること

## 解決方法

- `zakuro-moq/Cargo.toml` の rev を `6c3ab63e89f7c196d269deaee51881dfd9d71730` へ更新し、
  `cargo update -p shiguredo_moqt` で `Cargo.lock` を再生成した
- `641ebb1` と `6c3ab63` の差に `zakuro-moq` が使う API の変更はなく、コードの変更は
  不要だった
- 検証: rust 1.99.0 の toolchain で `cargo check -p zakuro-moq --all-targets` /
  `cargo clippy -p zakuro-moq --all-targets -- -D warnings` / `cargo fmt --all -- --check`
  が exit 0
