# WebSocket の close code と reason を記録する

- Created: 2026-10-09
- Completed: {YYYY-MM-DD}
- Branch: feature/add-record-websocket-close-code
- Polished: {YYYY-MM-DD}

## 目的

接続が切れた理由を、事後に DuckDB から読めるようにする。

いまは集計で `unexpected-disconnect` としか分からず、SFU 側がエラーで切ったのか、
ネットワークが切れたのか、接続数の上限などで拒否されたのかを切り分けられない。実際に
接続が一斉に切れる事象を調べたときは、生ログから `[WebSocket] Received Close` の行を
数える以外に手が無く、接続ごとの理由は追えなかった。

## 現状

- `sora-rust-sdk` の `SoraConnectionEventHandler` は `on_websocket_close(code, reason)` を
  持つが、`zakuro/src/virtual_client.rs` の `VirtualClientEventHandler` は実装していない。
  SDK 側の既定実装は何もせず、close code と reason は SDK の `rtc_log_info!` の行にしか
  残らない
- `zakuro/src/connection_lifecycle.rs` の `ConnectionLifecycle` は切断理由を `end_reason`
  に `unexpected` として持つだけで、close code と reason を持たない
- `zakuro/src/duckdb_schema.sql` の `connection_lifecycle` テーブルにも対応する列が無い

## 設計方針

- `VirtualClientEventHandler` に `on_websocket_close` を実装し、close code と reason を
  `ConnectionLifecycle` に記録する。最初に観測した値だけを残し、後続の通知で上書きしない
  (`mark_disconnected` と同じ方針)。切断の向きは `end_reason` で分かるため、close code は
  その補助情報として扱う
- `connection_lifecycle` に `websocket_close_code` (INTEGER、未観測は NULL) と
  `websocket_close_reason` (VARCHAR、未観測は NULL) を追加する
- `docs/DUCKDB.md` の `connection_lifecycle` の列一覧と注意事項を更新する
- `--summary-json` の `failure_reasons` は既存の分類のままとし、close code は DuckDB で
  見る。理由別の集計をサマリに足すかは別途判断する

## 完了条件

- サーバーから close code 付きで切られた接続について、その値が `connection_lifecycle` に
  記録されること
- close code を観測しなかった接続では NULL のままであること
- `docs/DUCKDB.md` に列の説明が追加されていること
- 単体テストで、最初に観測した close code が保持されることと、未観測のまま終わる場合が
  検証されること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
