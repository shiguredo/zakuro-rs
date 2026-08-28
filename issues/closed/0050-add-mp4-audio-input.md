# `--input-mp4` で MP4 の音声も送信する（デコード → PCM → 再エンコード）

- Created: 2026-08-27
- Completed: 2026-08-28
- Branch: feature/add-mp4-audio-input
- Polished: 2026-08-27

## 目的

`--input-mp4` 指定時に、映像パススルーに加えて MP4 内の音声も送信できるようにする。負荷試験で映像だけ無音になる現状を解消し、音声付きの現実的なトラフィックを再現する。

音声は Opus / AAC を PCM にデコードし、既存の `FakeAudioCapturer` 経由で WebRTC の builtin Opus エンコーダーに渡す。エンコード済み Opus / AAC の RTP パススルーは、確認できた範囲で sora-rust-sdk / webrtc-rs にカスタム AudioEncoder の登録 API が無く、SDK 側の変更が必要なため本 issue の対象外とする。

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
3. デコード結果を有界の PCM FIFO / キューに入れ、`FakeAudioCapturer` の 10 ms（480 samples/channel）供給に合わせて切り出す。ファイル全体の PCM は保持しない。demux とデコードは既存の音声スレッド内で読み出し駆動で進め（FIFO に残量が不足していれば次の sample を読み出してデコードを追加する）、新しいスレッドや Mutex による共有状態は導入しない。音声トラックの終端に達したら先頭からループ再生する（映像のループとは独立）。デコードが供給に追いつかず FIFO が枯渇した場合は無音を送出して継続する
4. WebRTC の builtin Opus エンコーダーが再エンコードして送信する
5. `FakeAudioSource` に MP4 音声用バリアントを追加し、`audio_thread` から読み出す

### コーデックと未対応の扱い

- 初期対応は Opus と AAC（`SampleEntry::Opus` / `SampleEntry::Mp4a`）とする
- channel mapping family 0 のモノラルまたはステレオを対象とする。ステレオは既存 WAV と同様にモノラルへダウンミックスする（`FakeAudioCapturer` の `CHANNELS` が 1 のため）
- AAC の出力サンプルレートが 48 kHz 以外（例: 44.1 kHz）の場合は、`wav_reader` の resample 実装を流用して 48 kHz にリサンプリングする
- 音声トラックの扱いは次の場合分けで確定する
  - 音声トラックが無い MP4: エラーにせず、現状どおり映像のみとする
  - 音声トラックが 1 本で対応コーデックかつチャンネル構成対応（モノラル / ステレオ）: 音声を送信する
  - 音声トラックが 1 本で未対応コーデック、またはチャンネル構成非対応（3 チャンネル以上）: 英語の warning を出し、音声は送らず映像のみとする
  - 音声トラックが 2 本以上（対応・未対応コーデックを問わず）: 任意の 1 本を選ばず起動時エラーとする

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
- `shiguredo_fdk_aac`（AAC デコード。libfdk-aac 共有ライブラリを実行時に動的ロードするため、ロードできない環境で AAC 音声を含む MP4 を指定した場合は起動時エラーとする）

## 完了条件

- Opus または AAC 音声を含む MP4 を `--input-mp4` に指定すると、送信ロールで音声が送られる（受信側で聞こえること、または DuckDB の `rtc_stats_outbound_rtp`（kind=audio）の `packets_sent` が増加することで確認できること）
- MP4 音声はファイル終端で先頭からループ再生される（映像のループとは独立）
- 音声トラックが無い MP4 では従来どおり映像のみで動作する
- 未対応音声コーデックのみの場合は warning のうえ映像のみで動作する
- 音声トラックが 2 本以上の MP4（対応・未対応コーデックを問わず）は起動時エラーになる
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
- `README.md`（`--input-mp4` の説明、排他制約表の更新）

## pending にした理由

調査の結果、対応量が想定より大きいため後回しとする。

- MP4 音声 demux、Opus / AAC デコード、有界 PCM キュー、`FakeAudioCapturer` 配線、CLI 排他、ステレオダウンミックスなど、zakuro 単独でも実装範囲が広い
- 負荷試験としての優先度は映像パススルーに比べ低く、いま着手する必然性が薄い
- エンコード済み Opus のパススルーはさらに sora-rust-sdk / webrtc-rs の変更が必要で、本 issue の再エンコード方式でも上記の実装量がある

## reopened にした理由

pending にしたのは誤りで、後回しにする対象は sora-rust-sdk 側の MP4 音声対応である。zakuro-rs の本 issue は open のまま残す。

## 解決方法

`--input-mp4` 指定時に MP4 内の音声トラック (Opus / AAC) をデコードして送信できるようにした。

- `src/mp4_audio.rs` を新規作成
  - `inspect_mp4_audio()`: MP4 の音声トラックを調査する。音声トラック無しは映像のみで続行、2 本以上は起動時エラー、未対応コーデック (FLAC 等)・チャンネル構成非対応 (3 チャンネル以上) は warning のうえ映像のみで続行
  - `Mp4AudioSource`: 音声サンプルを順次読み出して Opus / AAC を PCM (48kHz モノラル) にデコードし、有界 FIFO で 10ms ごとの供給に合わせて切り出す。ファイル全体の PCM は保持しない
  - トラック終端で先頭からループ再生 (映像のループとは独立)。Opus はデコーダーをリセットして dOps の PreSkip を再破棄、AAC はデコーダーを再生成 (FDK 内部バッファの持ち越し防止)
  - ステレオは `wav_reader::downmix_to_mono` でダウンミックス、44.1kHz 等の AAC は `wav_reader::resample` で 48kHz へリサンプリング (既存実装を共用)
  - `--fdk-aac-lib` で指定した libfdk-aac を Linux で動的ロードする AAC デコードは Linux 限定 (クレートが非 Linux でコンパイルエラーになるため依存とコードを cfg ゲート)
- `src/fake_audio_capturer.rs`: `FakeAudioSource::Mp4Audio` バリアントを追加し、音声スレッドから MP4 音声を供給
- `src/main.rs`: `--input-mp4` かつ送信ロール・音声有効・`--no-audio-device` でないときに MP4 音声トラックを調査し、対応音声があれば外部 ADM を起動 (既存の `input_mp4.is_none()` ガードを外した)
- `src/args.rs`: 共通引数 `--fdk-aac-lib` を追加。`--input-mp4` と `--input-wav` の同時指定を起動時エラーに変更
- `Cargo.toml` / `Cargo.lock`: `shiguredo_mp4` / `shiguredo_opus` を追加。`shiguredo_fdk_aac` は `[target.'cfg(target_os = "linux")'.dependencies]` で Linux 限定に追加
- `.github/workflows/ci.yml`: Linux CI のビルドに `libfdk-aac-dev` を追加
- テスト
  - `src/mp4_audio.rs`: `testdata/` の ffmpeg 生成フィクスチャを使う単体テストを追加。Opus ステレオ / モノラルの調査とデコード (PreSkip 破棄・ループ時の再適用・2 周目の PCM 一致・FIFO 枯渇時の無音送出・破損入力での継続)、映像 + 音声の複合 MP4、音声トラック無し / 2 本以上 / FLAC / 6ch の分岐、AAC (Linux 限定) のデコード・リサンプリング・ループ一致
  - `src/args.rs`: `--input-mp4` × `--input-wav` 排他、`--fdk-aac-lib` のパース / 存在検証 / JSONC 経由の展開と拒否のテストを追加
- ドキュメント: `docs/ZAKURO.md` の実装状況と設計差分表、`README.md` の MP4 パススルー節・オプション表・排他制約表を更新。`testdata/README.md` にフィクスチャの生成コマンドを記録
- `CHANGES.md` は CODEBASE.md の規約 (2026.0.0 の間は記載しない) により更新していない
