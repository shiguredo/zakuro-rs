# fuzz/ を追加して外部入力のデコーダに cargo-fuzz を当てる

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-fuzzing-targets
- Polished: {YYYY-MM-DD}

## 目的

zakuro はユーザーが用意した Y4M / WAV / MP4 / JSONC を読み、HTTP で届く JSON-RPC も解析する。これらの入力はいずれも破損・不正を想定できる一方で、現状はパニック耐性を機械的に検証する仕組みが無い。`shiguredo-rust` の規約は Fuzzing を cargo-fuzz で行うことを求めており、その基盤が欠けている。

## 現状

- `fuzz/` ディレクトリは存在しない。`Cargo.toml` に fuzz ワークスペースの定義も、dev-dependencies としての fuzzing 基盤も無い
- `Makefile` には `fuzzing` / `fuzzing-parallel` / `fuzzing-list` の target が並ぶが、`fuzz/` が無く `cargo fuzz` も未導入のため実行できない (削除済み)
- 公開 API が無いため、外部 fuzz クレートから対象関数を呼べない (**前提: `issues/0019`**)
- 現状のパニック耐性の検証は `src/` 内の単体テストによる境界値チェックに依存している。例えば `src/wav_reader.rs` のテストモジュールは 24bit 拒否・非 PCM 拒否・未知チャンクスキップ・44.1kHz から 48kHz へのリサンプル・ステレオのモノミックスを個別に確認しているが、「任意バイト列で panic しないこと」を網羅的には見ていない
- 同じく `src/mp4_audio.rs` のテストモジュールは MP4 の音声トラック抽出と AAC / Opus デコードを検証しているが、これも意図的に組んだ入力に対してのみである
- `shiguredo-rust` は「Fuzzing: 任意入力に対するクラッシュ耐性 (パニック安全性)」を fuzz の役割として定義し、「PBT に任意入力でパニックしないことだけを検証するテストを書かないこと」と役割分担を定めている

## 設計方針

1. `fuzz/` を cargo-fuzz 標準の構成 (独立ワークスペース、`fuzz_targets/`、`Cargo.toml` で zakuro を `path` 依存) で追加する。本体クレートと同じ `edition` を使う
2. 最初に狙うターゲットは、破損入力が実際に届く経路を持つデコーダに絞る
   - Y4M ヘッダとフレーム (`src/y4m_reader.rs`)
   - WAV ヘッダとチャンク (`src/wav_reader.rs`)
   - MP4 の音声トラック抽出 (`src/mp4_audio.rs`)。実行時ロードライブラリを必要としない経路 (コンテナ解析と Opus) を優先する
   - JSONC 設定ファイルの解析と整形 (`src/jsonc_fmt.rs` と `src/cmd_lint.rs` 経由の検証)。`zakuro lint` / `zakuro fmt` は任意のユーザーファイルを直接読む入口であり、最も現実的な攻撃面である
   - JSON-RPC のリクエスト解析 (`src/json_rpc.rs`)。HTTP 経由で任意ボディが届く
3. 入力サイズからメモリを一気に確保するパスに注意する。`shiguredo-rust` は「入力バイナリデータをデコードする際に `Vec::with_capacity()` 等の事前確保を原則使わないこと」を定めるため、fuzz で見つかった大量確保は挙動として報告し、単体でクラッシュしない場合は別 issue で扱い本 issue の完了条件には含めない
4. CI で fuzz を回すかは本 issue の対象外とする (cargo-fuzz は nightly を要求する。`hisui` の `ci.yml` は nightly ツールチェーンを入れる job を持っており、前例はあるが実行時間の設計が別途必要)。まず開発者が手元で実行できる状態を作り、再現手順を `fuzz/README.md` に残す
5. 見つかったクラッシュは修正コミットと分離する。修正は `bug` の別 issue を立てて扱う (1 issue 1 目的のため、fuzz 基盤の追加とクラッシュ修正は混ぜない)

## 完了条件

- `cargo +nightly fuzz run <target>` で上記ターゲットが 1 つ以上実行でき、最低 1 分間の fuzzing が完了すること
- `cargo fuzz list` が追加したターゲットをすべて返すこと
- fuzz の実行方法と、クラッシュ再現に使う corpus の扱いが `fuzz/README.md` に書かれていること
- 実行結果としてクラッシュが 0 件であること (見つかった場合はその旨を本 issue に記録し、修正は別 issue に切り出す)
- 本体の `cargo test` と `cargo clippy --all-targets` が fuzz 追加によって壊れていないこと

## 変更対象

- `fuzz/` (新規: ワークスペース定義とターゲット一式)
- `Cargo.toml` (ワークスペース構成への組み込み、`exclude` の設定)
- `.gitignore` (`fuzz/corpus/` と `fuzz/artifacts/` の除外)

前提として `issues/0019` (lib ターゲット追加) が必要。`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない。
