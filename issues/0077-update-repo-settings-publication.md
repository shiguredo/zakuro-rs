# GitHub リポジトリ設定を OSS 公開向けに整える

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/update-repo-settings-publication
- Polished: {YYYY-MM-DD}

## 目的

公開前の穴はリポジトリ内のファイルだけではない。GitHub のリポジトリ設定 (description・fork 可否・Issues の有効無効・ブランチ保護・セキュリティ設定) も外部から見える顔であり、アクセス制御に直結する。組織の既存公開リポジトリと実測で差分を取り、乖離だけを潰す。

## 現状

`gh api repos/shiguredo/{zakuro-rs,hisui,sora-rust-sdk,webrtc-rs}` と `gh api repos/.../branches/develop/protection` で実測した差分 (2026-09-07 時点)。

| 設定 | zakuro-rs | hisui | sora-rust-sdk | webrtc-rs | 判定 |
|---|---|---|---|---|---|
| `description` | 当時 `null` | `Recording Composition Tool Hisui` | `Sora Rust SDK` | `Rust bindings for libwebrtc` | **乖離 (修正済み、下記)** |
| `allow_forking` | `false` | `true` | `true` | `true` | **乖離 (現状変更不能、下記)** |
| `license` | 当時 `null` | Apache-2.0 | Apache-2.0 | Apache-2.0 | `issues/0059` で解決済み |
| `has_issues` | `false` | `false` | `false` | `false` | 組織慣習。据え置き |
| `topics` | `[]` | `[]` | `[]` | `[]` | 組織慣習 (誰も設定していない)。据え置き |
| ブランチ保護 (`develop`) | 無し | 無し | 無し | 無し | 組織慣習。CI をマージゲートにする仕組みは `issues/0062` で扱う |
| `secret_scanning` | `disabled` | `disabled` | `disabled` | `disabled` | 組織慣習。据え置き |
| `dependabot_security_updates` | `enabled` | - | - | - | 後述 (`issues/0069` に関係) |
| マージ方式 3 種 / `allow_auto_merge` / `delete_branch_on_merge` | 一致 | 一致 | 一致 | 一致 | 乖離なし |
| `has_wiki` / `has_projects` / `has_discussions` / `has_pages` | 一致 (すべて false) | 同 | 同 | 同 | 乖離なし |
| `homepage` | 未設定 | 未設定 | 未設定 | 未設定 | 組織慣習。設定しない |
| `visibility` / `default_branch` | `private` / `develop` | `public` / `develop` | `public` / `develop` | `public` / `develop` | 反転はリリースの段取り側で判断 |

### description (修正済み)

- `Cargo.toml` の `[package] description` は `"Recording Composition Tool Zakuro"` であり、CI の `Verify Cargo metadata` ステップが同じ文字列を要求している
- `gh api -X PATCH` で `description` を同じ値に設定した。設定後の `gh api` 返り値で反映を確認済み

### allow_forking (現状は変更できない)

- `gh api -X PATCH repos/shiguredo/zakuro-rs -f allow_forking=true` は HTTP 422 `This organization does not allow private repository forking` で reject された
- つまり **非公開リポジトリである間は組織ポリシーにより fork 不可**であり、兄弟リポジトリが fork 可能なのは公開されているからに過ぎない
- visibility を public に反転させた時点で解除される見込みだが、自動で true になるかは実測が必要である。fork できない公開リポジトリは OSS として外部貢献を受け付けられないため、反転後に必ず確認する

### Dependabot の事実が一部判明した

- `security_and_analysis.dependabot_security_updates` は `enabled` である。`issues/0069` が指摘する「`.github/dependabot.yml` が無いのに dependabot ブランチが存在する」状態は、リポジトリ単位でセキュリティ更新が有効になっていることと無関係ではない。事実は `issues/0069` 側に追記した

## 設計方針

1. 組織の公開リポジトリで**誰も設定していない項目を新たに作り込まない** (topics、branch protection、secret scanning、SECURITY.md 系は前例がないため本作らない)。差分があるものだけを揃える
2. GitHub 側の設定変更はファイルと同じく「実測 → 変更 → 実測で反映確認」の手順で進める。API の返り値で完結させる
3. 組織ポリシーで変更が拒否される項目は無理に回らない (別経路の設定変更は行わない)。いつ変更可能になるかの条件を本 issue に記録して残す
4. visibility の反転自体は本 issue のスコープに含めない。反転の段取りは `issues/0061` の 現状 が前提とする履歴の扱いと絡むため、別途判断する

## 完了条件

- `description` が `Cargo.toml` の `description` と同一文字列になっていること (**達成済み**)
- 組織の公開リポジトリと差分がある設定項目をすべて列挙し、揃えたもの / 組織慣習として据え置いたもの / 変更できなかったものを区別して記録できていること (**達成済み**)
- visibility を public に反転した後、`allow_forking` が `true` になっていること。false のままなら公開リポジトリとして fork を許可する設定に変更すること (**未達。反転後のみ検証可能**)
- 反転後に License / CI の表示が崩れていないことの確認は `issues/0068` が扱う (本 issue では再確認しない)

## 変更対象

- GitHub リポジトリ設定 (`description` を変更済み。`allow_forking` は反転後に設定)
- リポジトリ内のファイルは変更しない
