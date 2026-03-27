# 解像度固定モード (`--fixed-resolution`) を追加する

Created: 2026-03-27
Model: Opus 4.6

## 概要

`--fixed-resolution` オプションを追加し、WebRTC の品質アダプテーションによる解像度変更を抑制して、指定した解像度を維持するようにする。

## 根拠

zakuro (C++) では `--fixed-resolution` オプションにより、帯域不足時でも解像度を固定できる。負荷試験で一定の映像品質を保証したい場合や、解像度変動による影響を排除したテストに必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. コマンドライン引数

- `--fixed-resolution` フラグを追加する

### 2. 映像キャプチャの変更

- `--fixed-resolution` 有効時に WebRTC のフレームアダプテーションを無効化する
- FakeVideoCapturer、VideoDeviceCapturer の両方に対応する
