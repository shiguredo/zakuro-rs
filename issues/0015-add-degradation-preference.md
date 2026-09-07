# degradation-preference オプションを追加する

- Created: 2026-03-27
- Completed: {YYYY-MM-DD}
- Branch: feature/add-degradation-preference
- Polished: {YYYY-MM-DD}

## 目的

`--degradation-preference` オプションを追加し、帯域不足時の映像品質低下戦略を選択できるようにする。zakuro (C++) では `disabled` / `maintain_framerate` / `maintain_resolution` / `balanced` を選択でき、負荷試験において帯域制限下での挙動を制御するために使う。C++ 版との機能互換性を維持するために対応する。

## 現状 (2026-08-26 確認)

### webrtc-rs (`shiguredo_webrtc` 0.152.1-canary.0): 対応済み

- `DegradationPreference` enum と `RtpParameters::set_degradation_preference` / `degradation_preference` が利用可能
- 値は `MaintainFramerateAndResolution` / `MaintainFramerate` / `MaintainResolution` / `Balanced`
- C++ の `disabled` は libwebrtc では `MAINTAIN_FRAMERATE_AND_RESOLUTION` と同値 (W3C の 4 値のうち disabled 相当がこの値)

### sora-rust-sdk (`sora_sdk` 2026.2.0-canary.0): 未対応

- `SoraConnectionBuilder` に `degradation_preference` の設定口がない
- `shiguredo/sora-rust-sdk` の `docs/SORA_CPP_SDK.md` / `docs/SUMOMO.md` でも「未実装」
- `shiguredo/sora-rust-sdk` の open issue `issues/0151-add-degradation-preference.md` で追加予定
  - ネゴシエーション後 (set_remote_description 成功後、create_answer 前) に video sender の `RtpParameters` へ `SetParameters` する方針
  - C++ SDK の `SoraSignalingConfig::degradation_preference` と同等 (シグナリングには含めないクライアント側設定)

### zakuro-rs

- CLI / `InstanceArgs` への配線は未実装 (本 issue)
- `docs/ZAKURO.md` に未実装として記載済み

## 依存関係

本 issue の実装は sora-rust-sdk の DegradationPreference 設定 API (0151) が前提。webrtc-rs だけでは zakuro-rs から安全に適用できない (video sender への `SetParameters` タイミングを sora-rust-sdk 内で行う必要がある)。0151 が closed になり、対応バージョンの `sora_sdk` を取り込んだうえで本 issue に着手する。

## 設計方針

1. `src/args.rs` の `InstanceArgs` に `--degradation-preference {disabled,maintain_framerate,maintain_resolution,balanced}` を追加する (C++ 版と同じ 4 値。小文字厳密)
2. `disabled` は `shiguredo_webrtc::DegradationPreference::MaintainFramerateAndResolution` にマップする
3. `SoraConnectionBuilder::degradation_preference(...)` (0151 で追加予定) に渡す
4. JSONC からも指定できるようにする (他の InstanceArgs と同様)

## 完了条件

- `--degradation-preference` で指定した値が、接続後の video sender の `RtpParameters.degradation_preference` に反映される
- 未指定時は従来どおり (SDK 既定値) の挙動になる
- 不正な値は起動時エラーになる
