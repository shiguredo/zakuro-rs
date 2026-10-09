# インスタンスを複数のワーカー (スレッド + runtime) に分けて接続処理を分散する

- Created: 2026-10-09
- Completed: {YYYY-MM-DD}
- Branch: feature/split-instance-workers
- Polished: {YYYY-MM-DD}

## 目的

1 プロセスで多数の仮想クライアントを動かしたときに、接続ごとの処理が 1 本の
ワーカースレッドに集中するのを解消する。

## 現状

- zakuro は 1 つの tokio runtime + 1 つの LocalSet で全インスタンスを動かしている
- 受信専用を 119 接続で動かしたときのスレッド別 CPU (5 秒計測) は次のとおりで、
  tokio のワーカー 1 本だけが 86% に達し、残り 3 本は 10% 以下だった

| スレッド | CPU |
| --- | --- |
| tokio ワーカー | 86.6% |
| libwebrtc のスレッド | 74.6% |
| tokio ワーカー (残り 3 本) | 9.0% / 9.0% / 8.4% |

- libwebrtc 側はインスタンスごとに `PeerConnectionFactory` が作られるため、
  インスタンスを分ければ分散する (実測: 受信を 5 インスタンスに分けると
  1 本 74.6% が 5 本 21% になった)
- 一方 tokio 側は runtime が 1 つなので、インスタンスを分けても分散しない

## 設計方針

- インスタンスを実行するワーカー (1 スレッド + 1 runtime + 1 LocalSet) を複数起動し、
  インスタンスをラウンドロビンで割り当てる
- ワーカー数は「インスタンス数」と「利用可能な CPU 数」の小さい方にする
- インスタンスの future は libwebrtc 由来の !Send 型を保持するため、インスタンスの
  生成と実行はワーカーのスレッド上で行う (チャネルで渡すのはインスタンス引数だけにする)
- インスタンスが panic した場合もワーカーが結果を返し、main が結果を待ち続けないようにする
- ワーカーの終了を待ってから集計を確定させる

## 完了条件

- 全インスタンスの結果 (成功 / 失敗 / panic) が main に集約されること
- インスタンスごとに別スレッドで実行され、tokio のワーカーが分散すること
- Ctrl+C で全ワーカーが停止し、summary が出力されること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
