# RPC に Query メソッドを追加して DuckDB にクエリーを実行できるようにする

Created: 2026-03-27

## 概要

JSON-RPC に `Query` メソッドを追加し、RPC 経由で DuckDB に対して任意のクエリーを実行できるようにする。

## 根拠

zakuro (C++) では PR #78 で RPC の Query メソッドが追加されている。外部ツールから DuckDB に蓄積された統計データを SQL で問い合わせることで、テスト中のリアルタイムモニタリングや自動化されたテスト検証が可能になる。C++ 版との機能互換性を維持するために対応する。

## 参考

- https://github.com/shiguredo/zakuro/pull/78
- https://github.com/shiguredo/zakuro/pull/79 (PR #78 と同一内容)

## 対応内容

### 1. JSON-RPC に Query メソッドを追加する

- Query メソッドのリクエストパラメータとして SQL 文字列を受け取る
- DuckDB に対してクエリーを実行し、結果を JSON で返す

### 2. DuckDB 統計書き込みモジュールの拡張

- 外部からのクエリー実行インターフェースを追加する
- クエリー結果を JSON に変換する機能を追加する

### 3. HTTP サーバーとの統合

- JSON-RPC ハンドラーから DuckDB へのアクセスを可能にする

## 依存

- 0003-add-json-rpc
- 0005-add-duckdb-stats-writer
