# MP4 入力系テストの一時ファイル作成と argv 構築をヘルパーに集約する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-input-mp4-test-helpers
- Polished: 2026-08-28

## 目的

`src/args.rs` の `mod tests` で `--input-mp4` 系テスト 4 件が同じ一時ファイル作成手順と argv 構築を繰り返している。アサーション以外のコピーをヘルパーに集約し、ボイラープレートの修正漏れが起こる余地をなくす。

## 現状

- `mod tests` の `--input-mp4` 系テスト 4 件 (`parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` / `parse_args_from_argv_rejects_input_mp4_with_openh264` / `parse_args_from_argv_accepts_input_mp4_without_openh264` / `parse_args_from_argv_rejects_input_mp4_with_input_wav`) は、いずれも `tempfile::TempDir::new()` と `std::fs::write(&mp4, b"dummy")` を同じ `expect` 文言付きで書いている
- 同じ 4 件は `minimal_sora_argv()` へ `--input-mp4` / `--sora-video-codec-type h264` / `--sora-video-bit-rate 1000` を `tpl.extend` で足しており、この 3 組 6 要素が重複している。 codec type と bit rate の値は 4 件とも `h264` / `1000` で一致するため、ヘルパー側で固定値として扱える
- テストヘルパーは `mod tests` 内に `minimal_sora_argv()` / `common_duckdb_argv()` / `to_argv()` が既にあり、`mod tests` 内に置くのが現行の流儀である。引数を取って argv を組み立てる形は `common_duckdb_argv()` に前例がある。`shiguredo-rust` の「テスト間で共有するヘルパーは `tests/helpers/` に置くこと」は `tests/` 配下の公開 API テストが対象で、private な `parse_instance_args()` / `parse_args_from_argv()` を対象にするこのテスト群には当てはまらない
- 同じ `mod tests` を触る issue が 3 件ある。#0053 はこの 4 件のうち 3 件のアサーションを強化し、#0054 は別テスト (`jsonc_fdk_aac_lib_expands_to_common_args`) から一時ファイル作成を削除し、#0056 は `mod tests` を `src/args/` 配下のサブモジュールへ追随させる。本 issue は #0053 と #0054 の後に、#0056 より前に着手する (#0056 が先行した場合は変更対象を移動先サブモジュールの `mod tests` と読み替える)

## 設計方針

1. `mod tests` 内に `(tempfile::TempDir, PathBuf)` を返すダミー MP4 作成ヘルパーを追加する。形状は `src/duckdb_stats/schema.rs` の `setup_db()` が `TempDir` を呼び出し側へ返す前例であり、呼び出し側も `let (_dir, conn) = setup_db();` と名前付きで束縛している。ヘルパー内で `TempDir` を作ってパスだけを返すと、ヘルパーから抜けた時点で一時ディレクトリが削除されるため、必ず `TempDir` を呼び出し側へ返す。呼び出し側は `let (dir, mp4) = ...;` と両方を束縛する (`let (_, mp4)` のように無名にすると文の終りで early drop して一時ディレクトリが消える)
2. `minimal_sora_argv()` に `--input-mp4 <パス>` / `--sora-video-codec-type h264` / `--sora-video-bit-rate 1000` を足した argv を返すヘルパーを追加する。MP4 パスに依存するので `fn minimal_sora_input_mp4_argv(mp4: &Path) -> Vec<String>` のように引数を受け取る形にし、`minimal_sora_argv()` の近くに置く
3. 4 件は「ヘルパーで `dir` と `mp4` を作り、ヘルパーの argv に個別のキーを足すだけ」の形に寄せる。`--openh264` 用の `libopenh264.dylib` と `--input-wav` 用の `audio.wav` は同じ `dir.path()` 配下にテスト本体で書き続ける。個別キーを足す場所には注意が必要で、`--h264-encoder` と `--input-wav` はインスタンス argv (ヘルパーの返り値へ `extend` する) だが、`--openh264` は共通引数なので `parse_args_from_argv()` の第 2 引数 (common_argv) へ渡す経路のまま変えない。テスト名・assert の内容・カバーする挙動は変更しない
4. 挙動を変えないことが前提のリファクタリングなので、エラー文言・排他検証・`parse_instance_args()` 側の変更は行わない (#0053 で確定した逐語 assert をさらに変更しない)

## 完了条件

- `--input-mp4` 系テスト 4 件の本文から、`tempfile::TempDir::new()` と `std::fs::write(&mp4, b"dummy")` と 3 組 6 要素を足す `tpl.extend` が無くなっていること (テスト個別のキーを足す `extend` まで消す条件ではない)
- 変更前後でテストの名前・数・検証内容が同一であること (`cargo test --workspace` の通過テスト数が変わらない)
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` が通過すること

## 変更対象

- `src/args.rs` (`mod tests` の `--input-mp4` 系テスト 4 件と、新設するテストヘルパー 2 件)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間はブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり、実ブランチは切らない。
