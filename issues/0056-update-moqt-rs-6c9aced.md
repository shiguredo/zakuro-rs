# moqt-rs の rev を 6c9aced へ更新する

- Created: 2026-11-23
- Completed: {YYYY-MM-DD}
- Branch: feature/update-moqt-rs-6c9aced
- Polished: {YYYY-MM-DD}

## 目的

zakuro-moq が依存する `shiguredo_moqt` を moqt-rs の develop 最新に追従させ、moqt-rs 側で
公開 API が変わった状態でも zakuro-moq をビルド・テストできる状態を保つ。

## 現状

- `zakuro-moq/Cargo.toml` の `[dependencies.shiguredo_moqt]` の rev は
  `e25dae7a2fa8c164bb2264176aeaf71bfd142ac0` (2026-11-22) を指している
- moqt-rs develop の最新は `6c9aced3f1d6223c98063a8083efb837c0504289` (2026-11-23) で、
  `e25dae7a` から 79 コミット進んでいる。`e25dae7a` は `6c9aced` の祖先であり、
  履歴の作り直しはない
- 差分に含まれる公開 API の変更は次のとおり
  - `SessionEvent` に `PeerClosed(SessionError)` を追加した。peer 起点の transport 層の
    セッション終了 (QUIC の `CONNECTION_CLOSE` / WebTransport の `WT_CLOSE_SESSION`) を、
    自側起点の `CloseSession` と variant で区別して通知する
  - `Session::recv_session_closed` を追加した。I/O 層が peer 起点の終了をコードと理由付きで
    通知する API である
  - `SessionError::reason` を `&'static str` から `Cow<'static, str>` に変更した
  - `SessionError::new` と `Session::close` が `impl Into<Cow<'static, str>>` を受けるようになり、
    `SessionError::new` は `const fn` ではなくなった
  - `MessageError` に `InvalidMsfPayload` を追加し、MSF の Log / Metrics の payload を扱う
    型と関数を追加した (いずれも追加のみ)
- `SessionEvent` と `MessageError` は `#[non_exhaustive]` ではないため、variant の追加は
  網羅 match を持つ利用側にとって破壊的変更になる
- ALPN の識別子は `moqt-23`、対象 draft は draft-ietf-moq-transport-23 で変わっていない
- moqt-rs の `rust-version` は `1.95` のままで、zakuro-rs の MSRV (`1.100`) を超えていない
- rev だけを更新すると `SessionEvent::PeerClosed` が未処理になり、zakuro-moq は
  E0004 (non-exhaustive patterns) でコンパイルできない

## 設計方針

- `zakuro-moq/Cargo.toml` の rev を `6c9aced3f1d6223c98063a8083efb837c0504289` に更新し、
  `cargo update -p shiguredo_moqt` で `Cargo.lock` を再生成する
- `drain_events` の match に `SessionEvent::PeerClosed(_)` の明示的な arm を追加する。
  wildcard で網羅性を潰さず、variant が増えたことに気づける状態を保つ
- zakuro-moq は transport 層の peer 起点の終了をストリーム読み込みのエラーとして扱い、
  `Session::recv_session_closed` を呼ばない。したがって `PeerClosed` は到達しない。
  arm では状態を変えずに無視し、到達しない理由を日本語コメントに残す
- 既存の PUBLISH / SUBSCRIBE 経路の挙動は変えない

## 完了条件

- `cargo check --locked -p zakuro-moq --all-targets` が通ること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked -p zakuro-moq --all-targets -- -D warnings` が通ること
- `cargo test --locked -p zakuro-moq` が通ること
- `cargo +1.100 check --locked --workspace` が通ること
- `python3 scripts/generate_third_party_licenses.py` を実行したときに、rev の更新に伴う
  正当な差分だけが `THIRD_PARTY_LICENSES.md` に出ること
