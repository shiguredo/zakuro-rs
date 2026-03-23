# pbt/ を追加して noprop でプロパティテストを書く

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-pbt-tests
- Polished: {YYYY-MM-DD}

## 目的

 zakuro は Y4M / WAV / MP4 / JSONC という形式化されたデータの読み書きを行い、JSONC の整形 (`zakuro fmt`) は可逆性を保つべき変換である。こうした不変条件は個別の単体テストでは網羅できず、`shiguredo-rust` の規約は PBT を noprop で行うことを求めている。現状は PBT の基盤も 1 件も存在しない。

## 現状

- `pbt/` ディレクトリは存在しない。`Cargo.toml` の dev-dependencies は `insta` と `tempfile` のみで `noprop` は入っていない
- `Makefile` には `pbt` と `pbt-with-cover` が並ぶが、`pbt` パッケージは存在しないため `cargo llvm-cov -p pbt --tests` は失敗する (削除済み)
- 公開 API が無いため、外部テストクレートから検証対象を呼べない (**前提: `issues/0019`**)
- 既存テストは `src/` 内の `#[cfg(test)]` と insta スナップショット (`src/snapshots/` の 5 ファイルは `jsonc_fmt` の出力整形を golden master で見ている) に集中している
- `shiguredo-rust` は「PBT: noprop のサンプラー (`sample_*`) で入力を生成し、プロパティを検証する (ラウンドトリップ等)」「unittest は pbt で実現できないものだけを書く」「pbt 以下に unittest を書かないこと」を定める。現状は役割分担の前提となる pbt が欠けている

## 設計方針

1. `pbt/` を独立パッケージ (zakuro を `path` 依存、`[dev-dependencies]` に `noprop`) として追加し、テストは `pbt/tests/prop_<module>.rs` の粒度で `src/<module>.rs` に対応させる (`shiguredo-rust` のファイル名規約)
2. 最初に検証するプロパティは、ラウンドトリップと冪等性が自然に定義できる変換に絞る
   - JSONC 整形の冪等性: `fmt(fmt(x)) == fmt(x)`、および `fmt` が意味を変えないこと (`src/jsonc_fmt.rs`)
   - WAV の読み取り: 生成した PCM データを `src/wav_reader.rs` で読むとサンプル値と再生長が一致すること。リサンプル入力のレート変換が期望比になること
   - Y4M の読み取り: 生成したヘッダとフレームが `src/y4m_reader.rs` で一致して読めること
   - MP4 のコンテナ解析: `src/mp4_audio.rs` のトラック列挙が、任意の妥当な MP4 でクラッシュせず整合した情報を返すこと (実ファイルではなく生成したコンテナを扱う範囲で)
   - 引数解析の妥当性: `src/args.rs` の排他検証が、生成した引数列に対して「同時に指定できない組み合わせを受理しない」こと (`issues/0012` が単体で広げようとしている分岐を、プロパティとして一般化できるか検討する)
3. サンプル生成は noprop の `sample_*` を使い、テスト関数名は英語にする (規約)
4. テストログメッセージは日本語に統一する (`AGENTS.md` の規約)。既存の `src/` 内テストと揃う
5. 単体テストで既に十分カバーされている境界値ケースを PBT に複製しない。「PBT でカバーできるものを単体テストで書かないこと」の逆方向 (既存単体を消す判断) は、カバレッジを実測してから別 issue で扱う
6. 実行時間は `Runner` の反復回数で制御する。CI で回す場合の時間予算を決めてから追加し、手元で数十分かかる設定にしない

## 完了条件

- `pbt/` がワークスペースに組み込まれ、`cargo test -p pbt` が通過すること
- 上記 1〜4 のうち最低 3 モジュール分のプロパティテストが `pbt/tests/prop_<module>.rs` の命名で存在すること
- noprop の失敗再現手順 (シードの固定と再実行方法) が `pbt/` 内の説明から読み取れること
- `src/` 内の既存単体テストが重複削除によって減っていないこと (削除は本 issue の対象外)
- CI で実行する場合、`pbt` の job が許容時間に収まること (収まらないなら CI 統合は別 issue に切り出す)

## 変更対象

- `pbt/Cargo.toml`、`pbt/tests/*.rs` (新規)
- `Cargo.toml` (ワークスペース定義への組み込み)

前提として `issues/0019` (lib ターゲット追加) が必要。`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない。
