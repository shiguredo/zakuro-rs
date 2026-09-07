# Dependabot の設定をリポジトリに追加し、停滞ブランチを整理する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-dependabot-config
- Polished: {YYYY-MM-DD}

## 目的

zakuro は依存の更新がそのままビルド可能性に直結する構成を持つ (ネイティブライブラリをビルド時にダウンロードする)。自動更新の仕組みがリポジトリ内に定義されていないと、公開後に誰が依存更新を起こすかが不明になる。

## 現状

- `.github/dependabot.yml` は存在しない。`renovate` 系の設定ファイルも存在しない
- 同じ組織の公開リポジトリ `shiguredo/hisui` は `.github/dependabot.yml` を持つ (`sora-rust-sdk` と `webrtc-rs` は持たない。組織内でも分散している)
- 一方で Dependabot は過去に動作した痕跡がある
  - `git log --all --author=dependabot` は 2 コミットを返す (`Bump rand from 0.8.5 to 0.8.6` 2026-04-22、`Bump rustls-webpki from 0.103.10 to 0.103.13` 2026-04-24)
  - `origin/dependabot/cargo/rand-0.8.6` と `origin/dependabot/cargo/rustls-webpki-0.103.13` がリモートに残り、いずれも develop に未マージで 1 コミットのまま放置されている
- つまり設定が組織レベルの既定設定で行われているか、設定が削除されたかどちらかであり、リポジトリを読んでも再現できない状態になっている
- Cargo.lock に解決される間接依存は 45 件以上あり (`cargo update` 時の "unchanged dependencies behind latest" 表示より)、うちどれを自動追従させるかの方針が無い

## 設計方針

1. まず現状を確定させる。GitHub のリポジトリ設定 (Dependencies / Dependabot) と組織レベルの既定設定を確認し、「設定が無いのに動いていた」のか「組織既定に依存している」のかを特定する。公開後も組織既定に依存するかは判断が要る (組織設定は公開リポジトリにも効くため)
2. 未マージの dependabot ブランチ 2 本を片付ける。`rand 0.8.6` と `rustls-webpki 0.103.13` が現在の `Cargo.lock` で既に解決済みならブランチは不要なので削除する。未解決なら内容を取り込み、放置しない
3. `.github/dependabot.yml` をリポジトリに置く場合は `hisui` の設定を読み、間隔・グループ化・reviewers の書き方を揃える
4. 更新範囲は「patch と minor のみ」を既定とし、major は別 issue で扱う (`update-deps` 相当の作業は人がやる運用ならその旨を書く)
5. `shiguredo_webrtc` と `sora_sdk` の canary 依存は、プレリリース追従を自動更新に混ぜない (`update-deps` の規約でもプレリリースは最新として扱わない)。除外設定か、別ディレクトリの明示的な pin として扱う
6. 自動更新が通るには CI が PR で走る必要がある。`issues/0062` が前提になる

## 完了条件

- Dependabot の設定がどこで定義されているか (リポジトリ or オーグ既定) が確定し、リポジトリを読むだけで再現できる状態になっていること
- 未マージの dependabot ブランチが 0 本であること
- patch / minor の自動更新 PR が CI を通過する仕組みが動いていること (`issues/0062` 前提)
- canary 依存が自動更新でプレリリースへ引きずられないことが設定または文書で明確になっていること

## 変更対象

- `.github/dependabot.yml` (新規、または組織既定に依存すると決めた場合は運用の記録)
- `origin/dependabot/*` ブランチの削除 (リモート操作)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
