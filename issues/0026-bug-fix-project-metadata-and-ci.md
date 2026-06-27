# Cargo.toml のプロジェクトメタデータと CI 設定の誤りを修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/fix-project-metadata-and-ci
- Polished: 2026-00-00

## 目的

Cargo.toml のプロジェクトメタデータが他プロジェクト (hisui) のままになっている問題と、CI ワークフローが壊れている問題を修正する。

## 優先度根拠

- Cargo.toml の `description` / `homepage` / `repository` が誤ったプロジェクト (hisui) を指しており、パッケージメタデータとして完全に不正
- CI ワークフローが存在しない action バージョン (`actions/checkout@v6`) とランナー (`ubuntu-slim`) を使用しており、CI が全く動作しない

## 現状

### Cargo.toml:6,8,9

```toml
description = "Recording Composition Tool Hisui"
homepage = "https://github.com/shiguredo/hisui"
repository = "https://github.com/shiguredo/hisui"
```

### .github/workflows/ci.yml

```yaml
# line 31: v6 は存在しない
- uses: actions/checkout@v6

# line 47: ubuntu-slim は有効なランナーではない
runs-on: ubuntu-slim
```

## 設計方針

1. Cargo.toml のメタデータを zakuro の正しい情報に修正する
2. CI の `actions/checkout` を `v4` に修正する
3. CI の `ubuntu-slim` を `ubuntu-latest` 等の有効なランナーに修正する

## 完了条件

- `cargo metadata` で zakuro に関する正しい情報が表示されること
- CI ワークフローが GitHub Actions で正常に起動すること

## 解決方法

Cargo.toml の 3 行を zakuro 用に修正する。CI の action バージョンとランナーを修正する。
