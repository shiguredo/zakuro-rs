# moqt-rs の rev を f91baf3 へ更新する

- Created: 2026-10-15
- Completed: 2026-10-15
- Branch: feature/update-moqt-rs-f91baf3
- Polished: {YYYY-MM-DD}

## 目的

`zakuro-moq` が固定している `shiguredo_moqt` の rev を現行 `develop` の最新へ更新する。
固定 rev が古いままだと、moqt-rs 側で入った REQUEST_UPDATE の検証強化や
DATA_STREAM_TIMEOUT の扱いの変更が `zakuro-moq` の負荷試験に反映されない。

## 現状

- `zakuro-moq/Cargo.toml` は `shiguredo_moqt` を rev
  `f36009d45bfa373b693642630b8f355b4d0aaed4` (2026-10-06) で固定している
- 現行 `develop` の最新は `f91baf3be4f9946ddcf08cfacf4cc493795285f5` (2026-10-14) であり、
  差分は 232 コミットである。タグは無く、`f91baf3` より新しい ref も存在しない
- `zakuro-moq` が使う API のうちソース互換性が壊れたものは、`Session` の
  `recv_data_stream_stop_sending` に第 2 引数 `error_code: Option<u64>` が追加された 1 件である。
  `send_object_datagram` の引数追加は `zakuro-moq` が使わないため影響しない
- `SessionEvent` に `StopSendingDataStream` が追加された。`drain_events` は
  `SessionEvent` をワイルドカード無しで網羅 match しているため、対応する arm が必須である
- 差分の主な内容は次のとおり
  - DATA_STREAM_TIMEOUT の満了を、セッションのクローズから受信方向への
    STOP_SENDING 指示へ変更した
  - REQUEST_UPDATE の state / role 検証を追加し、仕様違反はセッション終了とした
  - 不正な AUTHORIZATION_TOKEN を、セッション終了から request 単位の拒否へ変更した
  - playout / audio_clock / video_decode_order などを追加した (`zakuro-moq` は使わない)

## 設計方針

- rev を `f91baf3` へ更新し、`cargo update -p shiguredo_moqt` で `Cargo.lock` を再生成する
- 破壊的変更には最小限で追従し、更新に起因する挙動の変更を持ち込まない
- ALPN は `moqt-22` のままとする。最新の moqt-rs も draft-22 のままであり、
  この 232 コミットに draft 改版もプロトコル識別子の変更も含まれない

## 完了条件

- `cargo check -p zakuro-moq --all-targets` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
- `cargo fmt --all -- --check` が通ること

## 解決方法

- `zakuro-moq/Cargo.toml` の rev を `f91baf3be4f9946ddcf08cfacf4cc493795285f5` へ更新し、
  `cargo update -p shiguredo_moqt` で `Cargo.lock` を再生成した
- `zakuro-moq/src/moq_client/session.rs` の `Publisher::on_stream_write_failure` で、
  `recv_data_stream_stop_sending` に第 2 引数 `None` を渡すようにした。peer が STOP_SENDING に
  載せたアプリケーションエラーコードは、STOP_SENDING の受信を観測する endpoint event
  subscriber を構成していないため取得できない
- 同ファイルの `run` にある `DataMessage::Header` の拒否経路の
  `recv_data_stream_stop_sending` 呼び出しを削除した。この API は自側が開いた送信
  data stream を対象とするため、受信 uni stream の id を渡すと必ず失敗し `let _ =` で
  破棄される no-op であった (旧 rev でも同じ)。Session へ受信停止を通知する API は
  `Session::send_data_stream_stop_sending` であり、拒否した stream の後始末を実装する
  ときに呼ぶ必要がある
- `drain_events` に `SessionEvent::StopSendingDataStream` の arm を追加した。
  `data_stream_timeout_ms` は既定が `None` で `zakuro-moq` は設定しないためこのイベントは
  到達しない。到達した場合に備えて warn を出し、未対応であることと必要な実装をコメントに
  残した
- 検証: `cargo fmt --all -- --check` /
  `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` /
  `cargo test --locked --workspace --features fdk-aac` (372 件 pass) /
  `cargo build --locked --workspace --features fdk-aac` と `zakuro` `zakuro-moq` の `--help` /
  `tombi lint` / `tombi format --check` が exit 0
- 更新に起因して認識しておくべき挙動差: relay が仕様違反の REQUEST_UPDATE を送ると、
  セッションが終了するようになった (仕様準拠の強化)
