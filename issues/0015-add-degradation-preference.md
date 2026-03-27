# degradation-preference オプションを追加する

Created: 2026-03-27
Model: Opus 4.6

## 概要

`--degradation-preference` オプションを追加し、帯域不足時の映像品質低下戦略を選択できるようにする。

## 根拠

zakuro (C++) では `--degradation-preference` で `disabled`、`maintain_framerate`、`maintain_resolution`、`balanced` を選択できる。負荷試験において帯域制限下での挙動を制御するために必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. コマンドライン引数

- `--degradation-preference {disabled,maintain_framerate,maintain_resolution,balanced}` オプションを追加する

### 2. WebRTC エンコーダ設定

- 指定された戦略を WebRTC のエンコーダ設定に反映する
