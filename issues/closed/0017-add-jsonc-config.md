# JSONC 設定ファイル (`--config`) を追加する

Created: 2026-03-27
Completed: 2026-03-27
Model: Opus 4.6

## 概要

`--config` オプションで JSONC (JSON with Comments) 形式の設定ファイルを読み込み、コマンドライン引数の代わりに使用できるようにする。

## 根拠

zakuro (C++) では `--config` で JSONC 設定ファイルを指定でき、複雑な設定を再利用可能な形で管理できる。多数のオプションを組み合わせる負荷試験で、設定の共有や再現性の確保に必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. JSONC パーサー

- JSONC (コメント付き JSON) を解析する
- nojson の JSONC 対応機能を利用する

### 2. コマンドライン引数

- `--config <FILE>` オプションを追加する

### 3. 設定の統合

- 設定ファイルの値をコマンドライン引数と同等に扱う
- コマンドライン引数が設定ファイルより優先する

## 解決方法

`src/args.rs` に `load_jsonc_config()` と `merge_args_with_config()` を追加した。`nojson::RawJson::parse_jsonc()` で JSONC ファイルをパースし、キー・値を `--key value` 形式に変換する。設定ファイルの引数を先、CLI 引数を後に配置して `noargs::RawArgs::new()` に渡すことで、CLI 引数優先のセマンティクスを実現した。
