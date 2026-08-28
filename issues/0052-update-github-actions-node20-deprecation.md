# GitHub Actions の Node.js 20 非推奨警告に対応する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/update-ci-node20
- Polished: {YYYY-MM-DD}
- Reporter: @voluntas

## 目的

CI 実行時に GitHub が出力する「Node.js 20 is deprecated」警告 (actions/checkout@v4 が Node.js 20 で実行されるため) を解消する。

## 現状

`.github/workflows/ci.yml` は `actions/checkout@v4` を使用している。GitHub の runner が Node.js 24 へ移行しつつあり、Node.js 20 で実行される action に非推奨警告の annotation が出る。2026-08-28 の CI 実行 (develop への push) で `CI (ubuntu-24.04)` / `CI (ubuntu-26.04)` の両ジョブに警告が出ることを確認した。CI 自体は成功するが、静かな非推奨の蓄積であり、GitHub 側の強制移行時に壊れる可能性がある。

## 設計方針

- `shiguredo-github-actions` スキルの規約 (外部 action の選定・コミットハッシュ固定 + バージョンコメント) に従う
- `/update-actions` スキルの手順で actions/checkout の最新リリースとコミットハッシュを調査し、Node.js 24 対応版へ更新する
- 他の action (`actions/checkout` 以外) に同種の警告が出ていないかも合わせて確認する

## 完了条件

- 新規の CI 実行 (push) で Node.js 非推奨警告の annotation が出ないこと
- `cargo fmt` / `cargo clippy` / `cargo test` を含む CI が引き続き成功すること

## 変更対象

- `.github/workflows/ci.yml` (`actions/checkout@v4` の参照先)