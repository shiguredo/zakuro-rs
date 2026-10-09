# 機械的に繰り返される libwebrtc のログを既定で抑制する

- Created: 2026-10-09
- Completed: {YYYY-MM-DD}
- Branch: feature/suppress-noisy-libwebrtc-logs
- Polished: {YYYY-MM-DD}

## 目的

負荷試験では使わない libwebrtc / sora-sdk の機械的なログを既定で抑制し、ログの量と
書き込み負荷を減らす。

受信専用の仮想クライアントを多数動かすと、libwebrtc が接続ごと・ストリームごと・
パケットごとに INFO ログを出し続ける。120 接続で 30 分動かしたときの実測では 29 万行
(48 MB) に達し、その 8 割近くが次の 6 種類で占められていた。

| 行数 | 割合 | 発生元 |
| --- | --- | --- |
| 85,807 | 29% | `basic_port_allocator.cc` (ネットワーク一覧とポート割り当ての経過) |
| 33,602 | 12% | `rtp_streams_synchronizer2.cc` (ストリームごとの同期統計) |
| 21,480 | 7% | sora-sdk の `[WebSocket] Received Pong` |
| 20,758 | 7% | `rtp_video_stream_receiver2.cc` (パケット 1 個ごとのログ) |
| 19,497 | 7% | `webrtc_video_engine.cc` (ストリームごとの映像統計) |
| 18,731 | 6% | `turn_port.cc` (TURN のリクエスト 1 回ごとの経過) |

1 行ごとに整形して stderr へ書き込むため、負荷が高いときほどログが増えて受信処理を
圧迫し、取りこぼしが増えてさらにエラーログが増えるという悪循環にもなる。

## 現状

- `--log-suppress` は運用者が抑制対象を指定する仕組みだが、既定では何も指定されない
  (`zakuro/src/log_filter.rs`)
- `--log-level` を `warning` にすれば INFO 行は消えるが、zakuro 自身の `[stats]` 行
  (`zakuro_core`、INFO) も同時に消えるため運用者は選びにくい
- dummy ADM のメッセージだけはコード組み込みで常に抑制している

## 設計方針

- 発生元ファイル名 (`basic_port_allocator.cc` など) とメッセージ (`[WebSocket] Received Pong`
  など) の 2 種類の既定の抑制リストを持ち、**INFO 以下の行だけ**を落とす。同じファイルが
  出す WARNING / ERROR は残す (実障害の切り分けに必要)
- 既定の抑制は `--log-level verbose` のときだけ無効にする。verbose は「抑制せず全部出す」
  の意味にする (sink の min severity が LS_INFO 固定のため verbose レベルの行自体は出ない)
- 判定は純粋な関数にして単体テストで検証する

## 完了条件

- 既定で、上記の INFO 以下のログが出力されないこと
- 同じ発生元の WARNING / ERROR は出力されること
- 切断理由 (`[WebSocket] Received Close`) や経路の切り替えなど、切り分けに使う INFO が
  出力されること
- `--log-level verbose` で既定の抑制が無効になること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
