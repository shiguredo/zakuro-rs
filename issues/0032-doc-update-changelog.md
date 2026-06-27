# CHANGES.md の develop セクションを実装済み全機能で更新する

- Priority: Medium
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/doc-update-changelog
- Polished: 2026-00-00

## 目的

`CHANGES.md` の `## develop` セクションが現状 4 件の変更のみで、多数の実装済み機能が記載されていない問題を解消する。

## 優先度根拠

リリース時に変更履歴が不完全だと、ユーザーが新機能を把握できず、バージョン間の差分が不明確になる。shiguredo-changelog 規約で要求される記載が不足している。

## 現状

```markdown
## develop

- [CHANGE] CLI 引数 `--fake-video-capture` を `--input-y4m` にリネームする
- [ADD] DuckDB ファイルへの統計情報出力
- [ADD] CLI 引数 `--input-wav` で WAV ファイルを音声入力としてループ再生
- [ADD] 複数 Zakuro インスタンス起動 (JSONC `instances` 配列と `--instance-hatch-rate`)
```

以下の実装済み機能が未記載:
- HTTP サーバー (`--http-host`, `--http-port`)
- JSON-RPC 2.0 API (`GET /.ok`, `POST /rpc`, `GetVersion`)
- JSONC 設定ファイル (`--config`)
- フェイク映像生成 (Raden)
- 砂嵐映像 (`--sandstorm`)
- Y4M 映像入力 (`--input-y4m`, 表記上は CHANGE として記載済みだが ADD としての記載も必要)
- MP4 パススルー送信 (`--input-mp4`)
- 実デバイスキャプチャ (`--video-input-device`)
- DataChannel メッセージング (`--sora-data-channels`)
- シナリオ機能 (`--scenario reconnect`)
- 統計収集・定期レポート
- OpenH264 エンコード (`--openh264`)
- NopVideoDecoder (受信映像廃棄)
- mTLS (`--client-cert`, `--client-key`)
- TLS 証明書検証スキップ (`--insecure`)
- サイマルキャスト / スポットライト対応

## 設計方針

shiguredo-changelog スキルの規約に従い、`[ADD]` / `[CHANGE]` の分類で全実装済み機能を追記する。公開 API の後方互換に影響する `[CHANGE]` があれば明記する。

## 完了条件

- `## develop` セクションに全実装済み機能が記載されていること
- shiguredo-changelog のフォーマット規約に準拠していること
- 各項目に `@voluntas` のクレジットが付与されていること

## 解決方法

`docs/ZAKURO.md` の「実装状況」チェックリストを参照し、`[x]` の全項目を `[ADD]` として追記する。破壊的変更 (`--fake-video-capture` → `--input-y4m`) は既に `[CHANGE]` として記載済み。
