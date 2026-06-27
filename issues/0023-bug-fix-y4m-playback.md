# Y4M 映像再生が完全に動作しないバグを修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/fix-y4m-playback
- Polished: 2026-00-00

## 目的

`--input-y4m` で Y4M ファイルを指定しても映像が再生されない致命的なバグを修正する。現在 Y4M ファイル指定時に砂嵐または Raden 描画にフォールバックしている。

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

`has_y4m` の判定を `self.image.take()` より前に行う。または `take()` で取得した `image` 変数に対してパターンマッチで分岐する。

具体的には line 98-101 の `take()` を line 107-110 の `has_y4m` 判定より後ろに移動する。`as_ref()` で読むだけなら `take()` の前に判定しても問題ない。

## 完了条件

- Y4M ファイルを `--input-y4m` で指定した場合に、ファイルの映像フレームが正しく WebRTC に送信されること
- Y4M ファイル指定時でも砂嵐・Raden 描画にはフォールバックしないこと

## 解決方法

`src/fake_video_capturer.rs` の `start()` メソッド内で、以下の順序に修正する:

1. 先に `has_y4m` の判定を行う（`self.image.as_ref().is_some_and(...)`）
2. その後で `self.image.take()` を実行する

または `take()` で取得した値に対して直接 `matches!` で判定する方式に変更する。
