.PHONY: ci test cover check clippy fmt fmt-check smoke clean

# CI と同じチェックを順に実行する。
# ci.yml と prek.toml も同じコマンド文字列を使う (定義を二重に持たせない)
ci: fmt-check clippy test smoke

# 整形されているかを確認する (ファイルは書き換えない)
fmt-check:
	cargo fmt --all -- --check

# 全テストを実行する
test:
	cargo test --locked --workspace --features fdk-aac

# 全テストカバレッジ付きで実行する (`cargo-llvm-cov` が必要)
cover:
	cargo llvm-cov --tests --workspace

# cargo check を実行する
check:
	cargo check --locked --workspace

# cargo clippy を実行する。--all-targets でテストコードも検査対象に含める。
# fdk-aac は Linux 限定の optional 依存だが、feature 指定自体は他 OS でも受理される
# (target 節で依存が解決されないだけ) ため、この行は macOS でも通る
clippy:
	cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings

# cargo fmt を実行する (ファイルは書き換える)
fmt:
	cargo fmt --all

# ビルドした実行ファイルを直接起動できることを確認する。
# cargo 経由の実行はライブラリ探索パスが注入されるため、rpath の欠落を検出できない。
# debug ビルドを使い、CI の実行時間を増やさない。
smoke:
	cargo build --locked --features fdk-aac
	./target/debug/zakuro --help

# ビルド成果物を削除する
clean:
	cargo clean
