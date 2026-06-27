# Cargo.toml のプロジェクトメタデータと CI 設定の誤りを修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-06-28
- Model: DeepSeek V4 Pro
- Branch: feature/fix-project-metadata-and-ci
- Polished: 2026-06-28

## 目的

Cargo.toml のプロジェクトメタデータが他プロジェクト (hisui) のままになっている問題と、CI ワークフローの slack_notify ジョブに存在する設定誤りを修正する。

## 優先度根拠

- Cargo.toml の `description` / `homepage` / `repository` が誤ったプロジェクト (hisui) を指しており、パッケージメタデータとして不正
- CI の `slack_notify` ジョブが存在しない action バージョン (`actions/checkout@v6`) と無効なランナー (`ubuntu-slim`) を使用している

## 現状

### Cargo.toml:6,8,9

```toml
description = "Recording Composition Tool Hisui"
homepage = "https://github.com/shiguredo/hisui"
repository = "https://github.com/shiguredo/hisui"
```

### .github/workflows/ci.yml

メイン `ci` ジョブは `runs-on: ${{ matrix.os }}` → `ubuntu-24.04` で正常に動作している。問題箇所は `slack_notify` ジョブ:

```yaml
# line 31: v6 は存在しない（正しくは v4）
- uses: actions/checkout@v6

# line 47: ubuntu-slim は有効なランナーではない
runs-on: ubuntu-slim
```

## 設計方針

1. Cargo.toml のメタデータを zakuro の正しい情報に修正する
2. CI の `actions/checkout` を `v4` に修正する
3. CI の `slack_notify` ジョブの `runs-on` を `ubuntu-latest` に修正する
4. CI に `cargo metadata` によるメタデータ検証ステップを追加し、再発を防止する

## 完了条件

- Cargo.toml の `description` / `homepage` / `repository` が zakuro の正しい値に修正されていること
- CI の `slack_notify` ジョブが正常に起動すること
- `cargo metadata` の検証が CI パイプラインに組み込まれていること

## 解決方法

### Cargo.toml

- `description` を `"Recording Composition Tool Zakuro"` に修正
- `homepage` を `"https://github.com/shiguredo/zakuro-rs"` に修正
- `repository` を `"https://github.com/shiguredo/zakuro-rs"` に修正

### .github/workflows/ci.yml

- `ci` ジョブの `actions/checkout@v6` → `actions/checkout@v4` に修正 (v6 は存在しない)
- `slack_notify` ジョブの `runs-on: ubuntu-slim` → `runs-on: ubuntu-latest` に修正 (ubuntu-slim は無効なランナー)
- `cargo metadata` による description 検証ステップを追加し、メタデータの再発防止を組み込んだ

### 変更ファイル

- `Cargo.toml`
- `.github/workflows/ci.yml`
