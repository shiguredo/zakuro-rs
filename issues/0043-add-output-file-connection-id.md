# connection ID ファイル出力 (`--output-file-connection-id`) を追加する

- Created: 2026-08-24
- Completed: {YYYY-MM-DD}
- Branch: feature/add-output-file-connection-id
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の `--output-file-connection-id <FILE>` は、接続中の仮想クライアントの connection ID を JSON ファイルに定期的に出力する。外部スクリプトから現在接続しているクライアントの接続 ID を取得するために使われ、Sora SFU 側のログと突き合わせる用途がある。C++ 版との機能互換性を維持するために対応する。

## 現状

- zakuro-rs には対応する CLI 引数がなく (`src/args.rs` の `CommonArgs` / `InstanceArgs`)、仕様は `docs/ZAKURO.md` に「DuckDB で代替可能」と記載され未対応のまま
- 接続統計は DuckDB (`src/duckdb_stats/`) で永続化され、接続単位の情報は全て DuckDB を直接クエリーすれば取得できる。したがって本対応は DuckDB を利用できない環境や既存スクリプトとの互換のための CLI 互換モードとして位置づける
- C++ 版は `zakuro/src/zakuro_stats.h` の ZakuroStats で仮想クライアントの stats を集計し、10 秒間隔で `{接続 URL: {チャネル ID: [connection ID...]}}` 形式の JSON をファイルへ書き込む (`zakuro/src/main.cpp` の stats スレッド)

## 設計方針

1. `src/args.rs` の `CommonArgs` に `--output-file-connection-id <FILE>` を追加する
2. 既存の接続統計経路 (mpsc チャネル等) から connection ID を集計し、C++ 版と同じ 10 秒間隔で JSON ファイルへ書き出すタスクを追加する
3. ファイルの JSON 形式は C++ 版と同じ `{接続 URL: {チャネル ID: [connection ID...]}}` とする
4. 既存の DuckDB 統計出力とは独立して動作させる (並存可能)
5. 集計対象の connection ID は offer メッセージから抽出する (`src/duckdb_stats/stats_json.rs` の `parse_offer_ids` で実績あり)

## 完了条件

- `--output-file-connection-id <FILE>` 指定時に、C++ 版と同形式の JSON ファイルが 10 秒間隔で出力される (実サーバー接続での手動確認)
