# ログ抑制パターンを --log-suppress で指定できるようにする

- Created: 2026-10-05
- Completed: {YYYY-MM-DD}
- Branch: feature/add-log-suppress-option
- Polished: {YYYY-MM-DD}

## 目的

libwebrtc や sora-rust-sdk が出す大量のログのうち、負荷試験の運用者が「このメッセージは
要らない」と判断したものを、設定だけで抑制できるようにする。

現状は dummy ADM 由来のメッセージをコードにハードコードして抑制しているが、同じように
大量に出るログは他にもある。`--log-level` は閾値でしか絞れないため、特定のメッセージだけを
外すにはレベルを上げて他のログも巻き込んで消すか、コードに組み込むしかない。

## 現状

- `log_filter` モジュールは dummy ADM の `failed to retrieve the playout delay` 1 種類だけを
  完全一致で抑制している
- `--log-level` では「閾値より軽いログを全部消す」ことしかできない
- 負荷試験の実サイクル (2026-10-05) では、抑制後のログの内訳は INFO が約 85 %、
  WARNING が約 13.5 % だった
  - INFO: `basic_port_allocator` のネットワーク一覧、`transport_feedback_adapter` の
    send time lookup 失敗、`sora_sdk::connection` の WebSocket 送受信、`rtp_streams_synchronizer2`
    の同期統計、`rtp_video_stream_receiver2` のパケット受信、`webrtc_video_engine` の受信統計など
  - WARNING: `packet_buffer` の `Packet buffer fully flushed.`
- これらは運用者にとって必要な情報でもあり、既定で消すべきではない

## 設計方針

- CLI `--log-suppress <SUBSTRING>[,<SUBSTRING>...]` と JSONC 最上位の
  `"log-suppress": ["...", ...]` を追加する
- 値は部分文字列として扱い、ログのメッセージ本体または発生元ファイル名のいずれかに
  含まれていれば、その行を出力しない
  - ファイル名 (`transport_feedback_adapter.cc` など) を指定すると、そのファイルからの
    ログをまとめて抑制できる
- dummy ADM の抑制はコード組み込みのまま残す (指定不要で常に有効)
- ログ初期化は引数パースより前に走るため、`--log-level` と同じく CLI / JSONC を覗き見る
  経路で読む。値の優先順位も同じく CLI が JSONC に勝つ
- カンマ区切りの各要素は前後の空白を取り除く。空要素は「全メッセージに一致してしまう」
  指定ミスなので起動エラーにする
- 抑制したログは stderr へ再出力しない。抑制対象以外の出力経路は変えない

## 完了条件

- `--log-suppress` / JSONC `"log-suppress"` で指定した文字列に一致するログが出力されない
- 未指定の場合は従来どおり出力される
- dummy ADM の抑制は指定なしで従来どおり有効
- `--log-level` の絞り込みは従来どおり
- `cargo fmt --all -- --check` / `cargo clippy --locked --workspace --all-targets
  --features fdk-aac -- -D warnings` / `cargo test --locked --workspace --features fdk-aac`
  が通ること
