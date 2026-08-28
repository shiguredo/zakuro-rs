# MP4 入力系テストの一時ファイル作成と argv 構築をヘルパーに集約する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-input-mp4-test-helpers
- Polished: {YYYY-MM-DD}

## 目的

`src/args.rs` の `mod tests` で `--input-mp4` 系テスト 4 件が同じ一時ファイル作成手順と argv 構築を繰り返している。アサーション以外のコピーをヘルパーに集約し、ボイラープレートの修正漏れが起こる余地をなくす。

## 現状

- `mod tests` の `--input-mp4` 系テスト 4 件 (`parse_args_from_argv_rejects_encoder_implementation_with_input_mp4` / `parse_args_from_argv_rejects_input_mp4_with_openh264` / `parse_args_from_argv_accepts_input_mp4_without_openh264` / `parse_args_from_argv_rejects_input_mp4_with_input_wav`) は、いずれも `tempfile::TempDir::new()` と `std::fs::write(&mp4, b"dummy")` を同じ `expect` 文言付きで書いている
- 同じ 4 件は `minimal_sora_argv()` へ `--input-mp4` / `--sora-video-codec-type h264` / `--sora-video-bit-rate 1000` を `tpl.extend` で足しており、この 3 組 6 要素が重複している (`mod tests` 内の `tempfile::TempDir::new()` は 14 箇所)
- テストヘルパーは `minimal_sora_argv()` が `mod tests` 内に既にあり、これが現行の流儀である。`shiguredo-rust` の「テスト間で共有するヘルパーは `tests/helpers/` に置くこと」は `tests/` 配下の公開 API テストが対象で、private な `parse_instance_args()` / `parse_args_from_argv()` を対象にするこのテスト群には当てはまらない
- `issues/0053` と同じテストを変更する。0053 のアサート強化を先に着手する方が衝突が少ない

## 設計方針

1. `mod tests` 内にダミー MP4 のパスを作るヘルパーを追加する。ヘルパー内で `TempDir` を作ってパスだけを返すと、ヘルパーから抜けた時点で一時ディレクトリが削除される。呼び出し側が `TempDir` を束縛できる形 (`&TempDir` を受け取る、または `(TempDir, PathBuf)` を返す) を選び、早期ドロップが起きないことを確認する
2. `minimal_sora_argv()` に `--input-mp4` / `--sora-video-codec-type` / `--sora-video-bit-rate` を足した argv を返すヘルパーを追加し、`minimal_sora_argv()` の近くに置く
3. 4 件は「ヘルパーで作った argv を基に個別のキーを足すだけ」の形に寄せる。テスト名・assert の内容・カバーする挙動は変更しない

## 完了条件

- `--input-mp4` 系テスト 4 件から、一時ファイル作成と argv 構築の重複が消えていること
- 変更前後でテストの名前・数・検証内容が同一であること (`cargo test --workspace` の通過テスト数が変わらない)
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` が通過すること

## 変更対象

- `src/args.rs` (`mod tests` の `--input-mp4` 系テスト 4 件と、新設するテストヘルパー)
