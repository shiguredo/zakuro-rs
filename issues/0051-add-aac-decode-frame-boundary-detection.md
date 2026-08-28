# AAC デコードでフレーム境界のずれを検出できるようにする

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/add-aac-frame-boundary
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

`--input-mp4` の AAC 音声デコードは「1 サンプル = 1 フレーム」を前提としている。この前提が崩れる入力 (1 サンプルに複数フレームを含む mp4a、または 1 フレーム未満のサンプルを含む破損 mp4a) では、パケット境界とフレーム境界のずれが警告なしに発生し、音の欠落・ズレが起きる。これを検出できるようにする。

## 現状

`src/mp4_audio.rs` の `decode_next_packet` (AAC デコード経路) は、`shiguredo_fdk_aac::Decoder` の `decode()` → `next_frame()` を 1 対 1 で呼ぶ。

- `next_frame()` は入力キューから 1 パケットを pop して `aacDecoder_Fill` + `aacDecoder_DecodeFrame` を 1 回実行するだけで、Fill 時の未消費バイト数 (`bytes_valid`) や FDK 内部入力バッファの残量を公開していない (shiguredo_fdk_aac 2026.1.0 の API 制約)
- 入力不足 (`AAC_DECODER_ERROR_AAC_DEC_NOT_ENOUGH_BITS`) の場合は `Ok(None)` を返し、データは FDK 内部バッファに残ったまま次のパケットと結合されてデコードされる
- ループ境界ではデコーダーを再生成してクリアされるが、ループ内のずれは解消されない

通常のエンコード済み MP4 (ffmpeg 等) では発生しないが、remux 由来の規格外ファイルや壊れたフレームを持つファイルで発生しうる。現在は仕様の前提として README に「AAC は 1 サンプル = 1 フレームの mp4a を前提」と明記するにとどまっており、ずれの検出手段がない。

## 設計方針

- 有力案: 依存クレートの `shiguredo_fdk_aac` に「1 回の DecodeFrame で消費された入力バイト数」を公開する API を追加してもらい、消費バイト数とパケットサイズの突き合わせでずれを検出する (shiguredo/fdk-aac-rs 側の変更が必要)
- 代替案: zakuro 側でトラックのタイムスケールとデコード出力サンプル数の対応を検証する (AAC トラックは通常 timescale = サンプルレートであり、サンプルの duration とデコード出力のサンプル数が対応するはず)
- ずれを検出したときの挙動 (警告して続行 / 起動時エラー) は実装時に確定する

## 完了条件

- AAC 音声を含む MP4 で「1 パケット = 1 フレーム」の前提が崩れている場合に、起動時または再生中にずれを検出して警告を出せること
- 正常な MP4 (ffmpeg 生成のエンコード済みファイル) では誤検出しないこと

## 変更対象

- `src/mp4_audio.rs` (`decode_next_packet` の AAC デコード経路、`AudioDecoderKind::Aac` の構築箇所)
- 必要に応じて依存クレート `shiguredo_fdk_aac` (shiguredo/fdk-aac-rs) の API 追加