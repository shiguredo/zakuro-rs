# WAV 音声ファイル読込機能 (`--input-wav`) を追加する

Created: 2026-03-27
Completed: 2026-06-26
Model: Opus 4.6

## 概要

`--input-wav` オプションで WAV ファイルから音声を読み込み、音声入力として送信する機能を追加する。

## 根拠

zakuro (C++) では `--fake-audio-capture` で WAV ファイルを指定し、その音声を繰り返し送信できる。特定の音声パターンでの負荷試験や、音声品質の検証に必要。C++ 版との機能互換性を維持するために対応する。CLI 名称は zakuro-rs の入力ソース系命名規則 (`--input-{形式}`) に合わせて `--input-wav` とする。

## 対応内容

### 1. WAV リーダーモジュール

- WAV ファイル (PCM 16bit, モノラル/ステレオ) を読み込む
- サンプルレート変換 (48kHz へのリサンプリング) を実装する
- EOF でループ再生する

### 2. コマンドライン引数

- `--input-wav <FILE>` オプションを追加する

### 3. AudioSource との連携

- 10ms フレーム単位で AudioSource に供給する

## 解決方法

### 1. `src/wav_reader.rs` (新規)

- RIFF/WAVE ヘッダパース (`fmt ` / `data` チャンク、未知チャンクはスキップ)
- PCM 16bit / モノラル・ステレオのみ受理 (それ以外はエラー)
- ステレオは L+R を平均化してモノラル化
- 入力サンプルレートから 48kHz に線形補間でリサンプリング
- 開いた時点で全サンプルをメモリにロード、`read_samples()` でループ再生

### 2. `src/fake_audio_capturer.rs` (拡張)

- `FakeAudioSource` enum を導入: `Beep(BeepTrigger)` / `Wav(WavReader)`
- `FakeAudioCapturer::new(source)` のシグネチャを変更し、ビープ専用から音源切替式に
- `audio_thread()` 内で `match` 分岐して Beep / WAV それぞれの 10ms バッファを生成

### 3. `src/args.rs` / `src/main.rs`

- `--input-wav <FILE>` オプションを追加 (ファイル存在チェック付き)
- 排他バリデーション: `--no-audio-device` / `--sora-audio=false` と同時指定不可
- `main.rs` で WAV モードを判定して `WavReader` を構築し、`FakeAudioSource::Wav` を渡す
- WAV モードでは映像連動ビープは無効化 (`BeepTrigger` を生成しない)

### 4. ドキュメント

- `README.md` に `--input-wav` の使用例・引数表・排他制約を追加
- `docs/ZAKURO.md` の実装状況リストで `[x] WAV 音声ファイル読込 (--input-wav)` に変更

### 5. CHANGES.md

- `[ADD] CLI 引数 --input-wav で WAV ファイル (PCM 16bit) を音声入力としてループ再生できるようにする` を追記

### 6. 確認

- 単体テスト 7 件 (48kHz mono / ステレオダウンミックス / 44.1kHz→48kHz リサンプル / ループ再生 / 非 PCM 拒否 / 24bit 拒否 / 未知チャンクスキップ) を追加
- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` (23 件) が全てパス
