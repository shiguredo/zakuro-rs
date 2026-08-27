# `--input-mp4` で MP4 の音声も送信する（デコード → PCM → 再エンコード）

- Created: 2026-08-27
- Completed: {YYYY-MM-DD}
- Branch: feature/add-mp4-audio-input
- Polished: {YYYY-MM-DD}

## 目的

`--input-mp4` 指定時に、映像パススルーに加えて MP4 内の音声も送信できるようにする。負荷試験で映像だけ無音になる現状を解消し、音声付きの現実的なトラフィックを再現する。

音声は Opus / AAC を PCM にデコードし、既存の `FakeAudioCapturer` 経由で WebRTC の builtin Opus エンコーダーに渡す。エンコード済み Opus のパススルーは、sora-rust-sdk / webrtc-rs の変更が必要なため本 issue の対象外とする。

## 現状

- `--input-mp4` は `sora_sdk::Mp4SampleReader` / `Mp4VideoCapturer` による映像パススルーのみ。SDK の reader は音声トラックを読み出さない
- `src/main.rs` の `run_zakuro_instance` では `instance.input_mp4.is_some()` のとき `use_fake_audio` を false にしているため、外部 ADM も起動せず音声は送られない
- 音声入力の既存経路は `src/fake_audio_capturer.rs` の `FakeAudioCapturer` のみ。`FakeAudioSource::Generated` / `FakeAudioSource::Wav` が 10 ms ごとに 48 kHz モノラル PCM を `AudioTransportRef::recorded_data_is_available` へ渡す
- `WavReader` は開いた時点で全 PCM をメモリに保持する。長時間 MP4 や `--vcs` が大きい負荷試験では同方式はメモリを圧迫するため採用しない
- zakuro の `Cargo.toml` に `shiguredo_mp4` / `shiguredo_opus` / AAC デコーダー依存は無い（映像 demux は SDK 経由）

## 設計方針

### 送信経路

1. zakuro 側で `shiguredo_mp4` を使い、MP4 から音声トラックの sample（データ位置・サイズ・タイムスタンプ・duration・SampleEntry）を demux する。映像は従来どおり SDK の `Mp4SampleReader` を使い、音声 demux は独立させる
2. Opus は `shiguredo_opus::Decoder`、AAC は `shiguredo_fdk_aac` のデコーダーで PCM（48 kHz）へ変換する
3. デコード結果を有界の PCM FIFO / キューに入れ、`FakeAudioCapturer` の 10 ms（480 samples/channel）供給に合わせて切り出す。ファイル全体の PCM は保持しない
4. WebRTC の builtin Opus エンコーダーが再エンコードして送信する
5. `FakeAudioSource` に MP4 音声用バリアントを追加し、`audio_thread` から読み出す

### コーデックと未対応の扱い

- 初期対応は Opus と AAC（`SampleEntry::Opus` / `SampleEntry::Mp4a`）とする
- channel mapping family 0 のモノラルまたはステレオを対象とする。ステレオは既存 WAV と同様にモノラルへダウンミックスする（`FakeAudioCapturer` の `CHANNELS` が 1 のため）
- 音声トラックが無い MP4 はエラーにせず、現状どおり映像のみとする
- 未対応コーデックの音声トラックのみがある場合は英語の warning を出し、音声は送らず映像のみとする
- 音声トラックが 2 本以上ある場合は、任意の 1 本を選ばず起動時エラーとする

### CLI / 既存音声との関係

- `--input-mp4` かつ送信ロールかつ音声有効かつ `--no-audio-device` でないとき、MP4 に対応音声があれば外部 ADM を起動する（現状の `input_mp4.is_none()` ガードを外す）
- `--input-mp4` と `--input-wav` の同時指定は起動時エラーにする
- `--sora-audio=false` / `--no-audio-device` 時は MP4 音声も送らない

### 対象外

- エンコード済み Opus / AAC の RTP パススルー（カスタム `AudioEncoder` / FrameTransformer）
- sora-rust-sdk / webrtc-rs の変更
- 映像と音声の厳密な共有再生時計（負荷試験用途のため、音声は独立ループでよい。ずれの累積補正は必須としない）
- 音声のみの MP4（映像トラック無し）
- FLAC など Opus / AAC 以外の音声コーデック

### 依存

- `shiguredo_mp4`
- `shiguredo_opus`
- `shiguredo_fdk_aac`（AAC デコード）

## 完了条件

- Opus または AAC 音声を含む MP4 を `--input-mp4` に指定すると、送信ロールで音声が送られる（受信側で聞こえること、または stats で音声送信が確認できること）
- 音声トラックが無い MP4 では従来どおり映像のみで動作する
- 未対応音声コーデックのみの場合は warning のうえ映像のみで動作する
- 音声トラックが 2 本以上の MP4 は起動時エラーになる
- `--input-mp4` と `--input-wav` の同時指定は起動時エラーになる
- PCM をファイル全体ぶんメモリに展開しない（有界バッファでストリーム供給する）
- `cargo test` / `cargo clippy --all-targets -- -D warnings` が成功する
- `docs/ZAKURO.md` の実装状況に MP4 音声送信を追記する（`CODEBASE.md` どおり 2026.0.0 の間は `CHANGES.md` には書かない）

## 変更対象

- `Cargo.toml` / `Cargo.lock`（`shiguredo_mp4` / `shiguredo_opus` / `shiguredo_fdk_aac`）
- `src/` 配下の MP4 音声 demux・デコードモジュール（新規）
- `src/fake_audio_capturer.rs`（`FakeAudioSource` 拡張、必要ならステレオ→モノラル）
- `src/main.rs`（`use_fake_audio` 条件、MP4 音声ソースの組み立て）
- `src/args.rs`（`--input-mp4` と `--input-wav` の排他）
- `docs/ZAKURO.md`
