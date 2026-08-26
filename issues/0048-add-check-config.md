# 設定検証フラグ (`--check-config`) を追加する

- Created: 2026-08-26
- Completed: {YYYY-MM-DD}
- Branch: feature/add-check-config
- Polished: {YYYY-MM-DD}

## 目的

nginx の `-t` と同様に、設定 (CLI 引数および `--config` の JSONC) を検証して終了するフラグを追加する。負荷試験を開始せずに設定ミスを CI やデプロイ前に検出できるようにする。C++ 版 zakuro には無い機能だが、設定が複雑化した zakuro-rs では運用上必要。

## 現状

- `src/args.rs` の `parse_args()` は `--help` / `--version` / `--show-video-codec-capability` を pre-parse で早期終了できるが、設定の妥当性だけを検証して終わる経路はない
- 引数・ファイル存在・排他・コーデックパラメータ等の検証は `parse_common_args` / `parse_instance_args` / `parse_args_from_argv` で行われる。`help_mode` のときはファイル存在チェックやバリデーションを skip する
- 起動後の追加検証として `src/main.rs` の `verify_video_encoder_implementation_specs` (OpenH264 ロード後のエンコーダー実装指定) や PEM 読み込みがある。その後はインスタンス起動・Sora 接続に進む
- `--show-video-codec-capability` は必須引数なしで早期終了するため、通常の設定検証経路とは目的が異なる

## 設計方針

1. `src/args.rs` に `--check-config` フラグを追加する (CommonArgs。短形 `-t` は付けない)
2. `parse_args()` は通常どおり完走させる (必須引数・ファイル存在・排他・JSONC マージを含む。`help_mode` にはしない)
3. パース成功後、`src/main.rs` の `async_main` では Sora 接続・VC 起動・HTTP サーバー・DuckDB writer 起動の前に、起動前検証まで実行して終了する
   - OpenH264 指定時はライブラリのロードと `verify_video_encoder_implementation_specs`
   - mTLS 指定時は PEM の読み込み可否
   - 入力ファイル系は `parse_args` 側の存在チェックで足りるものを重複させない
4. 成功時は標準出力またはログに成功を示して exit 0、失敗時は既存のエラー経路と同様に非 0 で終了する (nginx `-t` と同じ成功 / 失敗の使い方)
5. JSONC のみの検証も `zakuro --config <FILE> --check-config` で可能にする (CLI 上書きとの併用も通常起動と同じ規則)
6. `--check-config` と `--show-video-codec-capability` / `--help` / `--version` が同時指定された場合の優先順位は、既存の help / version / show-video-codec-capability の優先規則に合わせ、`--check-config` はそれらより後とする

## 完了条件

- 正しい設定で `--check-config` を付けると、Sora に接続せずに成功終了する (exit 0)
- 不正な設定 (必須欠落・未知キー・存在しないファイル・排他違反等) では非 0 で終了し、通常起動時と同様のエラーが得られる
- `--config` 指定の JSONC も検証対象になる
