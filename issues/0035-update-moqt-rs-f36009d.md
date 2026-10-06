# moqt-rs の rev を f36009d へ更新する

- Created: 2026-10-06
- Completed: {YYYY-MM-DD}
- Branch: feature/update-moqt-rs-f36009d
- Polished: {YYYY-MM-DD}

## 目的

`zakuro-moq` が固定している `shiguredo_moqt` の rev を現行 `develop` の最新へ更新する。
固定 rev が古いままだと、moqt-rs 側で入った GOAWAY や datagram 周りの修正が
`zakuro-moq` の負荷試験に反映されない。

## 現状

- `zakuro-moq/Cargo.toml` は `shiguredo_moqt` を rev
  `6c3ab63e89f7c196d269deaee51881dfd9d71730` (2026-10-05) で固定している
- 現行 `develop` の最新は `f36009d45bfa373b693642630b8f355b4d0aaed4` (2026-10-06) である。
  `6c3ab63` は `f36009d` の祖先であり到達できる
- 差分の主な内容は次のとおり
  - `session` の GOAWAY deadline 満了への対応 (request stream の終端と遅延状態の整理) と
    `TerminationReason::GoawayTimeout` の追加
  - `examples` (moq-pub / moq-sub) の改善 (datagram の上限超過を対処法付きのエラーにする、
    音声再生の jitter buffer など)
  - issue / ドキュメントの整備
- `zakuro-moq` は `shiguredo_moqt` の Sans I/O な `session::core::Session` と
  `SessionEvent` を使う。`SessionEvent` は未対応 variant を無視する形で扱っており、
  `TerminationReason` は判定に使っていないため、今回の差分でコードの変更は不要と見込まれる

## 設計方針

- rev を `f36009d` へ更新し、`cargo update -p shiguredo_moqt` で `Cargo.lock` を再生成する
- `zakuro-moq` の実装は変えない (API の変更があればビルドエラーとして検出し、そのときに対処する)

## 完了条件

- `cargo check -p zakuro-moq --all-targets` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
- `cargo fmt --all -- --check` が通ること
