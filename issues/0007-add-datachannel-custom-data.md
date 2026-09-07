# DataChannel でカスタムデータを送信できるようにする

Created: 2026-03-27

## 概要

DataChannel の設定で `data` フィールドを指定すると、指定した JSON データをシリアライズして送信できるようにする。`data` が未指定の場合は従来通り ZAKURO ヘッダ付きバイナリで送信する。

## 根拠

zakuro (C++) では PR #80 で DataChannel のカスタムデータ送信機能が追加されている。固定フォーマットのバイナリだけでなく、ユーザー定義の JSON データを送信できるようにすることで、より柔軟な負荷試験シナリオに対応できる。C++ 版との機能互換性を維持するために対応する。

## 参考

- https://github.com/shiguredo/zakuro/pull/80

## 対応内容

### 1. DataChannel 設定の拡張

- `--sora-data-channels` の JSON 設定内に `data` フィールドを追加する
- `data` フィールドが指定された場合、その JSON データをシリアライズして送信する

### 2. 送信ロジックの分岐

- `data` が指定されている場合: JSON データをシリアライズして送信する
- `data` が未指定の場合: 従来通り ZAKURO ヘッダ (シグネチャ + タイムスタンプ + カウンター + Connection ID) + ランダムバイナリで送信する

### 3. args.rs の変更

- `data-channels` 設定から `data` フィールドを読み取る処理を追加する
