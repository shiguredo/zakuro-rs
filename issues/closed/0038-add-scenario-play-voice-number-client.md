# シナリオ操作 PlayVoiceNumberClient を追加する

- Created: 2026-08-03
- Completed: 2026-08-26
- Branch: feature/add-scenario-play-voice-number-client
- Polished: 2026-08-24

## 目的

zakuro (C++) の ScenarioPlayer は `OpPlayVoiceNumberClient` を持ち、シナリオ実行中に数字音声を再生する (`zakuro/src/scenario_player.h` の `OP_PLAY_VOICE_NUMBER_CLIENT` 処理)。C++ 版では引数なしの操作で、実行時に vc インデックス + 1 の番号の数字音声を GameAudioManager 経由で送信音声に載せる。C++ 版の reconnect シナリオでは Sleep の間に数字音声を挟んで音声送信タイミングを制御している。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/scenario.rs` の `ScenarioOp` は `Sleep` / `Disconnect` / `Exit` / `SendDataChannelMessage` / `Reconnect` を持つ (`PlayVoiceNumberClient` は非対応)
- `src/fake_audio_capturer.rs` がフェイク音声の生成 (`GeneratedAudio`: BIP / BOP / HUM / ノイズ、48kHz モノラル) と WAV 再生 (`FakeAudioSource::Wav`) を担う
- C++ 版は数字音声の WAV リソースを EmbeddedBinary でバイナリに埋め込み、VoiceNumberReader が 0-99 を 16kHz モノラルの WAV 断片の連結で合成し、GameAudioManager 経由で再生する。GameAudioManager は zakuro-rs では実装しない方針 (`docs/ZAKURO.md` の「実装しない機能」)

## 設計方針

1. `src/scenario.rs` の `ScenarioOp` に `PlayVoiceNumberClient` を追加する (C++ 版と同じく引数なし)。再生番号は vc_id + 1 で決定し、vc_id は `ScenarioPlayer` の生成時に受け取る。vc_id + 1 が 100 以上の場合は再生しない (C++ 版の `Read` は空を返すのと同じ)
2. 数字音声のリソースは埋め込みリソース issue には依存せず、本 issue で C++ 版 `resource/` から取得した WAV 断片をリポジトリに配置して `include_bytes!` で埋め込む
3. 再生経路は GameAudioManager を新設せず、`src/fake_audio_capturer.rs` の `FakeAudioSource` に数字音声ソースを追加して、シナリオ操作がフェイク音声キャプチャに再生を要求する方式とする
4. C++ 版は 16kHz であるが zakuro-rs のフェイク音声は 48kHz のため、リサンプル処理を通す
5. 数字の合成規則は C++ 版 `voice_number_reader.h` に合わせる

## 完了条件

- シナリオに PlayVoiceNumberClient 操作が定義できる
- 操作到達時に vc_id + 1 の番号の数字音声が再生され、Sora 経由で送信される (実サーバー接続での手動確認)

## 解決方法

一度実装したが、zakuro-rs では非対応とすることにした。実装を取り除き、本 issue を closed のまま非対応として扱う。

判定根拠:

- GameAudioManager は `docs/ZAKURO.md` の「実装しない機能」であり、数字音声再生はその代替実装になる。GameAudioManager 非実装方針に合わせて PlayVoiceNumberClient も実装しない
- reconnect シナリオの音声送信タイミング制御は Sleep のみで足りる (C++ 版との差分として許容する)
- 埋め込み WAV 断片 (44 ファイル) と再生経路の維持コストに見合う利用場面が zakuro-rs にない

取り除いたもの:

- `ScenarioOp::PlayVoiceNumberClient` と `ScenarioPlayer` の再生要求経路
- `src/voice_number_reader.rs` と `resource/voice_number/` の WAV 断片
- `FakeAudioSource::VoiceNumber` と再生要求チャネル

reconnect シナリオは `Reconnect → [Sleep(1-5s)] × 9 → ループ` とし、C++ 版の PlayVoiceNumberClient 挿入は行わない。`docs/ZAKURO.md` の「実装しない機能」に追記した。
