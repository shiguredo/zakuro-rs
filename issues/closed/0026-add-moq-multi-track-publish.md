# zakuro-moq で複数トラックを publish できるようにする

- Created: 2026-10-02
- Completed: 2026-10-02
- Branch: feature/add-moq-multi-track-publish
- Polished: {YYYY-MM-DD}

## 目的

MOQ の publisher は 1 セッション上で複数のトラックを publish できる
(draft-ietf-moq-transport-21 §6.3: "a client can initiate a MOQT session, subscribe, and
start publishing Objects all in parallel")。実際の配信は映像・音声・catalog のように
複数トラックを同時に流すため、負荷試験でも 1 仮想クライアントが複数トラックを
publish できる必要がある。

現状は 1 仮想クライアント = 1 トラックで、`--sora-moq-track-name` は 1 値しか取れない。

## 現状

- `src/moq_client.rs` の `run` が Track Name を `<接頭辞>-<instance>-<vc>` として 1 本作り、
  `src/moq_client/session.rs` の `SessionConfig` に 1 トラック分だけ渡している
- `Publisher` は 1 トラック分の状態 (Request ID / Track Alias / Group / Object) を持つ
- 設定は `--sora-moq-track-name` / `--sora-moq-object-rate` / `--sora-moq-object-size` の
  3 つで、レートとサイズは全トラック共通

## 設計方針

1. 設定をトラックの配列にする。JSONC は次の形にする

   ```jsonc
   {
     "url": "moqt://relay.example.com:4433",
     "namespace": "zakuro",
     "tracks": [
       { "name": "video", "object-rate": 30, "object-size": 1000 },
       { "name": "audio", "object-rate": 50, "object-size": 200 }
     ]
   }
   ```

2. CLI では `--tracks` に `name:rate:size` 形式のカンマ区切りを取る
   (例: `--tracks video:30:1000,audio:50:200`)。JSONC との併用時は CLI が優先される
3. Track Name は `<指定名>-<instance>-<vc>` として仮想クライアントごとに一意化する
   (relay は同一トラックへの複数 publisher を区別しないため)
4. Track Alias は 1 セッション内で一意な値をトラックごとに払い出す
5. object の送信はトラックごとに独立した周期で行う (1 本のタスクで複数トラックを回す)
6. 統計はトラックごとの送信数を記録する

## 完了条件

- 1 仮想クライアントが複数トラックを publish し、それぞれのトラックで指定レートの
  object が流れること
- JSONC の `tracks` 配列と CLI の `--tracks` の両方で指定できること
- 実際の sora-moq relay に対して複数トラックの publish が受理されること
  (relay のホストはリポジトリに書かない)
- `make ci` が通ること

## 変更対象

- `zakuro-moq/src/args.rs`
- `zakuro-moq/src/moq_client.rs`
- `zakuro-moq/src/moq_client/session.rs`
- `zakuro-moq/README.md` (または `README.md`)

## 解決方法

### 設定

- `zakuro-moq/src/args.rs` の `TrackSpec { name, object_rate, object_size }` を追加し、
  `--tracks` に `名前[:レート[:サイズ]]` のカンマ区切りを取る。JSONC は `tracks` 配列を取り、
  `--tracks` の形式へ変換して同じ検証経路に載せる
- レートとサイズの既定値は 30 objects/sec と 1000 バイト。レートは 0.001 - 1,000,000、
  サイズは 1 - 1,048,576 バイトに制限する (`Duration::from_secs_f64` の panic を防ぐ)
- 同名のトラックは起動エラーにする

### セッション

- `SessionConfig` を `tracks: &[TrackConfig]` に変え、1 セッションで全トラックを publish する
- Track Alias は 1 からトラックごとに払い出す。Track Name は `<指定名>-<instance>-<vc>`
- object の送信はトラックごとの周期で行う。全ての送信予定時刻の最小値まで `sleep_until` で待ち、
  送信時は期日を過ぎたトラックだけを進める (トラック数に依存しない 1 本のタイマーで駆動する)
- 遅れが 10 object 分を超えたら現在時刻から仕切り直す (Forward State 0 からの復帰時に
  溜まった分を一気に送らない)
- トラックごとに bidi request stream を 1 本開くため、その受信方向はストリームごとのタスクで
  読み、デコードしたメッセージをチャネルでメインタスクへ渡す (`Session` が `&mut self` を要求し、
  複数ストリームを select! で直接読めないため)
- PUBLISH の受理 (`RequestOkReceived`) は request_id で該当トラックへ反映し、受理されたトラックから
  順に送信を始める
- payload は最大 object サイズで 1 つだけ作り、トラックと仮想クライアントで共有する
- 送信した object 数は `AtomicU64` で集計し、5 秒ごとに
  `[stats] objects-sent=... recent-rate=.../s` としてログへ出す

### 確認

- `cargo test --locked --workspace` が通る (`zakuro-moq` 41 件)
- 実 relay に対して 2 仮想クライアント × 2 トラック (video 30 objects/sec + audio 50 objects/sec) を
  10 秒実行し、両トラックの PUBLISH が受理され、各仮想クライアントが 799 object (要求 800 object) を
  送信した。集計は 1532 object・約 160 objects/sec (要求 2 × 80 objects/sec) だった
- 疎通確認に使った relay のホストはリポジトリに書かない
