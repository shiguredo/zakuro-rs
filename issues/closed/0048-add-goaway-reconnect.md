# GOAWAY 受信時に New Session URI へ再接続する

- Created: 2026-10-19
- Completed: 2026-10-19
- Branch: feature/add-goaway-reconnect
- Polished: {YYYY-MM-DD}

## 目的

MOQT relay の drain (再起動・ローリング更新) で GOAWAY が届いても、zakuro-moq の仮想クライアントが
新セッションへ移行して負荷を継続できるようにする。

draft-ietf-moq-transport-22 §9.2 (GOAWAY) は、New Session URI が示された場合にクライアントが
その URI を新セッションに使うことを MUST としている。§6.6.1 (Graceful Session Migration) は
GOAWAY をセッション移行の合図とし、移行をアプリケーションから透過に行うことを理想としている。
現状は GOAWAY をログに出すだけで移行しないため、relay の drain のたびに負荷試験の
仮想クライアントが全停止する。

## 現状

- `SessionEvent::GoawayReceived` は `timeout` だけを warn ログに出し、`new_session_uri` と
  `on_request_stream` を捨てている (`moq_client::session::drain_events`)
- relay が GOAWAY の Timeout 経過後に GOAWAY_TIMEOUT (0x10) で QUIC を閉じると、
  制御ストリームの read が失敗して `session::run` が Err で終わる。`moq_client::run` はこれを
  「想定外切断」として扱い、既定 `--max-retry 0` のため Stopped になり再接続しない
- 実測 (`client-drain.log`): GOAWAY (timeout=5000ms) と New Session URI を受信後、5 秒で 0x10
  により切断され、仮想クライアントが停止した
- 0x10 は GOAWAY メッセージの Type と同じ数値だが別の番号空間 (Session Termination Code) である
- moqt-rs は 1 接続 1 Session であり、旧 Session の状態を新 Session へ引き継ぐ API は無い
- `MoqClientContext` は起動時のホスト / ポートの名前解決結果・SNI・UDP ソケットを固定している

## 設計方針

- セッション移行の対象は制御ストリームの GOAWAY のみとする。request stream の GOAWAY は
  当該 request のみを移行する SHOULD があるが、zakuro は 1 セッションに全トラックを載せており
  per-request migration を実装しないため、既知の未対応として info ログのみ出す
  (relay が当該 request を reset すると既存のリトライ経路に入る)
- GOAWAY 受信時は旧セッションを FIN / client GOAWAY (空 URI) / NO_ERROR で閉じ、新セッションを
  張り直す (close-then-connect)。仕様の理想は新セッションをバックグラウンドで確立する
  dual-session 移行だが、初版は単純さを優先する
- New Session URI が空の場合は「current URI を再利用」として、いま使っている URI へ再接続する
- New Session URI が非 UTF-8、または `moqt://` として解釈できない場合は、現在 URI へ黙って
  フォールバックせず仮想クライアントを停止する (malformed URI の規定は仕様に無いが、誤った
  接続先へ繋がない)
- 設定 `--reconnect-on-goaway <true|false>` を追加し、既定を true にする。false のときは
  従来挙動 (relay が閉じるまで publish を続け、その後停止) を維持する
- 移行は計画切断として扱い、`retry_count` をリセットして即座に再接続する。`Retrying` イベントは
  移行成功時には出さない (失敗ではない)
- 移行先の host / port が変わっても接続できるよう、`MoqClientContext` を host / port をキーに
  キャッシュする (インスタンス内で共有し、ソケットが仮想クライアント数だけ増えないようにする)
- `--duration` はセッション単位のままとし、移行でタイマーがリセットされることを docs に明記する

## 完了条件

- 制御ストリームの GOAWAY 受信から新セッション確立までの流れが `session::run` → `moq_client::run`
  で実装されている
- GOAWAY 受信時に PUBLISH / SUBSCRIBE の新規 request を送らないこと
- `--reconnect-on-goaway` の既定値が true で、false を指定すると従来挙動になる
- New Session URI の解決 (空 URI / `moqt://` / IPv6 / 不正値) の単体テストがある
- 接続先キャッシュのキーが host / port のみで path に依存しないことの単体テストがある
- `--reconnect-on-goaway` の解釈と JSONC の bool 変換の単体テストがある
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
- `cargo build --locked --workspace --features fdk-aac` が通ること
- README の MOQ オプションと JSONC 例、`docs/MOQ_GOAWAY.md` が実装に合っていること

## 解決方法

- `moq_client::session` に `SessionEnd` (`Closed` / `Migrated`) を追加し、`session::run` の
  戻り値を `Result<SessionEnd>` に変更した。`SessionConfig` に `migrate_on_goaway` を追加し、
  `SessionState` に GOAWAY を保持する `goaway: Option<GoawayInfo>` を追加した
- `drain_events` の `GoawayReceived` で `new_session_uri` / `timeout` / `on_request_stream` を
  すべて受け取るようにした。セッション移行の対象は制御ストリームの GOAWAY のみで、その内容を
  `GoawayInfo` として記録する。request stream の GOAWAY は per-request migration が未対応で
  あることを info ログに出す。URI はログ注入を避けて Debug 表記で出す
- 移行判定を `should_migrate` に集約し、ループ先頭、PUBLISH / SUBSCRIBE を送る前、メインループ
  末尾で判定するようにした。GOAWAY 受信後に新規 request を送ると moqt-rs が
  `SendRequestError::PeerGoawayReceived` を返してセッションエラーになるため、初期 request の
  送信前に移行する
- メインループで GOAWAY 保持かつ移行有効なら break し、既存の終了処理 (FIN / client GOAWAY /
  `session.close(0x0)`) を通して `SessionEnd::Migrated` を返す。移行無効時は従来どおり
  peer がセッションを閉じるまで publish を続け、`SessionEnd::Closed` を返す
- `moq_client::run` で現在の接続先 `current_endpoint` を保持し、`SessionConfig` を接続先ごとに
  作り直すようにした。`SessionEnd::Migrated` を受けたら `resolve_next_endpoint` で移行先を
  決定し、`retry_count` をリセットして即座に再接続する。移行先の解決に失敗した場合はログを出して
  仮想クライアントを停止する。移行ログは `moqt://` URI 形式で old -> new を出し、URI は Debug
  表記にして query を伏せる
- `resolve_next_endpoint` は空 URI なら現在の接続先、非空なら
  `MoqEndpoint::parse_new_session_uri` の結果を返し、非 UTF-8 や `moqt://` 以外はエラーにする。
  `parse_new_session_uri` は CLI 引数ではない URL のエラーに `--url` を含めない
- `moq_client::transport` の `MoqClientContext` を、host / port をキーにした
  `EndpointKey` で接続先ごとの `EndpointState` を
  `Arc<tokio::sync::Mutex<HashMap<EndpointKey, Arc<EndpointState>>>>` にキャッシュする構造に
  変更した。path / query だけが違う移行で UDP ソケットを作り直さない。キャッシュヒット時も
  MutexGuard を接続処理の await 中に保持しないよう、guard を落としてから接続する。host は
  小文字に正規化してキーにする
- `args` に `--reconnect-on-goaway <true|false>` (既定 true) を追加し、`InstanceArgs` /
  `MoqClientConfig` / `main` の起動ログへ反映した。JSONC では値付きオプションとして
  `"reconnect-on-goaway": false` と書ける
- ドキュメントを更新した: `README.md` の MOQ の説明・オプション表・JSONC 例、
  `docs/LOADTEST_GAP.md` の GOAWAY 移行の行と、今回の変更で陳腐化した参照
  (moq_client.rs / transport.rs / main.rs / args.rs / README.md) をシンボル名へ修正、
  `docs/MOQ_GOAWAY.md` の決定事項と残課題
- 検証: `make ci` (fmt-check / clippy / test / smoke) が exit 0。
  `cargo test --locked --workspace --features fdk-aac` は 386 件 pass
  (zakuro 314 / zakuro-core 8 / zakuro-moq 64)。`cargo build --locked --workspace
  --features fdk-aac` と `zakuro-moq --help` で `--reconnect-on-goaway` の表示を確認した
- 実 relay での GOAWAY 移行の確認はこの環境では未実施 (relay の drain を起こす手段が無い)。
  手動確認時は relay の再起動中に `MOQT session migration:` ログと新セッションの established が
  出ること、`--reconnect-on-goaway=false` では従来どおりの経路になることを確認する。
  実 peer が必要な結合フロー (GOAWAY 受信から再接続まで) は単体テストできないため、
  手動確認と実 relay を使う E2E の整備を残課題とする
