# 接続の合否を集計してサマリを出力する

- Created: 2026-10-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-connection-outcome-summary
- Polished: {YYYY-MM-DD}

## 目的

負荷試験の結果を 1 つの数値として取り出せるようにする。接続 1 本ごとの合否は
`connection_lifecycle` テーブルに記録されるようになったが、試験全体の結果を機械的に
取り出すには DuckDB ファイルを読む必要があり、CI で判定に使うには重い。

## 現状

- `zakuro-core/src/stats.rs` の `StatsCollector` は、仮想クライアントの状態変化
  (`StatsEvent`) を集約して `StatsSnapshot` を 5 秒ごとにログへ出す。接続の合否は扱っていない
- 接続の合否は `zakuro/src/connection_lifecycle.rs` の `ConnectionLifecycle::judge` が
  求めるが、接続終了時に DuckDB へ書くだけで、集計には回っていない
- 接続確立までの所要時間は `attempt_started_at` と `webrtc_connected_at` から求められるが、
  その分布を見るには DuckDB へのクエリが要る
- プロセス終了時、`zakuro/src/main.rs` は DuckDB の writer を止めて終わるだけで、
  試験全体の結果をまとめて出す処理は無い

## 設計方針

- `StatsEvent` に接続終了のイベントを追加し、判定結果、失敗理由、停止の有無、確立までの
  所要時間を集約側へ渡す
- `StatsCollector` の集約タスクは、終了時に `StatsSummary` を返す。`StatsSummary` は
  判定結果別の接続数、失敗理由別の接続数、停止した接続数、確立までの所要時間の
  パーセンタイル (p50 / p95 / p99) を持つ
- 5 秒ごとのログ (`StatsSnapshot`) にも判定結果別の接続数を足す。`StatsSnapshot` は
  毎イベント複製されるため、重い集計 (所要時間の一覧) は入れない
- プロセス終了時に、人が読めるサマリをログへ出す。加えて `--summary-json <PATH>` が
  指定されていれば同じ内容を JSON でファイルへ書く
- しきい値判定と終了コードは後続で扱う

## 完了条件

- プロセス正常終了時に、判定結果別の接続数、失敗理由別の接続数、停止した接続数、
  確立までの所要時間の p50 / p95 / p99 がログに 1 回だけ出ること
- `--summary-json <PATH>` を指定すると、同じ内容が JSON ファイルに書かれること
- パーセンタイルの計算と、イベントの集約が単体テストで検証されること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
