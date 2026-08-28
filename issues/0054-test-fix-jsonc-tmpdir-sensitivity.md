# JSONC に一時パスを埋め込むテストが TMPDIR の内容に依存して fail するのを修正する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-jsonc-test-tmpdir-sensitivity
- Polished: {YYYY-MM-DD}

## 目的

`src/args.rs` の一部テストは `tempfile::TempDir` の実パスを JSONC 文字列へ埋め込んでいる。JSONC パーサーは `${` を含む文字列を環境変数置換として意図的に拒否するため、`TMPDIR` に `${` を含む環境ではテストが落ちる。環境に依存せず通過する形にする。

## 現状

- JSONC の文字列値に `${` が含まれると環境変数置換の未対応としてエラーを返す。実装は `push_kv()` / `flatten_sora_object()` / `parse_jsonc_config()` / `expand_instances()` の 4 箇所にあり、`fdk-aac-lib` のような共通キーは `push_kv()` が `環境変数置換 '${...}' は未対応です` を返す。これは現バージョンで意図した仕様 (環境変数置換の未対応を起動時にエラーへ落とす) であり、パーサー側は変更しない
- `mod tests` の `jsonc_fdk_aac_lib_expands_to_common_args` は一時ディレクトリに作った `libfdk-aac.so.2` のパスを JSONC の `"fdk-aac-lib"` の値へ埋め込むため、`TMPDIR` に `${` を含むと意図した拒否にヒットして fail する
- 再現手順: `mkdir -p '/tmp/zk_a${X}b'` のうえで `TMPDIR='/tmp/zk_a${X}b' cargo test --workspace` を実行すると `test result: FAILED. 203 passed; 1 failed` となる (fail するのは上記 1 件のみ)。既定の `TMPDIR` では通過する
- CLI argv 経由のテスト (`fdk_aac_lib_parses_as_common_arg` など) は一時パスを `Vec<String>` の要素として渡すだけで、`${` を検出する JSONC 文字列パースを通らないため影響しない。`mod tests` 内で JSONC に実パスを埋め込んでいるのは上記 1 件のみである
- 当該テストは `parse_jsonc_config()` までしか呼ばない。`--fdk-aac-lib` のファイル存在チェック (`fdk-aac-lib: library file not found`) は `parse_common_args()` にあり、`parse_jsonc_config()` は値の実在性を見ないため、このテストに実ファイルは不要である

## 設計方針

1. `jsonc_fdk_aac_lib_expands_to_common_args` で JSONC に埋め込む値を、`${` を含まないと確定できる固定のダミーパスに変更する。同じ `mod tests` の `jsonc_fdk_aac_lib_inside_instance_rejected` が `/nonexistent/libfdk-aac.so` を使っており、その形に揃える。実ファイルが不要になるため `tempfile::TempDir` と `std::fs::write` は削除する
2. `parse_jsonc_config()` へ実パスを渡している他のテストが無いかを `mod tests` 内で横断検索し、見付かった場合は同じ方針で直す
3. JSONC 側の `${` 拒否 (上記 4 箇所) と、それを検証している既存テスト (`環境変数置換` のエラーを期待するテスト) は変更しない

## 完了条件

- `TMPDIR` に `${` を含むディレクトリを指定しても `cargo test --workspace` が通過すること
- JSONC の環境変数置換拒否を検証している既存テストがそのまま通過すること
- `cargo test --workspace` のテスト数が変更前後で同一であること (実ファイル作成の削除でテストが減っていないこと)

## 変更対象

- `src/args.rs` (`mod tests` の `jsonc_fdk_aac_lib_expands_to_common_args` と、同じ方式を使っているテスト)
