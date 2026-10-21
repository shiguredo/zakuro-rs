# MOQT GOAWAY 移行 (New Session URI への再接続) 検討メモ

zakuro-moq が GOAWAY を受信したときに New Session URI へ再接続する仕組みの検討結果と実装方針。
仕様は draft-ietf-moq-transport-22 (2026-10-01 発行、最新の公開 I-D)、依存は
`shiguredo_moqt` rev `49c711e` を前提とする。本文書は実装前の検討内容を残したものであり、
実装は本文書の「結論」と「設計」に従っている。本文中の `ファイル:行` は実装前の検討時点のものを
含むため、現在のソース上の位置はシンボル名で検索すること。

## 結論

- 再接続は実装する。初版は「GOAWAY を受けたら、旧セッションを NO_ERROR で閉じ、新セッションを
  張り直す」(close-then-connect)。
- 設定は `--reconnect-on-goaway <true|false>` (既定 `true`) を追加する。デフォルト有効でよい。
- New Session URI が空なら「current URI を再利用」として、いま使っている URI へ再接続する
  (停止ではない)。
- セッション移行の対象は制御ストリームの GOAWAY のみ。request stream の GOAWAY は当該 request
  だけを移行する SHOULD があるが、zakuro は 1 セッションに全トラックを載せているため
  per-request migration を実装していない (既知の未対応。relay が当該 request を reset すると
  既存のリトライ経路に入る)。
- New Session URI が不正 (非 UTF-8 / `moqt://` として解釈できない) な場合は、現在 URI へ
  黙ってフォールバックせず、VC を停止する (誤った接続先へ繋いで relay の誤りを隠さない)。

## 1. 一次資料の規範

出典: https://www.ietf.org/archive/id/draft-ietf-moq-transport-22.txt
(§ 番号は -22 のもの。引用は原文のまま)

### 1.1 §6.6.1 Graceful Session Migration

> The server sends a GOAWAY message, signaling the client to establish a new session and
> migrate any Established subscriptions. The GOAWAY message optionally contains a new URI
> for the new session, otherwise the current URI is reused.

> After the client receives a GOAWAY, it's RECOMMENDED that the client waits until there are
> no more Established subscriptions before closing the session with NO_ERROR. Ideally this is
> transparent to the application using MOQT, which involves establishing a new session in the
> background and migrating Established subscriptions and published namespaces.

> A GOAWAY on the control stream migrates the entire session, as described in this section.
> A GOAWAY on a single request stream instead migrates only that request, leaving the rest of
> the session in place; see Section 9.2.

- GOAWAY は drain の合図であり、relay の再起動・停止を待たずにクライアントを移行させる
  ための仕組みである。
- セッション全体の移行を義務づける MUST / SHOULD はない (§7.4.1 / §7.6.1 が MAY 実装の挙動
  として記述する)。ただし移行して新セッションを張る場合、URI の使い方には MUST がある (1.2)。
- request stream の GOAWAY は当該 request のみの移行であり、§9.2 が指定 URI (無い場合は
  現在のセッション) での再発行を SHOULD としている。本実装は per-request migration を行わない
  (既知の未対応)。

### 1.2 §9.2 GOAWAY

> New Session URI: When received by a client, indicates where the client can connect to
> continue this session or re-issue this request. The client MUST use this URI for the new
> session if provided. If the URI is zero bytes long, the current URI is reused instead. The
> new session URI SHOULD use the same scheme as the current URI to ensure compatibility. The
> maximum length of the New Session URI is 8,192 bytes.

> Timeout: The time in milliseconds the sender will wait for graceful closure. When sent on the
> control stream, the sender closes the session with GOAWAY_TIMEOUT after the indicated timeout
> if there are still open requests. [...] A value of 0 indicates the sender has no specific
> timeout, but the recipient SHOULD migrate as quickly as possible. This is a hint; the sender
> of the GOAWAY MAY close the session or reset the request stream before the indicated timeout
> has elapsed.

> Upon receiving a GOAWAY on the control stream, an endpoint SHOULD NOT initiate new requests
> to the peer including SUBSCRIBE, PUBLISH, FETCH, PUBLISH_NAMESPACE, SUBSCRIBE_NAMESPACE,
> SUBSCRIBE_TRACKS and TRACK_STATUS.

> A GOAWAY MAY also be sent on a request stream to initiate migration of that individual
> request. Upon receiving a GOAWAY on a request stream, the endpoint SHOULD re-issue that
> specific request on a session at the specified URI (or the current session if no URI is
> provided), and close the old request stream using the appropriate mechanism (e.g. FIN, stream
> reset, or PUBLISH_DONE).

- Timeout は GOAWAY 送信側 (このケースでは relay) が旧セッションを閉じるまでの猶予で、
  hint にすぎない。zakuro 側がこの Timeout を待つ必要はない。
- 新規 request (PUBLISH / SUBSCRIBE など) を GOAWAY 後に出さないのは、control stream の
  GOAWAY に対する SHOULD NOT である。既存の subscription / publish の継続は妨げられない。
- request stream の GOAWAY は当該 request のみの移行であり、指定 URI (無い場合は現在の
  セッション) での再発行が SHOULD。本実装は per-request migration を実装していない。
- 移行先 URI の同一 origin 制約はない (scheme を揃える SHOULD のみ)。別 authority も
  仕様上は許される。認可 token の引き継ぎや malformed URI の扱いは規定がない。
- 「current URI」の厳密な定義は規定がない。本メモでは「移行対象のセッションを張った URI」
  と解釈する (移行を繰り返す場合は直前の URI)。

### 1.3 §12.2 GOAWAY_TIMEOUT (0x10)

> GOAWAY_TIMEOUT (0x10): The session was closed because the peer took too long to close the
> session in response to a GOAWAY (Section 9.2) message.

- 0x10 は「GOAWAY を送った側が、peer が猶予内に閉じなかったとき」の Session Termination
  Code であり、GOAWAY メッセージの Type 0x10 とは別の番号空間である。ログで混同しないこと。

## 2. 実装前の挙動 (実測とコード)

実測 (`client-drain.log`): relay から GOAWAY (timeout=5000ms) と New Session URI が届き、
zakuro-moq はそのまま publish を続け、relay が 5 秒後に GOAWAY_TIMEOUT (0x10) で閉じた。
その後 VC は再接続せず停止する。以下は実装前のコードの流れである。

コード上の流れ:

1. `SessionEvent::GoawayReceived { timeout, .. }` を warn ログに出すだけで、
   `new_session_uri` / `on_request_stream` を捨てている
   (`zakuro-moq/src/moq_client/session.rs:1674-1676`)。
2. relay の QUIC CONNECTION_CLOSE (0x10) は stream エラーとして現れ、制御ストリームの
   read が Err になるため `session::run` は **Err** で終わる
   (`session.rs:689-690`)。peer の close code/reason を Session へ渡す API は moqt-rs に
   無いため、コードは文字列にしか現れない。
3. `moq_client::run` はこれを「想定外切断」として `retry_count += 1` し、既定
   `--max-retry 0` のため Stopped になる (`moq_client.rs:254-289`)。
4. 理論上は acceptor がエラーなしで閉じて `Ok(())` になる競合もある (`session.rs:822,832`)。
   **Ok/Err だけで GOAWAY 起因かを判定してはならない**。GOAWAY の情報は
   `SessionEvent` から構造化して取り出す必要がある。

moqt-rs f91baf3 の受信側挙動:

- 制御ストリームの GOAWAY 受信で `SessionEvent::GoawayReceived` を発行する
  (`src/session/goaway.rs:472-500`、イベント定義 `src/session/types.rs:317-327`)。
  フィールドは `new_session_uri: Vec<u8>`、`timeout: u64`、`on_request_stream: Option<u64>`。
- 受信側は GOAWAY のタイマーを持たず、何も自動で閉じない。0x10 を送るのは GOAWAY 送信側。
- GOAWAY 受信後は `send_subscribe` / `send_publish` / `send_fetch` / `send_track_status` が
  `SendRequestError::PeerGoawayReceived` を返す (`src/session/core.rs:1985-1995`)。
  既存 subscription の継続と data 送信は可能。
- New Session URI の検証はバイト長 8192 と role 制約だけ。URI の形式検証・パース API は
  無いため、`moqt://` としての解釈は zakuro 側の責務 (`MoqEndpoint::parse` が使える)。
- 1 接続 1 Session であり、旧 Session から新 Session へ状態を移す API は無い。request_id・
  track alias・subscription はすべて再接続後に再確立する (zakuro はもともとセッションを
  張り直すたびに PUBLISH / SUBSCRIBE をやり直す構造なので、この点は追加実装が不要)。

## 3. 設計

### 3.1 セッション層 (`zakuro-moq/src/moq_client/session.rs`)

実装は次のとおり。

- `SessionState` に `goaway: Option<GoawayInfo { new_session_uri: Vec<u8>, timeout_ms: u64 }>`
  を追加した。
- `drain_events` の `GoawayReceived` で `new_session_uri` / `timeout` / `on_request_stream` を
  すべて受け取る。セッション移行の対象は制御ストリームの GOAWAY のみで、その内容を
  `GoawayInfo` として記録する。request stream の GOAWAY は per-request migration が未対応で
  あることを info ログに出す (記録しない)。URI はログ注入を避けるため Debug 表記でログに出す。
- 移行判定は `should_migrate(has_goaway, migrate_on_goaway, cancelled)` に集約した。
  GOAWAY 受信後に新規 request を送らないよう、ループ先頭と PUBLISH / SUBSCRIBE を送る前、
  メインループ末尾で判定する。これは draft-ietf-moq-transport-22 §9.2 (GOAWAY) の SHOULD NOT
  と、moqt-rs が `send_publish` / `send_subscribe` を
  `SendRequestError::PeerGoawayReceived` で拒否する挙動に合わせるためである。
- メインループ末尾でも判定して break する。break 後の既存終了処理が publisher stream の
  FIN → client GOAWAY (空 URI) → `session.close(0x0)` を行うため、旧セッションは NO_ERROR で
  閉じられる。
- `session::run` の戻り値を `Result<SessionEnd>` に変更した
  (`SessionEnd::Closed` / `SessionEnd::Migrated { new_session_uri, timeout_ms }`)。
  `moq_client.rs` の session future の扱いも Output を保持するよう変更した。
- 再接続を無効にしたときは従来挙動 (GOAWAY 後も publish を続け、relay が閉じるまで待つ) を
  維持するため、`SessionConfig` に `migrate_on_goaway` を渡して break を制御する。

### 3.2 クライアント層 (`zakuro-moq/src/moq_client.rs`)

- `config.endpoint` を直接使わず、ループ内で `current_endpoint: MoqEndpoint` を持ち回し、
  `SessionConfig` を接続先ごとにループ内で作り直す (移行先の authority / path を SETUP に載せる
  ため)。
- 移行先の決定は純関数に切り出してテスト可能にする。

  ```
  fn resolve_next_endpoint(current: &MoqEndpoint, new_session_uri: &[u8]) -> Result<MoqEndpoint>
  ```

  - 空 bytes → `current` の複製 (仕様の「current URI を再利用」)
  - UTF-8 として不正、または `MoqEndpoint::parse_new_session_uri` の失敗 → エラー (VC 停止)
- セッションが `SessionEnd::Migrated` で終わったら:
  - 移行有効: ログを出して `retry_count = 0`、待機せず即 continue。`Retrying` イベントは出さない。
    `--max-retry` は接続失敗時の予算なので移行では消費しない。
  - 移行無効: セッション層が GOAWAY で break しないため、従来どおり peer が閉じるまで
    publish を続ける。peer が 0x10 で閉じた後の扱いは既存のリトライ経路のままで、既定
    `--max-retry 0` では Stopped、指定時は `Retrying` を経て再試行し得る。
- 移行先への接続失敗は既存の接続失敗経路 (`moq_client::run` の接続失敗ブランチ) にそのまま乗せ、
  `--max-retry` / `--retry-interval` を適用する。
- 移行回数をログに出す (`migrations=`)。負荷試験のレポートで「何回 drain を跨いだか」が
  分かるようにする。

### 3.3 接続コンテキスト (`zakuro-moq/src/moq_client/transport.rs`)

`MoqClientContext` は初期接続先の host / port / SNI / 名前解決結果と、経路 (QUIC 直接接続 /
QMux) ごとの TLS 設定を持つ。host か port が変われば接続先ごとの状態を作り直す必要があり、
authority / path だけの変更なら `SessionConfig` の作り直しだけで済む。
QMux 経路では UDP ソケットを共有せず、仮想クライアントごとに TCP + TLS 接続を張る。

実装は次のとおり。

- `MoqClientContext` に endpoint ごとの `EndpointState` のキャッシュを持たせた
  (インスタンス内で `Arc<tokio::sync::Mutex<HashMap<EndpointKey, Arc<EndpointState>>>>` を
  共有)。VC ごとに再構築すると、1000 VC が同時に移行した場合に UDP ソケットが VC 数だけ
  増えるため。
- キャッシュキーは host と port だけの `EndpointKey` とした。名前解決・SNI・UDP ソケットは
  host / port で決まり、authority / path は `SessionConfig` 側で扱うため、path / query だけが
  異なる New Session URI でソケットを作り直さない。
- 名前解決と QUIC 接続の await を Mutex の保持中に行わない。キャッシュの読み書きだけを
  ロックで保護し、キャッシュヒット時も guard を落としてから接続する。初回構築はロックを
  持たずに行い、同じ移行先へ同時に接続した場合は先に登録された状態を使い、余分に作った状態は
  drop する。
- TLS クライアントは 1 回だけ構築する。
- QMux 経路では UDP ソケットも s2n-quic のクライアントも作らず、`EndpointState` は名前解決結果と
  SNI だけを持つ。TCP + TLS 接続は仮想クライアントごとに張るため、QUIC のソケット共有に相当する
  再利用はしない (QMux の接続は `qmux.rs` の `connect`)。
- `--insecure` / `--ca-cert` は移行先にもそのまま適用する。移行先の証明書が `--ca-cert` の
  範囲外なら TLS 接続に失敗し、既存のリトライ経路で停止する。
- 使われなくなった接続先のキャッシュ削除 (eviction) は未実装。多数の異なる relay へ移行し
  続けるとソケットが残るため、必要なら別途対応する。

### 3.4 設定

- `InstanceArgs` (インスタンス設定) に bool を追加する。URL ごとに移行可否を変えられる
  ほうが自然である。
- CLI: `--reconnect-on-goaway <true|false>` (既定 `true`)。
  - `--sora-audio` と同じ「既定有効の bool を値付き opt で受ける」流儀
    (`zakuro/src/args.rs:1458-1466`)。help に `(true/false, default: true)` を明記する。
  - 代替案は `--no-goaway-reconnect` (値なしフラグ、既定有効)。`--no-duckdb-output`
    (`zakuro/src/args.rs:1013-1017`) と同じ流儀だが、JSONC が `"no-goaway-reconnect": true`
    になり二重否定で読みにくい。
- JSONC: `"reconnect-on-goaway": false` と書けること。値付き opt なので `is_flag()` には
  追加しない (追加すると true/false が値として積まれず、逆に値なしフラグ扱いになる)。
  `push_kv` の bool 分岐がそのまま動く。
- `parse_instance_args` の `noargs::opt` の `present_and_then` で `true` / `false` 以外は
  エラーにする (`validate_instance_args` ではない)。
- main の起動ログに設定値を追加する。
- README の MOQ オプション表と JSONC 例を更新する。

### 3.5 統計・ログ

- 移行成功時は既存の `StatsEvent::Disconnected` → `StatsEvent::Connected` の 1 対で表現する
  (既存の送出経路がそのまま使える)。移行成功時は `Retrying` / `Stopped` は出さない
  (移行先への接続失敗時は既存のリトライ経路により出る)。
- `StatsSnapshot` は connected の増減しか見ないため、移行回数だけ Connected/Disconnected が
  増えても整合は壊れない (`zakuro-core/src/stats.rs` の `StatsSnapshot::apply`)。
- ログは英語 (AGENTS.md)。実装のログは次のとおり。URI は Debug 表記で出力し、移行ログは
  query を伏せる (`endpoint_label`)。
  - `MOQT: GOAWAY received (timeout=5000ms) new_session_uri="moqt://..."`
  - `[i0/vc-0] MOQT session migration: "moqt://relay-a/path-a" -> "moqt://relay-b/path-b" (timeout=5000ms, migrations=1)`

### 3.6 テスト

モック・スタブは禁止 (AGENTS.md) なので、relay を使わずに書ける純関数とパーサに絞る。

- `resolve_next_endpoint`: 空 URI → current、`moqt://` URI (IPv6 を含む) → パース結果、
  非 UTF-8 / 不正 scheme → エラー。
- `should_migrate`: GOAWAY の有無、移行設定、token キャンセルの組み合わせ。
- `--reconnect-on-goaway` の既定値 / `true` / `false` 解釈。JSONC の bool 変換。
- `EndpointKey`: path / query だけが違う接続先が同じキーになること、host / port が違えば
  別のキーになること、host の大文字小文字が正規化されること。
- `MoqEndpoint::parse_new_session_uri` のエラーが CLI 引数の `--url` に言及しないこと。
- GOAWAY 受信から再接続までの結合フロー (実 peer が必要) は単体テストできないため、
  実 relay での手動確認に回す。E2E (`.github/workflows/e2e-test.yml`) は relay を再起動
  できないため、drain を能動的に起こす手順は別途用意する。

## 4. デフォルト有効の判断

**推奨: デフォルト有効。**

賛成理由:

1. GOAWAY はアプリケーションから透過な移行を理想とする drain の合図である (§6.6.1)。
   移行そのものを義務づける MUST / SHOULD はないが、GOAWAY 受信後は新規 request を開始しない
   ことが SHOULD NOT とされ (§9.2)、moqt-rs は実際に `send_publish` / `send_subscribe` を
   拒否する。移行しないクライアントは drain 後に既存 publish しか継続できず、新規の
   PUBLISH / SUBSCRIBE を開始できない。既定無効はこの制約を既定の挙動にすることになる。
2. 負荷試験ツールとしての用途に合う。relay の再起動・ローリング更新は正常系であり、
   そのたびに全 VC が停止すると試験が成立しない。移行を既定で行えば「drain を跨いで負荷を
   維持できるか」を検証できる。
3. Sora 版の前例と整合する。シナリオの計画切断は即時再接続し、retry_count をリセットする
   (`zakuro/src/virtual_client.rs:337-351`)。
4. 既存の `--max-retry 0` は「想定外切断で停止」の意味であり、drain を想定外扱いするのは
   意味論として誤り。

既定有効による影響 (docs / README に明記する):

- `--duration` はセッション単位のタイマーなので、移行すると新セッションでリセットされる。
  VC の総寿命が `--duration` を超え得る。総寿命で止めたい場合は別途実装が必要。
- `retry_count` は移行でリセットするため、`--max-retry` の予算が移行で回復する。
- 統計は移行回数だけ Connected / Disconnected が増える。成功/失敗の判定は MOQ 版では
  行っていないため、合否への影響はない。
- 移行先が別 authority の場合、`--ca-cert` / `--insecure` の設定がそのまま適用される。
  接続失敗はログに出るが、既定 `--max-retry 0` では VC が停止する。
- 全 VC が同時に GOAWAY を受けると、新 relay へ一斉に再接続する (thundering herd)。
  必要なら移行 reconnect のレート制御や jitter を将来追加する。
- 購読中 (`--subscribe-tracks`) の VC でも旧セッションを即 NO_ERROR で閉じるため、
  §6.6.1 の RECOMMENDED (Established subscription がなくなるまで待つ) からは外れる。
  MUST ではないが、理想は新セッションをバックグラウンドで確立して移行してから旧セッションを
  閉じる (§7.4.1 / §7.6.1 の switchover)。初版は close-then-connect の割り切りとし、
  再接続のハンドシェイク時間程度の配信ギャップを許容する (長さは環境依存)。

反対寄りの意見と緩和:

- 「relay の drain 完了で VC が止まる挙動を測りたい」用途はある。`--reconnect-on-goaway=false`
  で従来挙動 (relay が 0x10 で閉じるまで publish を続け、その後停止) を選べるようにする。
- 明示した `--url` と違う host へ繋ぐのは驚きがある。移行時は old -> new を info ログに
  必ず出し、README に「New Session URI へ接続し直す」と明記する。

## 5. 決定事項と残課題

決定済み:

1. 既定有効とする (`--reconnect-on-goaway` の既定 true)。
2. New Session URI の parse 失敗時は現在 URI へフォールバックせず、仮想クライアントを停止する
   (malformed URI の扱いは仕様に規定が無いが、誤った接続先へ繋がない)。
3. request stream の GOAWAY は per-request migration を実装しない (既知の未対応)。
   セッション移行の対象は制御ストリームの GOAWAY のみ。
4. `--duration` はセッション単位のまま (移行でリセットされる)。
5. `MoqClientContext` は host / port をキーに接続先ごとにキャッシュする。

残課題:

1. 移行 reconnect の同時実行制御 (jitter / レート制限) を入れるか。
2. `--duration` を仮想クライアントの総寿命にするか。
3. request 単位の移行 (per-request migration) を実装するか。
4. 新セッションをバックグラウンドで確立してから旧セッションを閉じる dual-session 移行
   (draft §7.4.1 / §7.6.1 の理想)。初版は close-then-connect のため、再接続のハンドシェイク時間程度の
   配信ギャップがある (長さは環境依存)。
5. 実際の relay が返す New Session URI の形式 (同一 authority か別 host か) の確認と、
   実 relay での drain 手動確認。特に「SETUP と GOAWAY が同時に届く drain」「request stream
   の GOAWAY を送る relay」の 2 ケース。
6. 使われなくなった接続先のキャッシュ削除 (eviction)。多数の異なる relay へ移行し続けると
   UDP ソケットが残る。
