# CI のワークフロー設定を公開リポジトリ向けに整える

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/update-ci-workflow-settings
- Polished: {YYYY-MM-DD}

## 目的

公開リポジトリの CI は、外部の誰にも実行される前提で最小権限・重複実行の抑制・ビルドキャッシュ・ロックファイル追従を明示する必要がある。さらに zakuro はビルド時に外部 GitHub Releases へ HTTP ダウンロードに行くため、実行の再現性を担保する設定が他のツールより重要になる。

## 現状

`.github/workflows/ci.yml` (67 行、job は `ci` と `slack_notify` の 2 つ) のうち、欠けている設定は以下である。

- トップレベルの `permissions:` が無い。`slack_notify` job だけが `permissions: actions: read` を持つ (`ci.yml` 末尾 job)。job 単位で宣言されていない job は既定のトークンスコープを継承する
- `concurrency:` が無い。`push` と `schedule` が重なると同じコミットで ci job が重複実行される。matrix が ubuntu 2 枚あるため 1 回で 2 job 消費する
- ビルドキャッシュが無い。`actions/cache` も Rust キャッシュ action も使っておらず、`target/` は毎回ゼロからビルドされる。DuckDB prebuilt のダウンロード先 (`target/duckdb-download/`) も毎回取り直しになる
- `cargo clippy` / `cargo test` に `--locked` が付いていない。`Cargo.lock` は追跡済みなので、ロック外の解決に失敗させる設定にできる
- `actions/checkout` はコミットハッシュで固定済み (`actions/checkout@3d3c42e5... # v7.0.1`) だが、`shiguredo/github-actions/.github/actions/slack-notify@main` はブランチ追従のミュータブル参照で、同じファイル内で固定方式が不整合になっている
- `slack_notify` は `secrets.SLACK_WEBHOOK` に依存し、`if: always()` で走る。fork からの PR ではシークレットが空になり、通知 job が红灯を出す可能性がある (PR トリガ追加は `issues/0062`)
- matrix の `ubuntu-24.04-arm` / `ubuntu-26.04-arm` / `macos-26` がコメントアウトされたまま (`ci.yml` の matrix 内コメント)。放置はコメントアウトされた job 定義という残骸になる
- 参考: 同じ組織の公開リポジトリの `ci.yml` を実測したところ、`shiguredo/sora-rust-sdk` と `shiguredo/webrtc-rs` は**トップレベルに `permissions: contents: read` / `actions: read` を持っている** (hisui には無い)。つまりトップレベル permissions は組織内の前例がある設定であり、 zakuro-rs だけが欠けている。`concurrency:` は 3 件とも持っておらず、こちらは一般則としての提案になる。キャッシュは `hisui` が `shiguredo/github-actions/.github/actions/rust-cache@main` を使っており、組織内コンポーネントで実現できる

## 設計方針

1. トップレベルに `permissions: contents: read` を置き、`slack_notify` 側の既存指定と重ねて最小権限にする
2. `concurrency` を追加する。`group` は `workflow` と `ref` から決まる値にして、同じブランチへの連続 push で古い実行をキャンセルする (`cancel-in-progress` は PR 用と push 用で判断を分ける)
3. キャッシュは `hisui` と同じ `shiguredo/github-actions` の `rust-cache` を取り込む。追加できない場合のみ `actions/cache` で `~/.cargo/registry` と `target/` を設計する。DuckDB prebuilt のダウンロード先をキャッシュするかはビルド時間を実測して決める
4. `cargo clippy` と `cargo test` に `--locked` を付ける。付けた状態で `cargo update` 由来の CI 失敗が起きないことを `Cargo.lock` 更新コミットで確認する
5. `slack-notify` はブランチ追従をやめ、`hisui` の運用を確認したうえでコミットハッシュ固定へ寄せる (少なくとも同じファイル内の `checkout` と固定方式を揃える)
6. `slack_notify` はシークレットが未設定の環境で失敗させない。`if` 条件に `secrets.SLACK_WEBHOOK != ''` を加える形で、fork PR でも红灯が出ないようにする
7. コメントアウトされた matrix エントリは、`issues/0064` のツールチェーン固定と合わせて復活可否を判断し、復活できないなら削除して issue 化の判断を残す (コメント残骸を置かない)

## 完了条件

- トップレベル `permissions` が宣言され、各 job の実権限が読み取れること
- 同じブランチへの連続 push で古い CI 実行がキャンセルされることを実測で確認できること
- キャッシュが効き、2 回目以降の ci job の実行時間が短縮されること (before / after を本 issue に記録する)
- `cargo clippy` / `cargo test` が `--locked` 付きで走ること
- `slack_notify` が `SLACK_WEBHOOK` 未設定環境で失敗しないこと
- 使われない matrix エントリのコメントが残っていないこと

## 変更対象

- `.github/workflows/ci.yml`

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
