# WAV 音声ファイル読込機能 (`--fake-audio-capture`) を追加する

Created: 2026-03-27
Model: Opus 4.6

## 概要

`--fake-audio-capture` オプションで WAV ファイルから音声を読み込み、フェイク音声として送信する機能を追加する。

## 根拠

zakuro (C++) では `--fake-audio-capture` で WAV ファイルを指定し、その音声を繰り返し送信できる。特定の音声パターンでの負荷試験や、音声品質の検証に必要。C++ 版との機能互換性を維持するために対応する。

## 対応内容

### 1. WAV リーダーモジュール

- WAV ファイル (PCM 16bit, モノラル/ステレオ) を読み込む
- サンプルレート変換 (48kHz へのリサンプリング) を実装する
- EOF でループ再生する

### 2. コマンドライン引数

- `--fake-audio-capture <FILE>` オプションを追加する

### 3. AudioSource との連携

- 10ms フレーム単位で AudioSource に供給する
