# issues/ の社内運用メタデータと社内用語を公開向けに整える

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/update-issue-metadata-publication
- Polished: {YYYY-MM-DD}

## 目的

`issues/` 配下をリポジトリで追跡する運用自体は、組織の既存公開リポジトリ (`hisui`、`sora-rust-sdk`、`webrtc-rs`) も同じなので問題ない。一方 zakuro-rs の issue ファイルには、開発環境の内情や社内でしか通じない用語が残っており、そのまま公開すると外部読者には読めない・出すべきでない情報が混じる。

## 現状

### LLM モデル名のメタデータ

- `issues/` 配下の 31 ファイルに `Model:` フィールドがあり、使用した LLM モデル名が実値で入っている
- 新しい issue 2 件 (`issues/0057`、`issues/0058`) には `Model:` 行が既に無く、テンプレ由来の項目が運用から外れている
- `create-issue` スキルの現行テンプレートにも `Model:` は無い

### テンプレートに無いメタデータ

- `issues/closed/0026-bug-fix-project-metadata-and-ci.md` は `Priority: High` を持つ。現行テンプレートのメタデータは `Created` / `Completed` / `Branch` / `Polished` / `Milestone` (任意) / `Reporter` (任意) で、`Priority` は含まれない
- `Polished:` を持つ issue が多数あるが、これは `/polish-issue` 系しか更新しない項目であり実値を残してよい

### 社内運用でしか通じない表現

- 開発プロセス内部の用語が本文に混じる issue がある (レビュー工程の名称、確信度の表記、承認の流れを示す語など)。例: `issues/closed/0040-add-scenario-play-sub-scenario.md` の closed 化理由を記した箇所
- 社内スラッシュコマンドの名前がそのまま書かれた issue がある (`issues/closed/0052-update-github-actions-node20-deprecation.md`)
- 社内スキル配置ディレクトリの相対パス (`skills/` 配下のファイルパス) へ言及する issue がある (`issues/0057-update-prek-hooks-config.md`)
- 要望者の識別方法が「社内ユーザー」と「外部ユーザー」を区別できない書き方の箇所がある (`issues/closed/0022-change-input-y4m.md`)
- 存在しないファイルパスを根拠として引用する箇所がある (`issues/0015-add-degradation-preference.md` が挙げる docs 配下の参照は、本リポジトリの `docs/` に実在しない)

### 判断が要るもの

- `CHANGES.md` と issue 内の担当者クレジット `@voluntas` は、公開 GitHub ハンドルであり組織の変更履歴慣習そのもの (`issues/closed/0032` がクレジット付与を完了条件にしている)。維持でよいと判断する
- `AGENTS.md` の `shiguredo-*` スキル参照と `CODEBASE.md` の存在は、`hisui` (AGENTS.md / CODEBASE.md 相当を公開済み) と `sora-rust-sdk` (CODEBASE.md を公開済み) に前例があるため対象外とする

## 設計方針

1. 追跡されている全 issue ファイルから `Model:` 行を削除する。機械的に消せる行であり、本文の技術的内容に依存しない
2. `Priority:` は現行テンプレートに無い項目なので、`issues/closed/` 側から削除するか、組織として使う項目ならテンプレート側へ正式に追加して全 issue で揃える。どちらにするか決めてから作業する (片方だけ残すのが最悪)
3. 社内運用用語は「何が起きたか」が外部から読み取れる表現に置き換える。レビュー工程名・承認の流れを示す語は、結果として何が決定されたかに書き換える (例: 「〜と判定され承認のうえ closed にした」→「対応不要と判断し closed にした」)
4. 社内スラッシュコマンド名は、そのコマンドが実際に行った内容 (GitHub Actions の更新など) に書き換える
5. `skills/` 配下の配置パスへの参照は削除するか、「組織の Rust 規約の参考設定」という一般表現に置き換える (スキル名自体は公開済みなので残してよい)
6. 要望者の識別は、外部ユーザーからの要望なのか開発者間の会話なのかを一般化して書く (`利用者からの要望` など)
7. 存在しない docs パスへの参照は、削除するか公開済みの上流 URL へ張り替える
8. 書き換えは本文の技術的結論を変えない。判断根拠が消えてはならない (`/polish-issue` で本文ごと作り直すのとは目的が違う)

## 完了条件

- `rg '^(- )?Model:' issues/` が 0 件であること
- `rg -n 'Priority:' issues/` が 0 件か、あるいはテンプレートに `Priority:` が定義され全 issue が揃っていること
- 社内のレビュー工程名・社内スラッシュコマンド名・スキル配置ディレクトリへの相対パスが issue 本文から読み取れないこと
- 存在しないファイルパスへの参照が残っていないこと
- `git ls-files issues/` の全 issue の技術的内容 (目的・設計方針・完了条件) が書き換え前と等価であること

## 変更対象

- `issues/` 配下の追跡済み markdown (31 ファイルの `Model:` 削除が中心。上記 2〜7 で該当 issue のみ本文を修正)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
