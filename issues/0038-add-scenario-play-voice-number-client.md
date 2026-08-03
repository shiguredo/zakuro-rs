# シナリオ操作 PlayVoiceNumberClient を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-scenario-play-voice-number-client
- Polished: 2026-08-03

## 目的

zakuro (C++) の ScenarioPlayer は `OpPlayVoiceNumberClient` を持ち、シナリオ実行中に数字音声を再生する (`zakuro/src/scenario_player.h` の `OP_PLAY_VOICE_NUMBER_CLIENT` 処理)。C++ 版では引数なしの操作で、実行時に vc インデックス + 1 の番号の数字音声を GameAudioManager 経由で送信音声に載せる。C++ 版の reconnect シナリオでは Sleep の間に数字音声を挟んで音声送信タイミングを制御している。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` と `Disconnect` のみで、reconnect シナリオは「C++ 版では Sleep の間に PlayVoiceNumberClient が挟まるが、zakuro-rs では音声再生未対応のため Sleep のみ」として実装されている
- `src/fake_audio_capturer.rs` がフェイク音声の生成 (`GeneratedAudio`: BIP / BOP / HUM / ノイズ、48kHz モノラル) と WAV 再生 (`FakeAudioSource::Wav`) を担い、`FakeAudioCapturer` が `AdmConfig::UseExternal` で Sora の AudioDeviceModule に接続されている。`FakeAudioCapturer` は instance 単位で 1 つ生成され、`start()` がソースを音声スレッドへ move する構造
- C++ 版は数字音声の WAV リソースを EmbeddedBinary でバイナリに埋め込み、VoiceNumberReader が 0-99 を 16kHz モノラルの WAV 断片の連結で合成し、GameAudioManager 経由で再生する。GameAudioManager は zakuro-rs では実装しない方針 (`docs/ZAKURO.md` の「実装しない機能」)
- 埋め込みリソース機能は `issues/0020-add-embedded-resources.md` で未対応 (open) だが、0020 の音声リソースのスコープはフェイク音声生成の基本波形データであり、数字音声 WAV は含まれない

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `PlayVoiceNumberClient` を追加する (C++ 版と同じく引数なし)。再生番号は vc_id + 1 で決定し、vc_id は `ScenarioPlayer` の生成時に受け取る。vc_id + 1 が 100 以上の場合は再生しない (C++ 版の `Read` は空を返すのと同じ)
2. 数字音声のリソースは `issues/0020` には依存せず、本 issue で C++ 版 `resource/` から取得した WAV 断片 (num000_01.wav 〜 num090_02.wav の 44 ファイル) をリポジトリに配置して `include_bytes!` で埋め込む
3. 再生経路は GameAudioManager を新設せず、`src/fake_audio_capturer.rs` の `FakeAudioSource` に数字音声ソースを追加して、シナリオ操作がフェイク音声キャプチャに再生を要求する方式とする。再生要求は mpsc チャネルで音声スレッドに送り、数字音声の再生中は数字音声を送出し、再生完了後に元のソース (Generated / Wav) へ戻る。capturer は instance 単位で 1 つのため、複数 vc の同時要求は最後の要求で置き換わる (C++ 版は `Play(client_id, buf)` が範囲外で no-op になり実質 vc 0 のみ再生されるが、zakuro-rs は全 vc の要求を最後勝ちで受け付ける。これは意図した差分)。`FakeAudioCapturer` が生成されない場合 (音声無効や `--input-mp4` / `--video-input-device` 使用時) は再生要求を無視する。C++ 版は数字音声のない間は無音になるが、zakuro-rs は再生完了後に元のソースへ戻って常時送出を継続する (意図した差分)
4. C++ 版は 16kHz であるが zakuro-rs のフェイク音声は 48kHz のため、`src/wav_reader.rs` のリサンプル処理 (48kHz への変換) を通す (断片ごとにリサンプルして連結する)
5. 数字の合成規則は C++ 版 `voice_number_reader.h` に合わせる (0-29 は単体、30/40/.../90 は十の位のみ、31-99 は十の位 + 一の位の連結。十の位は連結用リソースを使う)。番号 → 断片の対応は単体テストで検証する
6. 既存 reconnect シナリオへの組み込みは本 issue では行わず (組み込みは Reconnect 操作対応と合わせて実施する)、動作確認は実サーバー (Sora SFU) への接続による手動確認で行う (モック・スタブ利用不可のため)。確認時のみ既存シナリオへの一時的な組み込みを許容する

## 完了条件

- シナリオに PlayVoiceNumberClient 操作が定義できる
- 操作到達時に vc_id + 1 の番号の数字音声が再生され、Sora 経由で送信される (実サーバー接続での手動確認)
