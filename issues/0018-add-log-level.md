# ログレベル制御 (`--log-level`) を追加する

Created: 2026-03-27
Model: Opus 4.6

## 概要

`--log-level` オプションを追加し、ログ出力の詳細度を制御できるようにする。

## 根拠

zakuro (C++) では `--log-level` で `verbose`、`info`、`warning`、`error`、`none` を選択できる。現在 zakuro-rs はログレベルが `Info` にハードコードされている。デバッグ時の詳細ログや、本番運用時のログ抑制に必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. コマンドライン引数

- `--log-level {verbose,info,warning,error,none}` オプションを追加する

### 2. ログ設定

- 指定されたレベルを log クレートの設定に反映する
- デフォルトは `info` とする
