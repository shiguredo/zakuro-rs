# Y4M 動画ファイル読込機能を追加する

Created: 2026-03-24
Completed: 2026-03-24
Model: Opus 4.6

## 概要

`--fake-video-capture <FILE>` オプションで YUV4MPEG2 (Y4M) 形式の動画ファイルを読み込み、フェイク映像ソースとして使用できるようにする。

## 根拠

zakuro (C++) には Y4MReader が実装されており、任意の Y4M ファイルを映像ソースとして利用できる。zakuro-rs では現在 Raden 描画と砂嵐のみ対応しており、実際の映像コンテンツを使った負荷試験ができない。C++ 版との機能互換性を維持するために必要。

## 対応内容

### 1. Y4M リーダーモジュール (`src/y4m_reader.rs`)

- YUV4MPEG2 ヘッダパース (W, H, F, I, A, C トークン)
- 対応クロマフォーマット: C420, C420jpeg, C420paldv, C420mpeg2
- フレーム読込 (FRAME ヘッダ + I420 データ)
- 経過時間に基づくフレーム位置計算 (`frame = ms * fps_num / (1000 * fps_den)`)
- ファイル終端でのループ再生
- フレームスキップ (要求フレームまで読み飛ばし)
- 同一フレーム要求時の重複読込回避

### 2. コマンドライン引数 (`src/args.rs`)

- `--fake-video-capture <FILE>` オプション追加
- ファイル存在チェック
- `--sandstorm` との排他バリデーション

### 3. FakeVideoCapturer の拡張 (`src/fake_video_capturer.rs`)

- `FakeVideoCapturerConfig` に Y4M ファイルパスフィールド追加
- `ImageHolder` に Y4M バリアント追加
- Y4M フレーム → I420Buffer → スケーリング → WebRTC フレーム送出
- Y4M ファイルの解像度と `--resolution` が異なる場合はスケーリング

### 4. main.rs の接続

- `args.fake_video_capture` を `FakeVideoCapturerConfig` に渡す

## C++ 版との差分

- C++ 版は `FILE*` + `fseek` でバイナリ I/O。Rust 版は `std::fs::File` + `std::io::Seek` を使用する
- C++ 版はインターレース非対応 (`Ip` のみ許可) を踏襲する

## 解決方法

- `src/y4m_reader.rs` を新規作成。C++ 版 Y4MReader と同等のロジックを Rust で実装
  - `std::fs::File` + `std::io::{Read, Seek}` でファイル I/O
  - ヘッダパース、フレーム読込、ループ再生、フレームスキップ、同一フレーム重複回避
- `src/args.rs` に `--fake-video-capture` オプションを追加。`--sandstorm` との排他バリデーション付き
- `src/fake_video_capturer.rs` に `ImageHolder::Y4m` バリアントと `tick_y4m` 関数を追加
  - Y4M の I420 データを `I420Buffer` の各プレーンに stride を考慮して行ごとにコピー
  - 出力解像度と Y4M 解像度が異なる場合は `scale_from` でスケーリング
- `src/main.rs` で `y4m_reader` モジュールを追加し、`FakeVideoCapturerConfig` に `y4m_path` を渡す
