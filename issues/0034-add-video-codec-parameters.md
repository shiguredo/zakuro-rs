# コーデックパラメータ (`--sora-video-vp9-params` 等) を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-video-codec-parameters
- Polished: {YYYY-MM-DD}

## 目的

zakuro (C++) では `--sora-video-vp9-params` / `--sora-video-av1-params` / `--sora-video-h264-params` / `--sora-video-h265-params` でコーデック固有パラメータを JSON で指定できる (`zakuro/src/util.cpp` の `--sora-video-*-params` オプション)。エンコーダの細かな挙動 (レート制御等) を制御して負荷試験の再現性を高めるために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/main.rs` の `run_zakuro_instance()` はコーデック設定を `sora_sdk::Video::new_vp8()` / `new_vp9()` / `new_av1()` / `new_h264()` / `new_h265()` で構築しており、`new_vp9(args.video_bit_rate, None)` のようにパラメータには常に `None` を渡している
- sora_sdk には `VideoVP9Params` / `VideoAV1Params` / `VideoH264Params` / `VideoH265Params` と、`Video::new_vp9(bit_rate, vp9_params)` 等のパラメータ付きコンストラクタが存在するため、CLI 未配線の状態
- `docs/ZAKURO.md` にも「コーデックパラメータ (`VideoVP9Params` 等) は SDK に API がある。zakuro-rs の CLI 未配線が残っている」と記載されている

## 設計方針

1. `src/args.rs` の `InstanceArgs` に `sora_video_vp9_params` / `sora_video_av1_params` / `sora_video_h264_params` / `sora_video_h265_params` (`Option<String>`) を追加する
2. 値は JSON 文字列として受け付け、nojson でパースして各 `Video*Params` 構造体に変換する。JSON キー名は C++ 版のパラメータ名と揃える
3. JSON が不正な場合はパースエラーで起動を拒否する
4. `src/main.rs` でパラメータが指定された場合のみ `Video::new_*(bit_rate, Some(params))` を使い、未指定時は現状の `None` を維持する
5. JSONC 設定ファイル経由では Object 値が `--sora-video-vp9-params {json}` にフラット展開される (`src/args.rs` の `flatten_sora_object()`) ため、そのままパースに乗る

## 完了条件

- 4 つのオプションが `--help` に表示される
- 指定したパラメータがシグナリングの SDP に反映される (offer の SDP で確認可能)
- 不正な JSON を指定するとエラーメッセージ付きで起動に失敗する
