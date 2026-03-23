# `src/args.rs` を責務単位でサブモジュールに分割する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-split-args-module
- Polished: 2026-08-28

## 目的

`src/args.rs` が 3516 行 (`src/` 配下で最大。次点の `src/duckdb_stats/stats_json.rs` 1237 行の約 2.8 倍) まで成長し、CLI 引数定義・コーデックパラメータ検証・JSONC 設定読み込み・診断用の pre-parse・テストが 1 ファイルに同居している。責務単位でサブモジュールへ分割し、変更の影響範囲を追える規模に戻す。

## 現状

- `src/args.rs` は 3516 行。`#[cfg(test)] mod tests` が末尾 1500 行を占め、テスト関数は 86 件
- 本番側には少なくとも次の責務が同居している
  - 引数定義と CLI パース本体: `CommonArgs` / `InstanceArgs` / `parse_common_args` / `parse_instance_args` / `parse_args_from_argv` / `parse_args`
  - コーデックパラメータの検証: `parse_video_vp9_params` / `parse_video_av1_params` / `parse_video_h264_params` / `parse_video_h265_params` と `for_each_params_member` / `params_u32` / `params_bool` / `params_string` / `unknown_params_key_error` / `validate_video_params_codec_type` / `parse_video_codec_implementation` / `parse_video_codec_type` / `parse_resolution`
  - JSONC 設定の読み込みと argv 化: `JsoncConfig` / `is_common_key` / `is_flag` / `push_kv` / `flatten_sora_object` / `is_unsupported_key` / `load_jsonc_config` / `validate_jsonc_config_str` / `parse_jsonc_config` / `expand_instances` / `split_cli_argv` / `dedupe_argv_last_wins`
  - `--show-video-codec-capability` の pre-parse: `ShowVideoCodecCapabilityInputs` / `warn_show_capability_missing_value` / `pre_parse_show_video_codec_capability_inputs`
  - ログレベルの先行読み取り: `parse_log_level_str` / `peek_log_level_from_tokens` / `peek_log_level_from_jsonc` / `peek_log_level`
- `shiguredo-rust` は「テストが長くなるのはモジュール自体が大きすぎるサインなので `src/<module>.rs` 側の分割を検討すること」を定めており、現状はこれに該当する。同じく「`mod.rs` を使わず `<module>.rs` + `<module>/<submodule>.rs` の構成にすること」も定めており、分割後の構成としてこれを守る必要がある
- 本番側 38 アイテムのうち `pub(crate)` は 8 件だけで、残り 30 件 (`parse_common_args` / `parse_instance_args` / `parse_args_from_argv` / 各 param 検証 / 各 JSONC 展開関数など) は private である。サブモジュールへ分けた時点で cluster 横断の参照が必ず発生する (`validate_jsonc_config_str` は `parse_args_from_argv` を呼ぶ、`parse_common_args` は `dedupe_argv_last_wins` と `parse_log_level_str` を呼ぶ、`parse_instance_args` は param 検証群を呼ぶなど)
- `src/args.rs` の先頭に `//!` のモジュールコメントを置いていない (`shiguredo-rust` は `src/<module>.rs` 先頭へ責務を 1〜2 行書くことを必須としている)。`src/` 配下では `diagnostic.rs` / `cmd_lint.rs` / `jsonc_fmt.rs` / `mp4_audio.rs` / `video_codec_capability.rs` などが持っている
- 前例として `src/duckdb_stats/` を分割した構成があるが、あちらは `src/duckdb_stats/mod.rs` を作り、`mod.rs` から `pub(crate) use` で再エクスポートしている。`shiguredo-rust` の現行規約 (「`mod.rs` を使わないこと」「re-export は基本的にやらないこと」) とは食い違うので、そのまま踏襲しない。`src/duckdb_stats/mod.rs` が現存する (`src/` 配下で唯一の `mod.rs`) が、その是正は本 issue の対象外

## 設計方針

1. `src/args.rs` を親モジュールとして残し、上記の責務 cluster を `src/args/` 配下のサブモジュールへ移す (`src/args.rs` に `pub(crate) mod` 宣言を置き、`mod.rs` は作らない)。cluster 1 の引数定義と CLI パース本体もサブモジュールへ移し、親に残すのは `mod` 宣言とモジュールコメントだけにする。具体的な単位は実際の `use` とシンボル依存を調べてから確定し、1 サブモジュール 1 コミットで進める
2. クレート外へ向けた `pub` は増やさない。サブモジュール間の参照に必要な `pub(crate) mod` 宣言と、private アイテムの `pub(crate)` (親から子だけを参照させるなら `pub(super)`) への昇格は許容する。「可視性修飾子の追加は許容、関数本体の変更は禁止」は前例 closed/0030 と同じ判断。re-export はしない (`shiguredo-rust` の「re-export は基本的にやらない」に従い、呼び出し側は `crate::args::<submodule>::<item>` で参照する)
3. 対応するテストは移った先のサブモジュールの `#[cfg(test)] mod tests` へ追随させる。private 対象の単体テストを `tests/` へ移動することはしない。複数 cluster を横断して検証しているテスト (`no_video_device_excludes_all_sources` など) は主対象となる cluster の `mod tests` へ置き、兄弟サブモジュールのアイテムを `pub(crate)` 越しに参照する。テスト共有ヘルパー (`minimal_sora_argv` / `common_duckdb_argv` / `to_argv` と #0055 で新設する 2 ヘルパー) は `src/args.rs` に `#[cfg(test)] pub(crate) mod test_support;` を置いて `src/args/test_support.rs` へ集約し、各サブモジュールの `mod tests` から `crate::args::test_support::` 経由で参照する。重複定義は採らない
4. 新規に作る各サブモジュールと `src/args.rs` には `//!` で責務を 1〜2 行書く (`shiguredo-rust` の必須規約。現状 `src/args.rs` 先頭には無い)。コメント追加は挙動を変えない
5. 挙動・エラーメッセージ・CLI が受け付ける引数・JSONC が受け付けるキーは一切変更しない純粋な移動に限定する

## 完了条件

- `src/args.rs` が分割され、`src/args.rs` 単体および `src/args/` 配下の各ファイルが、本番側 (`#[cfg(test)] mod tests` を除く) で次点の `src/duckdb_stats/stats_json.rs` (1237 行) を超えていないこと。`mod tests` を含めた実行数が 1237 行を大きく超える場合は、行の付き先ではなく cluster の切り方を調整する手がかりにする
- `#[test]` 関数名と検証内容が無変更で通過すること。テストの完全修飾名は移動に伴い `args::tests::<関数>` から `args::<サブモジュール>::tests::<関数>` へ変わるため、担保は `cargo test --workspace` の通過テスト数が変わらないこととエラー文言の逐語一致で行う (`#[test]` 関数の識別子自体は変えない)
- 本 issue で `src/` 配下に `mod.rs` を追加しないこと (既存の `src/duckdb_stats/mod.rs` は対象外)。`pub` (修飾子なし) への変更を 1 件も行わないこと。現状は修飾子なしの `pub` は 0 件
- 新設する各サブモジュールと `src/args.rs` に `//!` の責務コメントがあること
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` が通過すること

## 依存関係

- #0053 / #0054 / #0055 は同じ `src/args.rs` を変更し、#0055 は `mod tests` へヘルパーを新設する。本 issue はこの 3 件が landed した後に着手する (3 件が本 issue 後にずれた場合は変更対象を移動先サブモジュール側へ読み替える)
- #0046 は `parse_args()` 内にインラインで書かれた capability 構築 (MP4 パススルー → OpenH264 → NopVideoDecoder) を含む 3 経路を集約する。本 issue を #0046 より前に land させ、#0046 着手時に `src/args.rs` 前提の現状記述を移動先シンボルへ refresh する
- `InstanceArgs` への追加を扱う #0015 / pending の #0041 / #0042 とはファイルを共有するが、順序依存は無い

## 変更対象

- `src/args.rs` と新設する `src/args/` 配下
- 移動したシンボルを参照する呼び出し側: `src/main.rs` / `src/cmd_lint.rs` / `src/duckdb_stats/stats_json.rs`（`crate::args::` を使う箇所。実測はこの 3 ファイル）

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間はブランチを切らない) により develop へ直接コミットし、「1 サブモジュール 1 コミット」も develop 上に乗せる。`Branch:` は名目上の名前であり、実ブランチは切らない。コミットメッセージは `shiguredo-git` の `{SEQ} {変更内容}` 形式で各コミットの変更内容を書き分ける。
