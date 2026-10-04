# shiguredo_webrtc を 0.154 に揃えて zakuro のビルドを直す

- Created: 2026-10-04
- Completed: {YYYY-MM-DD}
- Branch: feature/update-shiguredo-webrtc
- Polished: {YYYY-MM-DD}

## 目的

`zakuro` が `shiguredo_webrtc` 0.152.1-canary.1 に依存し続けており、`sora_sdk` が要求する
0.154 と同じ crate の別バージョンとして共存している。両者の型は互換性がないため、
`zakuro` のビルドが型不一致で失敗する。0.154 に揃えてビルドを回復する。

## 現状

- 直前のコミット (rustup、`Cargo.toml` の `rust-version` 更新) で `Cargo.lock` が更新され、
  `sora_sdk` が 2026.2.0-canary.6 になった。`sora_sdk` は `shiguredo_webrtc` を `~0.154`
  で要求する
- `zakuro/Cargo.toml` は `shiguredo_webrtc = "0.152.1-canary.1"` のままで、依存グラフに
  `shiguredo_webrtc` 0.152.1 と 0.154.0 が共存する
- `cargo build -p zakuro --features fdk-aac` が 21 件のエラー (E0308 / E0053 / E0277) で
  失敗する。エラーはすべて `shiguredo_webrtc` の複数バージョン共存に由来する
  (例: `zakuro/src/main.rs` の `VideoCodecType` と `sora_sdk` が要求する `VideoCodecType`
  が別 crate の型になる)
- この破損は rustup コミットの時点で既に存在し、moqt-rs 追従の変更とは無関係

## 設計方針

- `zakuro/Cargo.toml` の `shiguredo_webrtc` を `0.154` に上げ、`sora_sdk` が要求する
  `~0.154` と同じ crate バージョンに統一する
- 0.154 の最新安定版は 0.154.0。0.152.1-canary.1 との API 差分は `VideoEncoderFactoryHandler` /
  `VideoDecoderFactoryHandler` のデフォルト実装削除などだが、`zakuro` は明示実装済みのため
  ソースコードの変更は不要
- 他の crate の多重バージョンは間接依存由来で型の受け渡しに影響しないため触らない
- `Cargo.lock` / `THIRD_PARTY_LICENSES.md` / `docs/ZAKURO.md` を追随させる。
  `THIRD_PARTY_LICENSES.md` は rustup コミットの `Cargo.lock` 更新時に再生成されて
  いなかったドリフト (間接依存のバージョン・ライセンス表記) も含めて再生成する

## 完了条件

- `cargo build -p zakuro --features fdk-aac` が通ること
- `make ci` (fmt / clippy / test / smoke) が通ること
- 依存グラフ上の `shiguredo_webrtc` が 0.154 の 1 バージョンになること

## 変更対象

- `zakuro/Cargo.toml`
- `Cargo.lock`
- `THIRD_PARTY_LICENSES.md`
- `docs/ZAKURO.md`
