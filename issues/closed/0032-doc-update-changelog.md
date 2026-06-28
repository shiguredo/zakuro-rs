# CHANGES.md の develop セクションを実装済み機能で更新する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-06-28
- Model: DeepSeek V4 Pro
- Branch: feature/doc-update-changelog
- Polished: 2026-06-28

## 目的

`CHANGES.md` の `## develop` セクションが現状 4 件の変更のみで、多数の実装済み機能が記載されていない問題を解消する。`docs/ZAKURO.md` の実装状況チェックリスト (`[x]`) を参照し、ユーザー視点で重要な機能を追記する。

## 優先度根拠

リリース時に変更履歴が不完全だと、ユーザーが新機能を把握できず、バージョン間の差分が不明確になる。

## 現状

CHANGES.md の `## develop` セクションには 4 件のみ記載:

```markdown
- [CHANGE] CLI 引数 `--fake-video-capture` を `--input-y4m` にリネームする
- [ADD] DuckDB ファイルへの統計情報出力
- [ADD] CLI 引数 `--input-wav` で WAV ファイルを音声入力としてループ再生
- [ADD] 複数 Zakuro インスタンス起動 (JSONC `instances` 配列と `--instance-hatch-rate`)
```

## 設計方針

shiguredo-changelog の規約に従い、`[CHANGE]` → `[ADD]` → の順で追記する。ユーザーが直接利用する機能を `[ADD]` として、内部インフラや開発者向けの項目は `### misc` として記載する。

追記対象は ZAKURO.md の実装状況 `[x]` 項目のうち、ユーザー視点で意味のあるものを選別する。既存の 4 件との重複を避ける。

Y4M 映像入力は既存の `[CHANGE]` エントリでカバー済みのため、重複する `[ADD]` は追加しない。

## 完了条件

- `## develop` セクションにユーザー向けの全実装済み機能が記載されていること
- エントリが `[CHANGE]` → `[ADD]` の順で整列されていること
- shiguredo-changelog のフォーマット規約に準拠していること
- 各項目に `@voluntas` のクレジットが付与されていること

## 解決方法

`CHANGES.md` の `## develop` セクションに、`docs/ZAKURO.md` の実装状況 `[x]` 項目のうち CHANGES.md に未記載だった機能を追記した。

### [ADD] 追加 (10 件)
- HTTP API サーバー
- JSONC 設定ファイル
- フェイク映像生成 (Raden / 砂嵐)
- Y4M / MP4 / 実デバイスキャプチャ
- DataChannel メッセージング / シナリオ
- OpenH264 / NopVideoDecoder
- コーデック / 解像度 / フレームレート / ビットレート
- mTLS / insecure
- サイマルキャスト / スポットライト / リトライ / Duration / repeat-interval

### misc [UPDATE] 追加
- Ctrl+C グレースフルシャットダウン / 統計収集 / フェイク音声ビープ音

### 変更ファイル
- `CHANGES.md`
