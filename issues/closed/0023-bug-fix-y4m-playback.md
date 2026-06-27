# Y4M 映像再生が完全に動作しないバグを修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-06-28
- Model: DeepSeek V4 Pro
- Branch: feature/fix-y4m-playback
- Polished: 2026-06-28

## 目的

`--input-y4m` で Y4M ファイルを指定しても映像が再生されない致命的なバグを修正する。現在 Y4M ファイル指定時は `has_y4m` が常に `false` になるためスレッド内の全分岐（Y4M / Sandstorm / Raden）がスキップされ、一切の映像フレームが WebRTC に送信されない空転ループになっている。

## 優先度根拠

`--input-y4m` のプレースホルダだった `--fake-video-capture` からリネームして正式な機能として提供しているにもかかわらず、実際には動作していない。ユーザーから見ると全く機能しないオプションであり最優先で修正する必要がある。

## 現状

`src/fake_video_capturer.rs:98-110` の `FakeVideoCapturer::start` 内で、`self.image.take()` で `ImageHolder` の所有権を移動した後に `self.image.as_ref()` で Y4M 判定を行っている:

```rust
let mut image = match self.image.take() { // ← ここで self.image は None になる
    Some(i) => i,
    None => return Ok(()),
};
let has_y4m = self
    .image                   // ← 常に None
    .as_ref()
    .is_some_and(|i| matches!(i, ImageHolder::Y4m(..)));
```

`has_y4m` が常に `false` になるため、line 119 の `if has_y4m` ブロックは絶対に実行されず、Y4M 再生が機能しない。

## 設計方針

`take()` で取得した `image` 変数に対して直接 `matches!` で Y4M 判定を行う方式を採用する。`self.image` に再アクセスする必要がなく、判定位置の前後関係に依存しないため堅牢である。`self.image.as_ref()` を使う方式（`take()` の前に判定を移動する）も可能だが、判定後に `take()` を実行する間にコードが追加されるとバグ再発のリスクがあるため採用しない。

### 留意点

修正により `tick_y4m` 関数が初めて実行されるため、`get_frame()` が要求するバッファサイズ（`Y4mReader::frame_size()`）と `FakeVideoCapturer::new()` で確保しているバッファサイズ（`W * H * 3 / 2`）が一致するかを確認する。奇数次元の解像度では `W * H * 3 / 2` の整数除算によりバッファ不足が発生する可能性がある。必要に応じて `reader.frame_size()` でバッファを確保するように修正する。

## 完了条件

### 動作条件

- Y4M ファイルを `--input-y4m` で指定して起動した場合に、`tick_y4m` 関数が呼ばれ I420 フレームが WebRTC に送信されること
- Y4M のフレームレートと config の `fps` が異なる場合のフレームスキップ挙動が既存の Raden / Sandstorm の fps 制御と矛盾しないこと（現在のループは全分岐で `1000 / fps` ms の sleep 周期であり共通）
- 非 Y4M ケース（Raden / Sandstorm）の動作が修正前後で変わらないこと
- `start()` の再呼び出し時（`handle.is_some()` の早期リターン、または `image` 消費済みによる `return Ok(())`）の動作が変わらないこと
- `tick_y4m` 内の I420 バッファコピー処理（行ごとの stride コピー、Y/U/V プレーン分離、スケーリング分岐）に既知の問題がないことを確認し、問題があれば修正すること
- `FakeVideoCapturer::new()` のバッファ確保サイズが `Y4mReader::frame_size()` の返すサイズと一致すること（奇数次元の解像度でのバッファ不足に注意）

### テスト条件

- `has_y4m` 判定ロジックを独立した関数（例: `fn is_y4m(image: &ImageHolder) -> bool`）に切り出し、`src/fake_video_capturer.rs` の `#[cfg(test)]` で単体テストすること
- テストでは `ImageHolder::Y4m` / `ImageHolder::Raden` / `ImageHolder::Sandstorm` の全バリアントで正しい判定結果を検証する
- テスト用の Y4M データは `tempfile` で `YUV4MPEG2 W{n} H{n} F30:1 Ip C420\nFRAME\n` + I420 raw データ（`W * H + 2 * ceil(W/2) * ceil(H/2)` バイト）を生成して使用する

## 解決方法

以下の修正を実施した。

### 1. `has_y4m` 判定バグの修正

`src/fake_video_capturer.rs` の `start()` 内で `self.image.take()` 後に `self.image.as_ref()` で Y4M 判定を行っていたバグを修正。`take()` 後のローカル変数 `image` に対して `is_y4m(&image)` を呼ぶ方式に変更した。

### 2. バッファサイズのバグ修正

`FakeVideoCapturer::new()` 内の I420 バッファ確保サイズを `W * H * 3 / 2`（整数除算で奇数次元では不足）から `Y4mReader::frame_size()` に変更した。`frame_size()` は `ceil(W/2) * ceil(H/2) * 2 + W * H` で正しいサイズを計算する。

### 3. `is_y4m` 関数の抽出

`ImageHolder` が Y4m バリアントかどうかを判定する `fn is_y4m(image: &ImageHolder) -> bool` を抽出し、単体テスト可能にした。

### 4. `frame_size()` の可視性変更

`Y4mReader::frame_size()` を `pub(crate)` に変更し、`FakeVideoCapturer` から呼べるようにした。

### 5. テスト追加

- `test_is_y4m_all_variants`: Y4m / Raden / Sandstorm 全バリアントの `is_y4m` 判定テスト
- `test_odd_dimension_buffer_size`: 奇数次元 (641x481) で `frame_size()` が `W*H*3/2` より大きいことの検証テスト

### 変更ファイル

- `src/fake_video_capturer.rs`: `has_y4m` 判定修正、バッファサイズ修正、`is_y4m` 関数追加、テスト追加
- `src/y4m_reader.rs`: `frame_size()` を `pub(crate)` に変更

### エッジケースと後方互換

- **非 Y4M ケース**: `has_y4m = false` のままなので Sandstorm / Raden 分岐に影響なし
- **`start()` 再呼び出し**: `handle.is_some()` チェックにより二重起動は防止されるため影響なし
- **Y4M ファイル末尾到達**: `Y4mReader::get_frame()` のループ再生に変更なし
- **破損 Y4M ファイル**: `Y4mReader::open()` のエラーハンドリングに変更なし
