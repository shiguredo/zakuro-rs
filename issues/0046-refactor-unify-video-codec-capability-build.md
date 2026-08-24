# ビデオコーデック capability の構築を 1 箇所へ集約する

- Created: 2026-08-25
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-unify-video-codec-capability-build
- Polished: {YYYY-MM-DD}

## 目的

ビデオコーデック capability 列の構築ロジック (既定エンジン + OpenH264 + MP4 パススルー + NopVideoDecoder の登録順・条件判定) が複数箇所に複製されている。将来どこか 1 箇所だけを変更すると、実際の接続・起動時検証・能力表示の間で登録内容が静かに乖離する。構築を 1 箇所へ集約してドリフトを防ぐ。

## 現状

- `src/main.rs` の `run_zakuro_instance` では、`SoraConnectionContextConfig` 構築ブロック内で MP4 パススルー → OpenH264 → NopVideoDecoder (受信ロール時のみ) の順に capability を登録し、続けてエンコーダー実装指定の適用 (`apply_video_encoder_implementation_specs`) を行う
- `src/main.rs` の `verify_video_encoder_implementation_specs` では、起動時検証用に OpenH264 → NopVideoDecoder の列を構築する (MP4 パススルーはエンコーダー実装指定と排他のため意図的に除外)
- `src/args.rs` の `parse_args()` にある `--show-video-codec-capability` の pre-parse では、`SoraConnectionContextConfig::default()` の `video_codec_capabilities` を起点に MP4 パススルー → OpenH264 → NopVideoDecoder の順で累積する (受信ロール判定には CLI の `--sora-role` を使う)
- 3 経路は「既定エンジン列 + 任意の OpenH264 / MP4 パススルー / NopVideoDecoder」という同一の組み立てを、入力の形態 (CLI パス / ロード済みライブラリ / 開封済みリーダー) とエラー処理 (警告して除外 / エラー) だけ変えて繰り返している

## 設計方針

- 「既定エンジン列の取得 + OpenH264 / MP4 パススルー / NopVideoDecoder の追加」を 1 つの関数に集約し、3 経路がそれを呼ぶ
- 登録順は MP4 パススルー → OpenH264 → NopVideoDecoder に統一する (run_zakuro_instance と一致させる)
- 各経路の差分 (追加する対象の有無・エラー時の挙動・エンコーダー実装指定の適用) は引数と呼び出し側に残す。組み立て関数は「列を既定どおり構築する」責務に絞り、preference への反映や検証は呼び出し側で行う
- 関数の置き場所は、`src/args.rs` (CLI/JSONC 処理) 側と `src/main.rs` (接続・実行・検証) 側の依存方向・既存モジュール構成を確認して選ぶ

## 完了条件

- 既定エンジン列 + OpenH264 + MP4 パススルー + NopVideoDecoder の組み立て (登録順・条件判定) が 1 箇所で定義され、`run_zakuro_instance` / `verify_video_encoder_implementation_specs` / `--show-video-codec-capability` pre-parse の 3 経路が同じ構築コードを呼ぶ
- `cargo test --workspace` と `cargo clippy --workspace -- -D warnings` が通る
- `--show-video-codec-capability` の出力 (エンジン列・登録順・パラメータ表示) が変更前と同一である
