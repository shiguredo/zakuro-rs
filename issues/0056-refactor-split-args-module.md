# `src/args.rs` を責務単位でサブモジュールに分割する

- Created: 2026-08-28
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-split-args-module
- Polished: {YYYY-MM-DD}

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
- `shiguredo-rust` は「テストが長くなるのはモジュール自体が大きすぎるサインなので `src/<module>.rs` 側の分割を検討すること」と「`mod.rs` を使わず `<module>.rs` + `<module>/<submodule>.rs` の構成にすること」を定めており、現状は両方の条件に該当している
- 前例として `issues/closed/0030-refactor-duckdb-stats-split.md` があるが、あちらは `src/duckdb_stats/mod.rs` を作り、`mod.rs` から `pub(crate) use` で再エクスポートしている。`shiguredo-rust` の現行規約 (「`mod.rs` を使わないこと」「re-export は基本的にやらないこと」) とは食い違うので、そのまま踏襲しない

## 設計方針

1. `src/args.rs` を親モジュールとして残し、上記の責務 cluster を `src/args/` 配下のサブモジュールへ移す (`src/args.rs` に `mod` 宣言を置き、`mod.rs` は作らない)。具体的な単位は実際の `use` とシンボル依存を調べてから確定し、1 サブモジュール 1 コミットで進める
2. 公開範囲は `pub(crate)` のまま維持し、`pub` を増やさない。re-export はしない (`shiguredo-rust` の「re-export は基本的にやらない」に従い、呼び出し側は `crate::args::<submodule>::<item>` で参照する)
3. 対応するテストは移った先のサブモジュールの `#[cfg(test)] mod tests` へ追随させる。private 対象の単体テストを `tests/` へ移動することはしない
4. 挙動・エラーメッセージ・CLI が受け付ける引数・JSONC が受け付けるキーは一切変更しない純粋な移動に限定する

## 完了条件

- `src/args.rs` が分割され、単独ファイルとしては `src/` 配下の他モジュールと同等規模に収まっていること
- 既存テストの名前と検証内容が無変更で通過すること (挙動・エラー文言の不変の担保)
- `src/` 配下に `mod.rs` が作られていないこと、`pub` 化されたシンボルが増えないこと
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` が通過すること

## 変更対象

- `src/args.rs` と新設する `src/args/` 配下
- 移動したシンボルを参照する呼び出し側 (`src/main.rs` など `crate::args::` を使う箇所)
