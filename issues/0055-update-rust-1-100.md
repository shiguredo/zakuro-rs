# CI を Rust 1.100 の変更に追従させる

- Created: 2026-11-16
- Completed: 2026-11-16
- Branch: feature/update-rust-1-100
- Polished: {YYYY-MM-DD}

## 目的

Rust 1.100 (Cargo 1.100) の変更で CI の ubuntu ジョブが失敗しているため、依存とコードを追従させて CI を緑に戻す。

## 現状

- Cargo 1.100.0 で build directory の配置が
  `target/<profile>/build/<pkg>/<hash>/out` に変わった
  (<https://github.com/rust-lang/rust/releases/tag/1.100.0>)
- `libduckdb-sys 1.10506.0` の `build_paths::profile_dir` は旧配置
  `build/<pkg>-<hash>/out` を前提としており、prebuilt DuckDB の展開先を決定できず panic する
- 2026-11-13 以降の `develop` の CI は ubuntu-24.04 / ubuntu-26.04 がこの理由で失敗している。
  MSRV (1.99) のジョブは成功している
- Rust 1.100 の clippy が `zakuro/src/scenario.rs` の
  `test_build_reconnect_scenario_structure` で `needless_range_loop` を指摘する
- `duckdb 1.20000.2` / `libduckdb-sys 1.20000.2` は新しい build directory 配置に対応している
- `duckdb 1.20000.2` で prebuilt をダウンロードするには `download-lib` feature を有効にする必要がある

## 設計方針

- `zakuro/Cargo.toml` の duckdb を `1.20000` にし、`download-lib` feature を有効にする
- `Cargo.lock` を再生成し、`THIRD_PARTY_LICENSES.md` を更新する
- `test_build_reconnect_scenario_structure` のループを `iter().enumerate().skip(1)` に変える。
  テストの意味は変えない
- `.cargo/config.toml` と README の DuckDB のダウンロード方法の記述を feature に合わせる
- 検証は `cargo fmt --all -- --check`、`cargo clippy --locked -p zakuro --all-targets -- -D warnings`、
  `cargo test --locked -p zakuro`、`cargo +1.99 check --locked --workspace` で行う

## 完了条件

- stable の CI (ubuntu-24.04 / ubuntu-26.04) と MSRV の CI が通ること
- DuckDB の統計出力テストを含む `zakuro` のテストが通ること
- `THIRD_PARTY_LICENSES.md` が `Cargo.lock` と一致していること

## 解決方法

- `zakuro/Cargo.toml` の duckdb を `1.20000` にし、`download-lib` feature を有効にした。
  `duckdb` と `libduckdb-sys` が `1.20000.2` になり、`Cargo.lock` を再生成した
- `libduckdb-sys 1.20000.2` は Cargo 1.100 の `build/<pkg>/<hash>/out` 配置に対応しているため、
  ubuntu の CI でも prebuilt DuckDB の展開先を決定できるようになった
- `zakuro/src/scenario.rs` の `test_build_reconnect_scenario_structure` のループを
  `scenario.ops.iter().enumerate().skip(1)` に変え、Rust 1.100 の
  `needless_range_loop` の指摘を解消した。テストの意味は変えていない
- `.cargo/config.toml` と README の DuckDB のダウンロード方法の記述を `download-lib` feature に合わせた
- `THIRD_PARTY_LICENSES.md` を `Cargo.lock` に合わせて再生成した
- 検証: `cargo fmt --all -- --check`、
  `cargo clippy --locked -p zakuro --all-targets -- -D warnings`、
  `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings`、
  `cargo test --locked --workspace --features fdk-aac`
  (zakuro 316 件 / zakuro-core 8 件 / zakuro-moq 126 件)、
  `cargo +1.99 check --locked --workspace` が exit 0
- fdk-aac のヘッダーは Ubuntu の `libfdk-aac-dev` を展開して
  `FDK_AAC_INCLUDE_DIR` を指定して検証した
