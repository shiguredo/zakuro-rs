# PUBLISH の受理を待ってから SUBSCRIBE を送る

- Created: 2026-10-19
- Completed: 2026-10-19
- Branch: feature/add-publish-acceptance-wait
- Polished: {YYYY-MM-DD}

## 目的

zakuro-moq が同一仮想クライアントで publish と subscribe を行うとき、relay が publisher を
登録する前に SUBSCRIBE を送って拒否され、E2E Test が失敗するのを解消する。

## 現状

- セッション確立後、全トラックの PUBLISH を送り、REQUEST_OK を待たずに SUBSCRIBE を送る
- relay は publisher を登録する前に同じトラックへの SUBSCRIBE を受けると REQUEST_ERROR
  (publisher なし) で拒否する
- E2E Test (run 39202160876) は
  `subscribe rejected: request_id=2 ... reason=ReasonPhrase("<<\"no publisher for track\">>")`
  で失敗した。同じ失敗は 0047 の push (run 38597257727) でも発生しており、E2E は継続的に
  失敗していた
- publish の受理は `drain_events` 内の `SessionEvent::RequestOkReceived` で
  `Publisher::on_accepted` を呼ぶ経路でのみ確認される

## 設計方針

- 全 publication が受理されるか、PUBLISH をまだ送っていない状態になるまで、SUBSCRIBE の送信を
  次のループ反復へ遅らせる
- 判定を純関数 `publishers_ready_for_subscribe` に切り出して単体テストする
- relay が PUBLISH に応答しない場合は、制御メッセージのタイムアウト (10 秒) がセッション
  エラーとして検出するため、無限には待たない
- publish のみ / subscribe のみの利用には影響しない

## 完了条件

- `publishers_ready_for_subscribe` の単体テストがある
- `cargo fmt --all -- --check` / `cargo clippy --locked --workspace --all-targets
  --features fdk-aac -- -D warnings` / `cargo test --locked --workspace --features fdk-aac` /
  `cargo build --locked --workspace --features fdk-aac` が通ること
- E2E Test が success になること (push 後に GitHub Actions で確認する)

## 解決方法

- `session::run` の確立後処理で `publishers_ready_for_subscribe` を判定し、全 PUBLISH の受理前は
  SUBSCRIBE を送らないようにした
- 受理待ちを info ログ (`MOQT waiting for publish acceptance before sending SUBSCRIBE`) に
  1 回だけ出す
- 検証: `cargo test -p zakuro-moq` 65 件 pass、`make ci` exit 0
- E2E Test はこの修正を push した実行で確認する
