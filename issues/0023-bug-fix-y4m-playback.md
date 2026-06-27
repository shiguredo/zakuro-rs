# Y4M 映像再生が完全に動作しないバグを修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
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

`src/fake_video_capturer.rs:107-110` の `has_y4m` 判定を、`take()` 後の `image` 変数に対して直接行うように修正する。

修正前:

```rust
let mut image = match self.image.take() {
    Some(i) => i,
    None => return Ok(()),
};
// ... width, height, fps 等の代入 ...
let has_y4m = self          // ← self.image は既に None
    .image
    .as_ref()
    .is_some_and(|i| matches!(i, ImageHolder::Y4m(..)));
```

修正後:

```rust
let mut image = match self.image.take() {
    Some(i) => i,
    None => return Ok(()),
};
let has_y4m = matches!(image, ImageHolder::Y4m(..));
```

修正は 1 行の置き換えのみ。

### エッジケースと後方互換

- **非 Y4M ケース**: `has_y4m = false` のままなので Sandstorm / Raden 分岐に影響なし
- **`start()` 再呼び出し**: line 90-92 の `handle.is_some()` チェックにより二重起動は防止される。`image` が `None` になった後でも早期リターンするため影響なし
- **Y4M ファイル末尾到達**: 現在の `Y4mReader::get_frame()` 実装はループ再生に対応しており、末尾到達後に先頭に戻って再生を継続する。修正後もこの挙動は変わらない
- **破損 Y4M ファイル**: `FakeVideoCapturer::new()` 内で `Y4mReader::open()` がエラーを返すため、`start()` 到達前にエラー終了する。既存のエラーハンドリングに変更なし
