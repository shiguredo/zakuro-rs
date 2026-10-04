# zakuro-moq の publish / subscribe E2E テストを CI に追加する

- Created: 2026-10-04
- Completed: {YYYY-MM-DD}
- Branch: feature/test-add-moq-e2e
- Polished: {YYYY-MM-DD}

## 目的

`zakuro-moq` は MOQT relay へ接続して publish / subscribe を行うが、実 relay との
疎通を CI で検証する仕組みが無い。MOQT はドラフト改訂で ALPN や節構成が変わるため、
依存の追従 (moqt-rs の rev 更新など) のたびに実接続の確認が必要になる。CI に E2E
テストを追加し、実 relay に対する publish / subscribe の成立と受信 payload の一致を
継続的に検証可能にする。

## 現状

- `zakuro-moq` は `--tracks` で publish、`--subscribe-tracks` で subscribe、
  `--verify-payload` で受信 payload のパターン検査ができる
- CI (`.github/workflows/ci.yml`) は fmt / clippy / test / smoke のみで、実際の MOQT
  接続は検証しない
- moqt-rs は E2E ワークフローで `secrets.TEST_MOQT_URI` を使った実 relay の検証を既に
  実施しており、未設定時はスキップする運用が確立している
- shiguredo 組織の secret `TEST_MOQT_URI` は moqt-rs に共有されている

## 設計方針

- `.github/workflows/e2e-test.yml` を追加する。`secrets.TEST_MOQT_URI` が未設定の場合は
  ジョブをスキップする (fork からの PR などでは secret が渡らないため)
- runner は `ubuntu-24.04`。`zakuro-moq` の依存は zakuro-moq と zakuro-core のみで
  macOS 専用依存を含まないため Linux でビルドできる (`ubuntu-slim` には Rust が無い)
- 1 仮想クライアントで video トラックを publish し、同じトラックを subscribe して
  `--verify-payload` で受信 payload を検査する。`--duration` 経過で正常終了させる
- Track Namespace は実行ごとにユニークにする (他の実行や手動確認と衝突させない)
- ログに relay のホスト・名前解決したアドレスを残さないよう `::add-mask::` でマスクする
  (moqt-rs の E2E と同じ方針)
- 判定は最終サマリ行 (`MOQT finished:`) の publish / subscribe 受理数、送受信 object 数、
  payload 不一致数で行う

## 完了条件

- `secrets.TEST_MOQT_URI` 設定時に E2E が実行され、publish / subscribe が両方成立して
  payload 不一致 0 になること
- 未設定時はスキップしてワークフローが成功すること
- actionlint が通ること

## 変更対象

- `.github/workflows/e2e-test.yml` (新規)

## 前提 (リポジトリ外の作業)

- `TEST_MOQT_URI` が zakuro-rs から参照できるよう登録されていること。2026-10-04 に
  リポジトリ secret として登録済み (未登録の環境ではテストはスキップされるだけで
  失敗しない)
