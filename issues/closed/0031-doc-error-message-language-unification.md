# エラーメッセージの言語を日本語に統一する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-06-28
- Branch: feature/doc-error-message-language-unification
- Polished: 2026-06-28

## 目的

`src/y4m_reader.rs` と `src/wav_reader.rs` の `ErrorMessage::new()` に渡す文字列を、他のファイルと同様に日本語に統一する。

`ErrorMessage` はユーザーに直接表示されるエラー文字列であり、CLAUDE.md の「常に日本語を利用すること」の対象である。アプリケーション内部のログメッセージ（CLAUDE.md「ログメッセージは全て英語」）とは区別される。

## 優先度根拠

同一プロジェクト内で `ErrorMessage::new()` に渡す文字列の言語が不統一だと、エラー発生時の表示が日本語と英語の混在になり混乱を招く。

## 現状

### 言語状況

- `src/y4m_reader.rs`: ErrorMessage 30 箇所（format! 動的生成含む）が英語
- `src/wav_reader.rs`: ErrorMessage 30 箇所（format! 動的生成含む）が英語
- `src/args.rs`: 日本語メッセージが多数（例: `"〜が不正です"`、`"〜は〜で指定してください"`）。一部英語も混在
- `src/main.rs`: 一部日本語、一部英語が混在

### テスト破壊

wav_reader.rs 内の以下のテストが英語のエラーメッセージ文字列を直接アサートしており、翻訳で破壊される:
- `wav_reader.rs:332`: `format!("{err}").contains("audio format")`
- `wav_reader.rs:349`: `format!("{err}").contains("bits per sample")`

翻訳時にこれらのアサーションも日本語に更新する必要がある。

## 設計方針

- `ErrorMessage::new()` の文字列を日本語に変更する。コード内の識別子・変数名・コメントは変更しない
- 既存の日本語メッセージ（args.rs）の文体に合わせる: 状態説明ではなく「〜が不正です」「〜が不足しています」のパターンを使用する
- `format!` を含む動的メッセージも日本語化し、変数部分は `{e}` や `{value}` のままとする（埋め込み値は OS 提供等により英語のまま）
- テストの expect メッセージは「テストのログメッセージは全て日本語にすること」に従い日本語のまま維持する
- `y4m_reader.rs:96,110,118` および `wav_reader.rs:82` 等の全箇所を翻訳対象とする

## 完了条件

- `y4m_reader.rs` と `wav_reader.rs` の全 `ErrorMessage::new()` の文字列が日本語に統一されていること
- 破壊されるテスト（wav_reader.rs:332,349）の expect メッセージが更新されていること
- `cargo test` が全テスト通過すること

## 解決方法

`src/y4m_reader.rs` と `src/wav_reader.rs` の全 `ErrorMessage::new()` の文字列を英語から日本語に翻訳した。

- y4m_reader.rs: 28 箇所のエラーメッセージを日本語化
- wav_reader.rs: 16 箇所のエラーメッセージを日本語化 + テストアサーション 2 箇所更新

### 変更ファイル

- `src/y4m_reader.rs`
- `src/wav_reader.rs`
