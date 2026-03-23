# actionlint をフックと CI に追加する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-actionlint-check
- Polished: {YYYY-MM-DD}

## 目的

ワークフローファイルの誤りは、push して GitHub 上で失敗するまで検出手段が無い。表現エラーはローカルで機械的に検出できるため、コミット前と CI の両方で止める。

## 現状

- 実案例がある。過去の対応で `.github/workflows/ci.yml` の `slack_notify` step に `if: ${{ secrets.SLACK_WEBHOOK != '' }}` を書いたところ、**step レベルの `if` では `secrets` コンテキストが使えない** (使えるのは `env`、`github`、`inputs`、`job`、`matrix`、`needs`、`runner`、`steps`、`strategy`、`vars`) ため、ワークフローの登録自体が失敗した。develop に push した run 219 は job が 1 つも生成されず "This run likely failed because of a workflow file issue" となっている
- この誤りは手元の `actionlint 1.7.12` (1 実行) で即座に特定できた (`context "secrets" is not allowed here` を報告)
- リポジトリに actionlint を実行する仕組みが無い。`prek.toml` のフックは cargo / Python 系のみ、`.github/workflows/` は `ci.yml` 1 本で自己検証しない
- `shiguredo-github-actions` スキルは actionlint を明示的に推奨も禁止もしていない。一方で `j178/prek-action` は許可済み action に含まれており、prek 経由で community の hook を追加する経路が確立している
- 既知の誤検出が 1 件ある。actionlint 1.7.12 のバンドルラベル目録に `ubuntu-26.04` が無く、`label "ubuntu-26.04" is unknown` と報告する。実際には存在するランナーで (`shiguredo/hisui` と `shiguredo/sora-rust-sdk` も使っており、 zakuro-rs の run 213 は ubuntu-26.04 で success になっている)、そのまま導入すると毎回失敗する

## 設計方針

1. `prek.toml` に actionlint フックを追加する。導入先は `shiguredo-rust` 参考設定と同じく prek の community repo 経由とし、rev はタグかハッシュで固定する (`prek.toml` の既存フックは `language = "system"` 中心だが、外部 repo を使う tombi の追加と同じ形に沿う)
2. CI 側は `j178/prek-action` を使った `prek run --all-files` job を追加する方向で検討する。この job はワークフロー自身の検証にもなる (追加される cargo test フックとの関係も確認する)
3. `ubuntu-26.04` の誤検出は「チェックを捨てる」のではなく、actionlint の設定ファイルで custom label として宣言する。`.actionlint.yaml` を置き、`self-hosted-runner` 側で既知ラベルを追加する書き方を確認してから導入する (actionlint のドキュメントで設定方法を確かめる)
4. 対象ファイルは `.github/workflows/*.yml` に限定する。他の YAML (例えば今後増える設定ファイル) まで actionlint にかけない
5. Composite action のような再参照 (`shiguredo/github-actions/.github/actions/rust-cache@main`) は actionlint のスコープ外で解決できないことを踏まえ、「ローカルで通っても上流 action の入力誤りは CI でしか分からない」旨を `README.md` か `docs/` に書けるか検討する (書けないなら本 issue では扱わない)

## 完了条件

- `prek run --all-files` を実行すると actionlint が走り、`ci.yml` を意図的に壊した一時変更 (step `if` に `secrets` を書く等) を検出すること。検出を実測して出力を本 issue に記録する
- 既存の `.github/workflows/ci.yml` に対して actionlint が**通過**すること (`ubuntu-26.04` の誤検出は設定で解消し、`actionlint` の exit code が 0 であること)
- CI に actionlint を実行する経路が増えていること (`prek run --all-files` を呼ぶ job でよい)
- 追加したフックが存在しないファイル種別を指さないこと (過去に存在しないファイル種別を指して失敗した前例を繰り返さない)

## 変更対象

- `prek.toml`
- `.actionlint.yaml` (新規)
- `.github/workflows/ci.yml`
