# JSONC に一時パスを埋め込むテストが TMPDIR の内容に依存して fail するのを修正する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-jsonc-test-tmpdir-sensitivity
- Polished: 2026-08-28

## 目的

`src/args.rs` の一部テストは `tempfile::TempDir` の実パスを JSONC 文字列へ埋め込んでいる。JSONC 設定の展開層 (下記 `push_kv()` など 4 箇所) は `${` を含む文字列を環境変数置換の未対応として意図的に拒否するため、`TMPDIR` に `${` を含む環境ではテストが落ちる。環境に依存せず通過する形にする。

## 現状

- JSONC の文字列値に `${` が含まれると環境変数置換の未対応としてエラーを返す。実装は `push_kv()` / `flatten_sora_object()` / `parse_jsonc_config()` / `expand_instances()` の 4 箇所にある。最上位の共通キー (`fdk-aac-lib` など) で `${` を含む値を返すのは `parse_jsonc_config()` であり、キーの分類と `push_kv()` への委譲より先に文字列値を拒否する。`push_kv()` のチェックは `sora` 配下など他経路から渡された文字列に、`expand_instances()` のチェックは `instances[i]` 直下の文字列値に、`flatten_sora_object()` のチェックは `sora.signaling-url` の配列要素に効く。文言は `push_kv()` と `parse_jsonc_config()` が同一で (`環境変数置換 '${...}' は未対応です (key: '<キー名>')`)、`expand_instances()` は `(instances[i] key: '<キー名>')`、`flatten_sora_object()` は `(sora.signaling-url: '<値>')` の書式なので、後者 2 箇所は文言から発生源を区別できる。いずれも現バージョンで意図した仕様 (環境変数置換の未対応を起動時にエラーへ落とす) であり、パーサー側は変更しない
- `mod tests` の `jsonc_fdk_aac_lib_expands_to_common_args` は一時ディレクトリに作った `libfdk-aac.so.2` のパスを JSONC の `"fdk-aac-lib"` の値へ埋め込むため、`TMPDIR` に `${` を含むと意図した拒否にヒットして fail する
- 再現手順: `mkdir -p '/tmp/zk_a${X}b'` のうえで `TMPDIR='/tmp/zk_a${X}b' cargo test --workspace` を実行すると `test result: FAILED. 203 passed; 1 failed` となる (fail するのは上記 1 件のみ)。既定の `TMPDIR` では通過する
- CLI argv 経由のテスト (`fdk_aac_lib_parses_as_common_arg` など) は一時パスを `Vec<String>` の要素として渡すだけで、`${` を検出する JSONC 文字列パースを通らないため影響しない。`mod tests` 内で JSONC に実パスを埋め込んでいるのは上記 1 件のみである
- 当該テストは `parse_jsonc_config()` までしか呼ばない。`--fdk-aac-lib` のファイル存在チェック (`fdk-aac-lib: library file not found`) は `parse_common_args()` にあり、`parse_jsonc_config()` は値の実在性を見ないため、このテストに実ファイルは不要である

## 設計方針

1. `jsonc_fdk_aac_lib_expands_to_common_args` で JSONC に埋め込む値を、`${` を含まないと確定できる固定のダミーパスに変更する。同じ `mod tests` の `jsonc_fdk_aac_lib_inside_instance_rejected` が `/nonexistent/libfdk-aac.so` を使っており、その形に揃える。実ファイルが不要になるため `tempfile::TempDir` と `std::fs::write` は削除する
2. `parse_jsonc_config()` へ実パスを渡している他のテストが無いかを `mod tests` 内で横断検索し、見付かった場合は同じ方針で直す（起票時の確認では該当無く、`format!` で JSONC 内容へ一時パスを埋め込んでいるテストは上記 1 件のみ）
3. JSONC 側の `${` 拒否 (上記 4 箇所) は変更しない。`${` 拒否を検証している既存テスト `env_var_substitution_is_rejected`（`sora.signaling-url` の文字列値経由で `push_kv()` の拒否を見ている）も変更しない

## 完了条件

- `TMPDIR` に `${` を含むディレクトリを指定しても `cargo test --workspace` が通過すること
- `mod tests` 内で JSONC 文字列へ一時パスを埋め込んでいるテストが残っていないこと
- JSONC の環境変数置換拒否を検証している既存テスト `env_var_substitution_is_rejected` がそのまま通過すること (静的文字列しか使わないので TMPDIR に依存しない)
- `cargo test --workspace` のテスト数が変更前後で同一であること (一時ファイル作成の削除でテスト関数自体を減らしていないことの確認)

## 変更対象

- `src/args.rs` (`mod tests` の `jsonc_fdk_aac_lib_expands_to_common_args`)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間はブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり、実ブランチは切らない (`issues/closed/0047` と同じ運用)。
