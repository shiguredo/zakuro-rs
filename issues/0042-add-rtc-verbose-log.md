# libwebrtc の verbose ログを出力できるようにする

- Created: 2026-10-09
- Completed: {YYYY-MM-DD}
- Branch: feature/add-rtc-verbose-log
- Polished: {YYYY-MM-DD}

## 目的

`--log-level verbose` で libwebrtc / sora-rust-sdk の verbose ログを取得できるようにする。

接続が一斉に切れるなど原因が自明でない事象の切り分けでは、SRTP / SCTP の内訳、ICE の
候補と経路の変化、DataChannel のメッセージ種別とサイズといった verbose の情報が要る。
いまはこれらを一切取得できないため、調査は接続単位の統計と SDK の info ログだけに頼る
ことになり、切り分けに時間がかかる。

## 現状

- `--log-level` は `verbose` を受け付けるが、verbose の行は出力されない。README の
  `--log-suppress` の節に制約として書いてある
- 原因は `webrtc::LogSink::min_severity_` が LS_INFO 固定で、C API に setter が無いこと。
  `zakuro/src/log_filter.rs` のモジュールコメントに記載している
- zakuro は `--log-suppress` と dummy ADM のログ抑制のために、stderr への直接出力を
  `set_log_to_stderr(false)` で止め、libwebrtc の LogSink 経由で出し直している。このため
  sink に届かない verbose の行は、どの経路でも出力されない
- `zakuro/src/main.rs` のログ初期化では `--log-level` を sink の設定と `log_bridge` の
  両方に渡しているが、sink の min severity 自体は変えられない

## 設計方針

- `shiguredo_webrtc` の LogSink に min severity を設定する C API を追加し、`zakuro` は
  `--log-level` に応じて sink の min severity を設定する。libwebrtc 側の `LS_VERBOSE` の
  行が sink に届くようにする
- stderr への直接出力を戻す案は採らない。戻すと `--log-suppress` による行単位の抑制が
  効かなくなり、ログ量の制御ができなくなるため
- verbose の行は量が非常に多いため、既定は現状どおり `info` とし、`--log-suppress` と
  併用できることを前提にする。README の `--log-suppress` の節の制約の記述も更新する

## 完了条件

- `--log-level verbose` を指定したときに verbose のログ行が出力されること
- `--log-suppress` で verbose の行も抑制できること
- `--log-level info` 以上を指定したときに verbose の行が出力されないこと
- README の `--log-suppress` の節が現状に合うように更新されていること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
