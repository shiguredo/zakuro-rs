# エラーメッセージの言語を日本語に統一する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/doc-error-message-language-unification
- Polished: 2026-00-00

## 目的

`src/y4m_reader.rs` と `src/wav_reader.rs` のエラーメッセージを、他のファイルと同様に日本語に統一する。

## 優先度根拠

同一プロジェクト内で `ErrorMessage::new()` に渡す文字列の言語が不統一だと、エラー発生時のログが日本語と英語の混在になり混乱を招く。CLAUDE.md の「常に日本語を利用すること」にも反する。

## 現状

- `src/args.rs`、`src/main.rs`、`src/data_channel.rs`: エラーメッセージは日本語
- `src/y4m_reader.rs`: エラーメッセージは英語 (例: `"Y4M: time went backwards"`, `"Y4M: empty file"`)
- `src/wav_reader.rs`: エラーメッセージは英語 (例: `"WAV file too short"`, `"WAV file missing RIFF header"`)

## 設計方針

両ファイルの全 `ErrorMessage::new()` の文字列を日本語に変更する。コード内の識別子・変数名・コメントは変更しない。

## 完了条件

- `y4m_reader.rs` と `wav_reader.rs` のエラーメッセージが日本語に統一されていること
- 既存のテストが通過すること（テストの expect メッセージも必要に応じて更新）

## 解決方法

各 `ErrorMessage::new("English message")` を `ErrorMessage::new("日本語のメッセージ")` に置き換える。

| ファイル | 現状 (英語) | 修正後 (日本語) |
|---------|------------|---------------|
| y4m_reader.rs:96 | `"Y4M: time went backwards"` | `"Y4M: 時間が巻き戻っています"` |
| y4m_reader.rs:110 | `"Y4M: buffer too small"` | `"Y4M: バッファが不足しています"` |
| y4m_reader.rs:118 | `"Y4M: read past end of file"` | `"Y4M: ファイルの終端を超えて読み取りました"` |
| wav_reader.rs:82 | `"WAV file too short"` | `"WAV ファイルが短すぎます"` |
| (他 10 箇所程度) | — | — |
