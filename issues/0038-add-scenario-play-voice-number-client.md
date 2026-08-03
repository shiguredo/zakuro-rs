# シナリオ操作 PlayVoiceNumberClient を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-scenario-play-voice-number-client
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) の ScenarioPlayer は `OpPlayVoiceNumberClient` を持ち、クライアント ID に対応する数字音声 (0-99 の英語読み上げ) を再生する (`zakuro/src/scenario_player.h` の `OP_PLAY_VOICE_NUMBER_CLIENT` 処理、`zakuro/src/voice_number_reader.h`)。C++ 版の reconnect シナリオでは Sleep の間に数字音声を挟んで、音声送信タイミングを制御している。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、reconnect シナリオは「C++ 版では Sleep の間に PlayVoiceNumberClient が挟まるが、zakuro-rs では音声再生未対応のため Sleep のみ」として実装されている
- `src/fake_audio_capturer.rs` がフェイク音声の生成 (`GeneratedAudio`: BIP / BOP / HUM / ノイズ、48kHz モノラル) と WAV 再生 (`FakeAudioSource::Wav`) を担い、`FakeAudioCapturer` が `AdmConfig::UseExternal` で Sora の AudioDeviceModule に接続されている
- C++ 版は数字音声の WAV リソースを EmbeddedBinary でバイナリに埋め込み、VoiceNumberReader が 0-99 を 16kHz モノラルの WAV 断片の連結で合成し、GameAudioManager 経由で再生する。GameAudioManager は zakuro-rs では実装しない方針 (`docs/ZAKURO.md` の「実装しない機能」)
- 埋め込みリソース機能は `issues/0020-add-embedded-resources.md` で未対応 (open)

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `PlayVoiceNumberClient { number: u32 }` を追加する
2. 数字音声のリソースは `issues/0020-add-embedded-resources.md` の対応に依存するため、0020 完了後に `include_bytes!` で埋め込んだ WAV を利用する。0020 完了前の先行実装では `--input-wav` と同じく外部ファイル読み込み (`src/wav_reader.rs`) で代替する
3. 再生経路は GameAudioManager を新設せず、`src/fake_audio_capturer.rs` の `FakeAudioSource` に数字音声ソースを追加して、シナリオ操作がフェイク音声キャプチャに再生を要求する方式とする
4. C++ 版は 16kHz であるが zakuro-rs のフェイク音声は 48kHz のため、`src/wav_reader.rs` のリサンプル処理 (48kHz への変換) を通す
5. 数字の合成規則 (0-29 は単体、30/40/.../90 は十の位 + 一の位の連結) は C++ 版 `voice_number_reader.h` に合わせる

## 完了条件

- シナリオに PlayVoiceNumberClient 操作が定義できる
- 操作到達時に指定番号の数字音声が再生され、Sora 経由で送信される
- 既存 reconnect シナリオに組み込める
