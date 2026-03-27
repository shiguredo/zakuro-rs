# WebRTC 統計情報を DuckDB ファイルに保存する機能を追加する

Created: 2026-03-27
Model: Opus 4.6

## 概要

WebRTC の統計情報 (RTCStats) を DuckDB ファイルに定期的に保存する機能を追加する。`--duckdb-dir` と `--duckdb-interval` オプションで制御する。

## 根拠

zakuro (C++) では PR #75 で DuckDB への統計保存機能が追加されている。負荷試験の結果を永続化し、SQL で分析できるようにすることで、試験結果の可視化や比較が可能になる。C++ 版との機能互換性を維持するために対応する。

## 参考

- https://github.com/shiguredo/zakuro/pull/75

## 対応内容

### 1. DuckDB 統計書き込みモジュール

- DuckDB ファイルへの接続・テーブル作成を実装する
- zakuro, connection, rtc_stats_* テーブルを作成する
- 統計情報を定期的に書き込む (--duckdb-interval で間隔指定)
- Prepared statement による効率的な書き込み

### 2. コマンドライン引数

- `--duckdb-dir <DIR>` オプションを追加する (DuckDB ファイルの保存先ディレクトリ)
- `--duckdb-interval <SEC>` オプションを追加する (書き込み間隔)

### 3. VirtualClient との連携

- VirtualClient から統計情報を収集して DuckDB に書き込む

## 依存

- 0002-add-http-server
