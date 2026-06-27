# build.rs の他プロジェクト由来の死にコードを削除する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-06-28
- Model: DeepSeek V4 Pro
- Branch: feature/refactor-remove-build-rs-dead-code
- Polished: 2026-06-28

## 目的

`build.rs` に残っている momo プロジェクト由来の死にコードを削除する。

## 優先度根拠

- `MOMO_COMMIT_SHORT` / `MOMO_BUILD_FLAGS` 環境変数はコードベース全体で一切参照されておらず、新規参加者が grep で見つけて依存があると誤認する原因になる
- `AYAME` / `SORA` / `RASPBERRYPI` / `PREVIEW` feature flag 検出は Cargo.toml に `[features]` セクションがないため常に空
- `cargo:rerun-if-changed=.git/HEAD` および `.git/refs` により、git 操作のたびに不要な再ビルドが発生している

## 現状

```rust
// build.rs:18 — 未使用の git コミットハッシュ取得
println!("cargo:rustc-env=MOMO_COMMIT_SHORT={commit}");

// build.rs:22-33 — 存在しない feature flag 検出
if std::env::var("CARGO_FEATURE_AYAME").is_ok() { ... }
if std::env::var("CARGO_FEATURE_SORA").is_ok() { ... }
if std::env::var("CARGO_FEATURE_RASPBERRYPI").is_ok() { ... }
if std::env::var("CARGO_FEATURE_PREVIEW").is_ok() { ... }

// build.rs:34 — 未使用の環境変数
println!("cargo:rustc-env=MOMO_BUILD_FLAGS={}", flags.join(", "));

// build.rs:37-38 — 不要な再ビルドトリガー
println!("cargo:rerun-if-changed=.git/HEAD");
println!("cargo:rerun-if-changed=.git/refs");
```

## 設計方針

build.rs を削除する。全内容が他プロジェクト由来の死にコードであり、残す理由がない。

## 完了条件

- `build.rs` が削除されていること
- `cargo build` が正常に完了すること
- `cargo build` を 2 回連続実行した際に 2 回目が再ビルドを発生させないこと
- `cargo test` が全テスト通過すること
- `cargo clippy --workspace` が警告なしで通過すること

## 解決方法

`build.rs` を `git rm` で削除した。全内容が他プロジェクト (momo) 由来の死にコードであり、残す理由がないため。

- `MOMO_COMMIT_SHORT` / `MOMO_BUILD_FLAGS` 環境変数はコードベース全体で一切参照なし
- `AYAME` / `SORA` / `RASPBERRYPI` / `PREVIEW` feature flag 検出は Cargo.toml に `[features]` セクションがないため常に空
- `cargo:rerun-if-changed=.git/HEAD` / `.git/refs` は git 操作のたびに不要な再ビルドを引き起こしていた

### 変更ファイル

- `build.rs` (削除)
