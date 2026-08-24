# コーデックパラメータ (`--sora-video-vp9-params` 等) を追加する

- Created: 2026-08-03
- Completed: {YYYY-MM-DD}
- Branch: feature/add-video-codec-parameters
- Polished: 2026-08-24

## 目的

zakuro (C++) では `--sora-video-vp9-params` / `--sora-video-av1-params` / `--sora-video-h264-params` / `--sora-video-h265-params` でコーデック固有パラメータを JSON で指定できる (`zakuro/src/util.cpp` の `--sora-video-*-params` オプション)。コーデックのプロファイル・レベル・B フレームなどの細かな挙動を制御して負荷試験の再現性を高めるために必要。C++ 版との機能互換性を維持するために対応する。

## 現状

- `src/main.rs` の `build_video()` (`run_zakuro_instance()` から呼ばれる) はコーデック設定を `sora_sdk::Video::new_vp8()` / `new_vp9()` / `new_av1()` / `new_h264()` / `new_h265()` で構築しており、`new_vp9(args.video_bit_rate, None)` のようにパラメータには常に `None` を渡している
- sora_sdk には `VideoVP9Params` / `VideoAV1Params` / `VideoH264Params` / `VideoH265Params` と、`Video::new_vp9(bit_rate, vp9_params)` 等のパラメータ付きコンストラクタが存在するため、CLI 未配線の状態
- `docs/ZAKURO.md` には「コーデックパラメータ (`VideoVP9Params` 等) と `DegradationPreference` は各 SDK / バインディングに API がある。zakuro-rs の CLI 未配線が残っている。」と記載されている

## 設計方針

1. `src/args.rs` の `InstanceArgs` に `sora_video_vp9_params` / `sora_video_av1_params` / `sora_video_h264_params` / `sora_video_h265_params` (`Option<String>`) を追加する。`noargs::opt()` の値付きオプションとして定義し、`is_common_key()` / `is_flag()` には追加しない
2. 値は JSON 文字列として受け付け、nojson の `RawJsonValue` から各フィールドを手動抽出して `Video*Params` を構築する (sora_sdk の `Video*Params` には nojson 用の `TryFrom<RawJsonValue>` が実装されておらず、`Json<T>` で直接パースできない。`DisplayJson` によるシグナリングへの出力には使える)。JSON キー名と値の型は sora_sdk の各型の `DisplayJson` に準拠する (sora_sdk のキー名・型は Sora サーバーの検証と一致している: VP9 `profile_id` 0-3 / AV1 `profile` 0-2・`level_idx` 0-31・`tier` 0-1 / H264 `profile_level_id` 文字列・`b_frame` 真偽)
   - H265 の `level_id` だけは、sora_sdk が文字列型で保持するため (`DisplayJson` では `"level_id":"120"`)、Sora サーバーの検証 (整数 0-255) と一致しない。この値をサーバーに送ると h265_params 全体が拒否されるため、`level_id` は本 issue ではサポート対象外とし、指定された場合は起動時エラーとする (H265 の対応キーは `profile_id` 0-31 / `tier_flag` 0-1 / `tx_mode` SRST か MRST か MRMT / `b_frame`)
   - `b_frame` は Sora サーバー側の `sora.conf` 設定 (`h264_b_frame` / `h265_b_frame`) に依存し、サーバー設定が無効な状態で `b_frame=true` を送ると検証エラーになる。ドキュメントやエラーメッセージでその旨を明示する
   - コーデックパラメータを送る変換自体も、Sora サーバーの `sora.conf` 設定 (`signaling_vp9_params` / `signaling_av1_params` / `signaling_h264_params` / `signaling_h265_params`) が有効でないと受け付けられない (`sora_media_video.erl` の `validate_video()`)。動作確認の前提としてサーバー設定を確認する旨を明記する
3. JSON が不正な場合はパースエラーで起動を拒否する。未知キー・範囲外値も起動時エラーとする (C++ 版は素通しでサーバー側検証に任せるが、zakuro-rs はクライアント側で検証してから送信する。サーバー側は未知キーで h264/h265/vp9/av1 の params 全体を拒否するため、クライアント側検証が安全)
4. `src/main.rs` でパラメータが指定された場合のみ `Video::new_*(bit_rate, Some(params))` を使い、未指定時は現状の `None` を維持する
5. `--sora-video-*-params` を指定した場合は、対応するコーデック (`--sora-video-codec-type` が vp9 等) の指定が必須。codec type 未指定・不一致はパースエラーで起動を拒否する (`build_video()` は codec type 未指定時に `new_vp8()` へフォールバックするため、params が無視される事故を防ぐ)
6. JSONC 設定ファイル経由では Object 値が `--sora-video-vp9-params {json}` にフラット展開される (`src/args.rs` の `flatten_sora_object()`) ため、そのままパースに乗る

## 完了条件

- 4 つのオプションが `--help` に表示される
- 指定したパラメータがシグナリングの connect メッセージの video に含まれ、Sora サーバーが返す offer の SDP (fmtp) に反映される (Sora サーバー側の sora.conf 設定 `signaling_*_params` と `*_b_frame` が有効な場合)
- 不正な JSON、`level_id` の指定、params と codec type の不一致を指定すると、エラーメッセージ付きで起動に失敗する
