# CHANGES.md の develop セクションを実装済み機能で更新する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-00-00
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

`docs/ZAKURO.md` の「実装状況」チェックリスト (`[x]`) を参照し、以下のカテゴリに従って追記する:

### [ADD] として追加（ユーザー向け機能）

- HTTP API サーバー (`--http-host`, `--http-port` / ヘルスチェック・JSON-RPC 2.0)
- JSONC 設定ファイル (`--config`)
- フェイク映像生成 (Raden / 砂嵐 `--sandstorm`)
- MP4 パススルー送信 (`--input-mp4`)
- 実デバイスキャプチャ (`--video-input-device`)
- DataChannel メッセージング (`--sora-data-channels`)
- シナリオ機能 (`--scenario reconnect`)
- OpenH264 エンコード (`--openh264`)
- NopVideoDecoder (受信映像廃棄)
- コーデック指定 (VP8/VP9/AV1/H264/H265)
- 解像度指定 / フレームレート指定
- 映像/音声ビットレート指定
- mTLS (`--client-cert`, `--client-key`) / TLS 証明書検証スキップ (`--insecure`)
- サイマルキャスト / スポットライト対応
- リトライロジック (`--max-retry`, `--retry-interval`)
- Duration / repeat-interval

### ### misc として追加（内部機能）

- Ctrl+C グレースフルシャットダウン
- シナリオ統計収集・定期レポート
- フェイク音声ビープ音生成
- vcs-hatch-rate (仮想クライアント段階的起動)
