## `--fake-video-capture` を `--input-y4m` にリネームする

- Priority: Medium
- Created: 2026-06-26
- Completed: 2026-06-26
- Branch: feature/change-input-y4m
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

`--fake-video-capture` という CLI 引数名は zakuro (C++) から引き継いだ命名だが、引数名から機能 (Y4M 動画ファイルを映像入力に使う) が直感的に分からない。既に `--input-mp4` が導入されており、入力ソース系の CLI 引数は `--input-{形式}` で統一されつつある。`--fake-video-capture` を `--input-y4m` にリネームし、命名規則を統一する。

## 優先度根拠

- Medium。利用者からの命名改善要望に基づく
- 機能的バグではないが、CLI 名称は一度公開すると変更コストが上がるため早期に整理する
- 既存ユーザーへの影響はあるが、旧名称指定時は noargs のデフォルト未知引数エラーで気付けるため移行コストは低い

## 現状

- `src/args.rs` で `noargs::opt("fake-video-capture")` として定義 (`fake_video_capture: Option<String>`)
- `src/main.rs` で `FakeVideoCapturerConfig.y4m_path` に渡している
- `README.md` / `docs/ZAKURO.md` に `--fake-video-capture` の記載あり
- `--sandstorm` / `--video-input-device` / `--input-mp4` との排他バリデーションが定義されている
- ヘルプ文 (`Y4M 動画ファイルからフェイク映像を生成する`) は「生成」という表現が raden 描画機能と紛らわしい

zakuro (C++) では `--fake-video-capture` のままだが、zakuro-rs は機能互換を維持しつつ CLI 名称は改善してよいと判断する。

## 設計方針

- `--fake-video-capture` を完全削除し、`--input-y4m` に置き換える (旧名称のエイリアスは残さない)
- 旧名称を渡された場合は noargs のデフォルト未知引数エラーに任せる。追加の移行案内ロジックは入れない
- struct フィールド名・JSON フィールド名も `input_y4m` / `input-y4m` に揃える
- 排他バリデーションのエラーメッセージも新名称に更新
- ヘルプ文を「Y4M ファイルを映像入力として再生する」のように、生成ではなく再生と分かる表現に改める

## 完了条件

- `--fake-video-capture` という文字列がコードベース・ドキュメントから完全に消えている
- `--input-y4m <path>` で Y4M ファイルが従来通り再生できる
- 排他バリデーションと存在チェックが新名称で機能する
- `cargo test`, `cargo clippy`, `cargo fmt --check` が通る

## 解決方法

### 1. `src/args.rs`

- `fake_video_capture` フィールドを `input_y4m` にリネーム
- `noargs::opt("fake-video-capture")` を `noargs::opt("input-y4m")` にリネーム
- ヘルプ doc を「Y4M ファイルを映像入力として再生する」に変更
- ファイル存在チェックのエラーメッセージを `input-y4m: file not found` に更新
- 排他バリデーションのメッセージを `--sandstorm` / `--video-input-device` / `--input-mp4` の 3 箇所で新名称に更新
- JSONC 由来の引数群でも `--input-y4m` がそのまま受理される (キー名共有のため追加変更なし)

### 2. `src/main.rs`

- `instance.fake_video_capture` を `instance.input_y4m` に変更

### 3. ドキュメント

- `README.md` の使用例・引数一覧表・排他制約 2 箇所を新名称に更新
- `docs/ZAKURO.md` の実装状況リスト 1 箇所を新名称に更新 (zakuro 互換 CLI 一覧の箇所は原典のため未変更)

### 4. CHANGES.md

- `[CHANGE] CLI 引数 `--fake-video-capture` を `--input-y4m` にリネームする` を追記

### 5. 確認

- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` (16 件) が全てパス
