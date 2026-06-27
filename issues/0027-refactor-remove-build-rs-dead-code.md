# build.rs の他プロジェクト由来の死にコードを削除する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/refactor-remove-build-rs-dead-code
- Polished: 2026-00-00

## 目的

`build.rs` に残っている momo プロジェクト由来の死にコードを削除する。

## 優先度根拠

- `MOMO_COMMIT_SHORT` / `MOMO_BUILD_FLAGS` 環境変数はコードベース全体で一切参照されていない
- `AYAME` / `SORA` / `RASPBERRYPI` / `PREVIEW` feature flag 検出は Cargo.toml に `[features]` セクションがないため常に空
- `cargo:rerun-if-changed=.git/HEAD` により不要な再ビルドが発生している
- 死にコードは保守の混乱を招く

## 現状

```rust
// build.rs:3-18 — 未使用の git コミットハッシュ取得
println!("cargo:rustc-env=MOMO_COMMIT_SHORT={commit}");

// build.rs:22-33 — 存在しない feature flag 検出
if std::env::var("CARGO_FEATURE_AYAME").is_ok() { ... }
if std::env::var("CARGO_FEATURE_SORA").is_ok() { ... }
// ...

// build.rs:34 — 未使用の環境変数
println!("cargo:rustc-env=MOMO_BUILD_FLAGS={}", flags.join(", "));

// build.rs:37-38 — 不要な再ビルドトリガー
println!("cargo:rerun-if-changed=.git/HEAD");
```

## 設計方針

build.rs 全体を削除する。コミットハッシュやビルドフラグの埋め込みが必要な場合は zakuro 用に新しく書き直す。

## 完了条件

- build.rs が削除されていること、または zakuro 用に書き直されていること
- `cargo build` が正常に完了すること
- 不要な再ビルドが発生しなくなっていること

## 解決方法

`build.rs` ファイルを削除する。将来的にコミットハッシュ埋め込みが必要になった場合は、zakuro 固有の環境変数名 (`ZAKURO_COMMIT_SHORT`) で再実装する。
