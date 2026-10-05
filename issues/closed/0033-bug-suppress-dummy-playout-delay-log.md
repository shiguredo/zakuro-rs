# dummy ADM 由来の playout delay エラーログを抑制する

- Created: 2026-10-05
- Completed: 2026-10-05
- Branch: feature/fix-suppress-dummy-playout-delay-log
- Polished: {YYYY-MM-DD}

## 目的

受信専用インスタンスの dummy ADM が出力し続ける無害なエラーログ
`failed to retrieve the playout delay` を出さないようにし、ログ量を削減して
負荷試験用ホストのディスク枯渇を防ぐ。

負荷試験用ホストでこのログが肥大しディスクが満杯になり、負荷試験が停止する障害が
発生した。恒久対処としてログの発生量そのものを減らす。

## 現状

- zakuro の受信側インスタンスは `AdmConfig::NoAudioDevice` で接続しており、
  sora-rust-sdk はこの指定で libwebrtc の dummy ADM
  (`AudioDeviceModuleAudioLayer::Dummy`) を生成する
- libwebrtc の `AudioDeviceModuleImpl::PlayoutDelay()` は下位 ADM が -1 を返すと
  LS_ERROR で `failed to retrieve the playout delay` を出力する。dummy ADM の
  `PlayoutDelay()` は常に -1 を返す実装である
- zakuro は受信音声を再生しないため playout delay の値を一切使わないが、受信音声
  チャネルを持つ VC ごとに `GetPlayoutRtpTimestamp()` 経由で高頻度に呼ばれ、この
  エラーログが出続ける
- 計測 (2026-10-05): あるログファイル 9,241,057 行のうち 8,912,061 行 (96.4 %) が
  このメッセージだった。1 サイクル (約 32 分) で約 800 MB、1 日で 30 GB 前後に達する
- `--log-level` では消せない。このメッセージは LS_ERROR のため error 指定でも残り、
  none を指定すると zakuro 自身のログまで消える
- 送信側は `FakeAudioCapturer` の自前 ADM を使うため、この経路ではエラーは出ない

## 設計方針

- libwebrtc の LogSink を追加し、メッセージの完全一致でこの 1 種類のログだけを落とす。
  このメッセージを出せるのは dummy ADM だけであり、zakuro で dummy ADM が使われるのは
  受信専用インスタンスだけなので、起動引数による切り替えは行わない
- libwebrtc の `LogMessage::~LogMessage()` は stderr への直接出力と sink への配信を
  別経路で行うため、sink に登録して捨てるだけでは stderr 行は止まらない。
  `set_log_to_stderr(false)` で直接出力を止め、sink 側で `default_log_line()` により
  抑制対象以外の全行を再出力する。既定のログの見た目は変えない
- sink の min severity は LS_INFO 固定 (`webrtc::LogSink` の private メンバで C API に
  setter がない) のため、`--log-level=warning` 以上を指定しても LS_INFO / LS_WARNING の
  行が sink に届く。`--log-level` の指定どおりに絞り込む処理を sink ハンドラ側に持たせる
- sink ハンドラは複数スレッドから呼ばれ得るため、可変状態を持たず、ログ初期化前に
  確定する最低重大度だけを保持する
- 既知の制約: `--log-level=verbose` の verbose 行は sink の min severity (LS_INFO) に
  弾かれて出力されなくなる。既定値は info であり、負荷試験用途では影響しない
- 代替案として、`AdmConfig::NoAudioDevice` をやめて playout delay で 0 を返す自前 ADM を
  `AdmConfig::UseExternal` で渡す方法 (C++ 版 zakuro と同じ構成) もある。ログは根本的に
  出なくなるが、現状 -1 により無効化されている音声同期の経路が有効になり計測値が変わり
  うるため、挙動を変えない LogSink 方式を採用する

## 完了条件

- dummy ADM 由来の `failed to retrieve the playout delay` がログに出力されないこと
- `--log-level` の info / warning / error / none の絞り込みが従来どおりであること
- zakuro 自身のログと他の libwebrtc のログは従来どおり出力されること
- `cargo fmt --all -- --check` / `cargo clippy --locked --workspace --all-targets
  --features fdk-aac -- -D warnings` / `cargo test --locked --workspace --features fdk-aac`
  が通ること

## 解決方法

- libwebrtc の LogSink を登録する `log_filter` モジュールを追加し、dummy ADM 由来の
  `failed to retrieve the playout delay` をメッセージの完全一致で破棄する
- libwebrtc の `LogMessage::~LogMessage()` は stderr への直接出力と sink への配信を
  別経路で行うため、ログ初期化で `set_log_to_stderr(false)` を設定し、sink が
  `default_log_line()` で抑制対象以外の全行を stderr へ再出力する
- sink の min severity は LS_INFO 固定で `--log-level` の絞り込みが効かない。このため
  sink ハンドラ側でも同じ最低重大度で絞り込み、`warning` / `error` / `none` の指定どおりに
  出力する
- `--log-level=verbose` の verbose ログは sink に届かないため出力されない。この制約を
  README に記載した
- 検証 (ホスト): 同一設定・`--duration 60` の試験で該当メッセージ 0 件、`[stats]` 332 件、
  他の libwebrtc のログ 93,884 件を確認した。修正前の同一設定のサイクルでは
  6,100,856 / 6,354,455 行 (96.0 %) が該当メッセージだった
- 検証 (ホスト): release ビルドを置き換えた後の実サイクルでも該当メッセージ 0 件
- 検証 (ローカル): `cargo fmt --all -- --check` / `cargo clippy --locked --workspace
  --all-targets --features fdk-aac -- -D warnings` / `cargo test --locked --workspace
  --features fdk-aac` が通る
