# フェイク音声自動生成機能を追加する

- Priority: Medium
- Created: 2026-03-27
- Completed: 2026-07-15
- Branch: feature/add-fake-audio-generate
- Polished: 2026-07-14

## 目的

WAV 未指定かつフェイク音声経路が有効なとき、現行の映像同期ビープ（ほぼ無音 + 短ビープ）を、C++ `Type::Safari` 相当の BIP / BOP / HUM / ノイズ連続 PCM（48kHz モノラル・2 秒ループ）に置き換える。

実マイクや `--input-wav` なしでも、受信側の音声処理パスに定常信号が流れる負荷試験を可能にする。カテゴリは `add` だが、デフォルト経路の破壊的置換を含む（旧 Beep は残さない）。

## 優先度根拠

Medium。

- GameAudioManager を実装しない方針のもと、WAV なしの連続音声が無いと「常時エンコード負荷のある音声送信」の試験ができない。Beep は Raden パイチャート一周時のみ短音で、sandstorm / y4m / `--no-video-device` では実質無音のまま
- 音声パス自体は Beep / Wav で既に動くため High にはしない。破壊的変更ではあるが、旧 Beep の A/V 同期キュー用途は本ツールの主目的（負荷試験）ではない

## 現状

### zakuro-rs

`src/fake_audio_capturer.rs` の `FakeAudioSource` は `Beep(BeepTrigger)` / `Wav(WavReader)` のみ。

- `Beep`: 通常無音。`BeepTrigger::trigger()` で 1000Hz / 100ms / 振幅 16000
- トリガ発火は `fake_video_capturer.rs` の **Raden 経路のみ**（`tick_raden` → `draw_animations`）。sandstorm / y4m では `BeepTrigger` を保持しても発火せず実質無音。`--no-video-device` も同様
- ADM 供給は既に 48kHz / モノラル / 10ms（再実装しない）

有効条件（`src/main.rs` の `use_fake_audio`）:

```text
!no_audio_device && audio && role.wants_send()
  && input_mp4.is_none() && video_input_device.is_none()
```

`FakeAudioCapturer` は **instance あたり 1**（1 音声スレッド）。複数 VC は同一 ADM を共有する。

### C++ zakuro（参照）

| 経路 | 実行時 | 生成本体 |
|------|--------|----------|
| WAV 未指定 | External + GameAudio（16kHz・キートリガ） | `zakuro.cpp` |
| WAV 指定 | SpecifiedFakeAudio | WavReader |
| AutoGenerateFakeAudio → `Type::Safari` | 通常 CLI では到達不能 | `zakuro_audio_device_module.cpp` の Safari |

Safari 定数（移植の一次参照）:

| 定数 | 値 |
|------|-----|
| `SAMPLE_RATE` | 48000 |
| バッファ長 | `SAMPLE_RATE * 2`（2 秒・モノラル i16） |
| `BIPBOP_DURATION` | 0.07 s |
| `bipbop_sample_count` | `(0.07_f64 * 48000.0).ceil() as usize`（= 3360。C++ は `(int)std::ceil(BIPBOP_DURATION * SAMPLE_RATE)`） |
| `BIPBOP_VOLUME` | 0.5 |
| `BIP_FREQUENCY` | 1500 Hz（`[0 .. bipbop)`） |
| `BOP_FREQUENCY` | 500 Hz（`[SAMPLE_RATE .. SAMPLE_RATE + bipbop)`） |
| `HUM_FREQUENCY` / `HUM_VOLUME` | 150 Hz / 0.1（全長加算） |
| `NOISE_FREQUENCY` / `NOISE_VOLUME` | 3000 Hz / 0.05（全長加算） |

`add_hum`: 各サンプルで `a = (int16_t)(volume * sin(i * 2π / (sample_rate/frequency)) * 32767)` を `*p += a`。呼び出しの `start` は常に `0`（BOP はポインタ／スライスオフセット）。C++ の `volume`/`frequency`/`sample_rate` 引数は `float`、`sin`/`M_PI` は double 混在。

理論上界（0.5+0.05+0.1）×32767 ≈ 21300 で i16 に収まる。

### ドキュメント

- `docs/ZAKURO.md`: Beep `[x]`、フル実装 `[ ]`。GameKeyCore / GameAudioManager は実装しない。設計差分表に「WAV 未指定時のデフォルト音源」は未記載
- `README.md`: 「映像同期ビープ」の記載あり

## 設計方針

### 1. スコープ

対象:

- Safari 相当の 2 秒 PCM を起動時に手続き生成し、10ms 単位でループ再生する
- `FakeAudioSource::Beep` を連続生成に **置き換える**（併存・旧 Beep フラグなし）
- `BeepTrigger` と映像 capturer のビープ配線（`tick_raden` 引数含む）を削除する
- `docs/ZAKURO.md` / `README.md` を新挙動に更新する

対象外:

- GameAudioManager / GameKeyCore / キートリガ数字音声 / `OpPlayVoiceNumberClient`
- `--input-wav` 経路の変更
- 埋め込みリソース（`0020`）。Safari は `sin` 加算のみで埋め込み不要（`0020` 側の依存記載修正は本 issue 対象外）
- 新規 CLI / JSONC / `InstanceArgs` 追加（closed `0021` の InstanceArgs 予告は採用しない）
- `CHANGES.md` 更新（`CODEBASE.md`: 2026.0.0 の間は記載しない）。バージョンが動いたら `[CHANGE]` でビープ廃止と連続自動生成への置換を記載する

### 2. 作業ブランチ

`Branch: feature/add-fake-audio-generate` は論理名。`CODEBASE.md` により 2026.0.0 の間はブランチを切らず `develop` 直で実装する。

### 3. 挙動

| 条件 | 変更前 | 変更後 |
|------|--------|--------|
| `use_fake_audio` かつ WAV なし（Raden） | ほぼ無音 + パイチャート同期短ビープ | Safari 2 秒ループ（常時） |
| 同上（sandstorm / y4m / `--no-video-device`） | 実質無音 | 同上（常時） |
| `use_fake_audio` かつ WAV あり | Wav ループ | 変更なし |
| `use_fake_audio` 偽 | なし | 変更なし |

配線: `use_wav_audio` → `Wav`、それ以外で `use_fake_audio` → `Generated`。`BeepTrigger` / `FakeVideoCapturerConfig.beep_trigger` / `audio_thread` の Beep ローカル状態は削除する。

### 4. データ構造

```rust
pub(crate) enum FakeAudioSource {
    Generated(GeneratedAudio),
    Wav(WavReader),
}
```

`GeneratedAudio` は **instance（= capturer）あたり 1**。`WavReader` と同型で `samples: Vec<i16>`（長さ `48000 * 2`）と `cursor: usize` を所有する。`Arc` 共有や VC ごとカーソルは不要。

`build_safari_audio() -> Vec<i16>` を純関数として切り出し、`GeneratedAudio` は薄いラッパとする。

### 5. 生成アルゴリズム

起動時に 1 回だけ組み立てる（毎 10ms 合成しない）。定数・順序は現状表どおり: BIP → BOP（`&mut data[SAMPLE_RATE..]`）→ NOISE 全長 → HUM 全長。

Rust の `add_hum` も C++ に合わせ **寄与ごとに `i16` へ切り捨ててから加算**する（浮動小数のまま累積して最後に一度キャストしない。`saturating_add` も使わない）。和は i16 内に収まるので、必要なら中間だけ `i32` に広げてから `as i16` してよい。

金値の位相: BIP / NOISE / HUM はバッファ絶対インデックス `i`。BOP のみスライス先頭からの相対インデックス `k`（絶対位置は `SAMPLE_RATE + k`）。`hum_period` は C++ 同様 `f32` 除算。完了条件は代表点の金値一致（全サンプル総当りは不要）。

## 完了条件

- `use_fake_audio` かつ WAV 未指定で Safari 相当の 2 秒ループが ADM 経由で送出される
- WAV 指定時は従来どおり `Wav` のみ
- `BeepTrigger` および映像 capturer のビープ配線が除去されている
- 下記単体テストがパスする
- `docs/ZAKURO.md`: Beep 行を **削除**し、フル実装行のみ `[x]`（二重チェックにしない）。設計差分表に「WAV 未指定時のデフォルト音源: C++ は External/GameAudio、zakuro-rs は Safari ループ（GameAudioManager 非実装のため）」を 1 行追加する
- `README.md`: 「映像同期ビープ」を連続自動生成の説明に更新し、旧 Beep が無くなったことと WAV 代替を一言含める
- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` がパス
- `CHANGES.md` は更新しない

## 解決方法

### 実装

- `src/fake_audio_capturer.rs`: C++ Safari 相当の `build_safari_audio` / `add_hum` / `GeneratedAudio` を追加し、`FakeAudioSource::Beep` を `Generated` に置き換えた。`BeepTrigger` と `BEEP_*` 定数、`audio_thread` のビープローカル状態を削除した
- `bipbop_sample_count` は C++ と同じ `(BIPBOP_DURATION * SAMPLE_RATE).ceil()`。`0.07 * 48000` の floating 誤差により値は 3361（issue 草案の 3360 表記は不正確）
- `src/fake_video_capturer.rs`: `beep_trigger` フィールドと `tick_raden` / `draw_animations` のトリガ配線を削除した
- `src/main.rs`: `use_wav_audio` → `Wav`、それ以外の `use_fake_audio` → `Generated` に配線した

### テスト

同ファイル `#[cfg(test)]` に以下を追加した（モックなし）:

- バッファ長 `48000 * 2` と `bipbop_sample_count == 3361`
- 金値: `samples[1]=3886` / `samples[SAMPLE_RATE+1]=1761` / `samples[bipbop+1]=1030`
- `GeneratedAudio::read_samples` の末尾折り返し

### ドキュメント

- `docs/ZAKURO.md`: Beep 行を削除しフル実装のみ `[x]`。設計差分表に「WAV 未指定時のデフォルト音源」を追加
- `README.md`: 連続自動生成への置換と旧 Beep 廃止・WAV 代替を記載
- `CHANGES.md` は `CODEBASE.md`（2026.0.0）どおり未更新
