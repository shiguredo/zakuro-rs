# CI に pull_request トリガを追加し対象パスを見直す

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-ci-pull-request-trigger
- Polished: {YYYY-MM-DD}

## 目的

公開リポジトリでは、外部貢献者からの PR に対して機械的なチェックが走ることをもってレビューとマージの判断材料にする。現在は PR に対して CI が一切走らず、マージゲートが実質ゼロである。

## 現状

`.github/workflows/ci.yml` の `on` は次の 2 つだけである。

- `push` のみ (トリガ)。`pull_request` は**無し**。`workflow_dispatch` も無し
- `push` には `paths` フィルタがあり、対象は `src/**`、`Cargo.toml`、`Cargo.lock`、`Makefile`、`.github/workflows/ci.yml` の 5 種
- `branches` / `tags` の絞り込みも無い

これによる具体的影響:

1. 外部 PR では fmt / clippy / test のいずれも走らない。fork からの PR に対して維持側が手元でビルド・テストしないと合否を判断できない
2. `paths` がホワイトリスト方式のため、ビルド再現性に直結する `rust-toolchain.toml` と `.cargo/config.toml` の変更が CI を発火させない。`.cargo/config.toml` は DuckDB の prebuilt ダウンロードを有効化する設定であり、壊れても気づけない
3. 同様に `testdata/**` の変更も発火しない。`src/mp4_audio.rs`・`src/y4m_reader.rs`・`src/wav_reader.rs` のテストは `testdata/` の実ファイルを読むため、テストフィクスチャの破損が CI を通ってしまう
4. 同じ組織の公開リポジトリ `shiguredo/hisui` の `ci.yml` は `paths` ではなく `paths-ignore` を使っており、デフォルトで全部流して明らかな対象外だけを除外する設計になっている。zakuro-rs はこの逆になっている

## 設計方針

1. `pull_request` を追加する。`branches` は `develop` に限定する (`CODEBASE.md` の規約で当面すべての作業が `develop` に向かうため)
2. `paths` フィルタは `hisui` に倣って `paths-ignore` 方式へ反転させる。何を除外すべきか (変更がビルド・テスト結果に無影響といえる範囲) を検討し、判断に迷うならフィルタを付けずに全パスへ適用する。ビルド設定 (`rust-toolchain.toml`、`.cargo/config.toml`) と `testdata/**` を必ず対象に含めること
3. `workflow_dispatch` を追加する。定期的な再検証と、schedule 実行の踏み直しで使えるようにする
4. `schedule` (`0 2 * * 1-5`) は現状維持でよい。ただし PR トリガと併用したときに同じブランチが重複実行にならないよう、`concurrency` の導入を `issues/0063` と合わせて検討する
5. 外部貢献者 PR でシークレットが必要な job (`slack_notify`) が落ちないことの確認は `issues/0063` の側で対応する (fork PR では `secrets.*` が空になる)

## 完了条件

- `develop` 向きの PR を作成したときに ci job が走り、fmt / clippy / test の成否が PR 上に見えること
- `rust-toolchain.toml` と `.cargo/config.toml` の変更のみを含む PR で CI が発火すること
- `testdata/**` の変更のみを含む PR で CI が発火すること
- `workflow_dispatch` で手動実行できること
- `push` 時の既存動作 (develop への直接 push で CI が走る) が壊れていないこと

## 変更対象

- `.github/workflows/ci.yml`

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
