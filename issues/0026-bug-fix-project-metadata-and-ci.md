# Cargo.toml のプロジェクトメタデータと CI 設定の誤りを修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
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

Cargo.toml の 3 行を以下の値に修正する:

```toml
description = "Recording Composition Tool Zakuro"
homepage = "https://github.com/shiguredo/zakuro"
repository = "https://github.com/shiguredo/zakuro"
```

`.github/workflows/ci.yml` の `slack_notify` ジョブ内:

```yaml
# line 31: v6 → v4
- uses: actions/checkout@v4

# line 47: ubuntu-slim → ubuntu-latest
runs-on: ubuntu-latest
```

CI のメイン `ci` ジョブに cargo metadata 検証を追加:

```yaml
- name: Verify Cargo metadata
  run: |
    DESCRIPTION=$(cargo metadata --format-version=1 --no-deps | jq -r '.packages[] | select(.name == "zakuro") | .description')
    if [ "$DESCRIPTION" != "Recording Composition Tool Zakuro" ]; then
      echo "::error::Cargo.toml description mismatch: $DESCRIPTION"
      exit 1
    fi
```
