# zakuro-moq で subscribe (購読) の負荷試験をできるようにする

- Created: 2026-10-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-moq-subscribe-load-test
- Polished: {YYYY-MM-DD}

## 目的

`zakuro-moq` は publish 専用で、relay が購読者へ object を正しく配送できているかを
測定できない。MOQ relay の負荷試験では「配信側 (publish)」と「視聴側 (subscribe)」の
両方を測る必要があり、購読側が無いと次のことが確認できない。

- 購読したトラックに object が届くか (publish が受理されただけでは配送は保証されない)
- 届いた object のペイロード長・LOC の時刻情報が送信側の設定と一致するか
- 購読者数を増やしたときに relay が破綻しないか

現状は `shiguredo_moqt` の `Session::send_subscribe` を呼ぶ経路が無く、受信した
uni data stream も `accept_receive_stream` で捨てている。

## 設計方針

- `--subscribe-tracks` で購読するトラック名を指定する (`名前` のカンマ区切り)。
  Full Track Name は publish と同じ規則で `<名前>-<instance>-<vc>` とする
  (publish 側と subscribe 側で同じ `--vcs` を指定すると 1 対 1 で対応する)
- JSONC では `subscribe-tracks: ["video", "audio"]` の配列で指定する
- トラック名の `{instance}` / `{vc}` を仮想クライアントの値で置換する。プレースホルダが
  無い場合は `<名前>-<instance>-<vc>` にする。`video-{instance}-0` と書けば全購読者が
  同じトラックを購読でき、1 publish に対して N subscribe の負荷をかけられる
- 1 仮想クライアントが publish と subscribe の両方を行えるようにする
  (`--tracks` と `--subscribe-tracks` の同時指定を許す)
- `--tracks` の既定値 (`video`) は `--subscribe-tracks` 未指定のときだけ適用する
  (購読専用で起動したときに意図しない publish をしないため)
- 受信した object 数とバイト数を仮想クライアントごと・トラックごとに数え、
  5 秒ごとにログへ出す
- 受信した uni data stream は stream ごとのタスクで読み、デコードした
  SUBGROUP_HEADER / SUBGROUP_OBJECT をチャネルでメインタスクへ渡す
  (`Session` は `&mut self` を要求するため、複数 stream を select! で直接読めない)
- `--verify-payload` を指定したときだけ、受信 payload が zakuro-moq の publisher が
  送るパターン (`位置 % 251`) と一致するかを検査し、不一致数を数える
  (実メディアを配信する relay に接続したときに誤検知しないよう既定は無効)

## 完了条件

- `--subscribe-tracks` で購読し、publish した object が実際に届くこと
- 受信した object 数・バイト数がログに出ること
- publish 側と subscribe 側を別プロセスで同時に動かして疎通確認できること
  (relay のホストはリポジトリに書かず実行時に渡す)
- `make ci` が通ること

## 変更対象

- `zakuro-moq/src/args.rs`
- `zakuro-moq/src/main.rs`
- `zakuro-moq/src/moq_client.rs`
- `zakuro-moq/src/moq_client/session.rs`
- `README.md`

## 解決方法

### 設定

- `--subscribe-tracks` で購読するトラック名を指定する (カンマ区切り)。Full Track Name は
  publish と同じく `<名前>-<instance>-<vc>` とし、publish 側と同じ `--vcs` を指定すると
  1 対 1 で対応する
- JSONC では `subscribe-tracks: ["video", "audio"]` の配列で指定する
- `--tracks` を省略して `--subscribe-tracks` を指定した場合は publish しない
  (既定値 `video` は購読専用で起動したときには適用しない)。両方を指定すると 1 つの
  仮想クライアントが publish と subscribe を同時に行う
- `--verify-payload` で受信 payload が publisher のパターン (`位置 % 251`) と一致するかを
  検査できる (既定は無効)
- 存在しないトラックへの SUBSCRIBE は relay が REQUEST_ERROR (`no publisher for track`) で
  拒否するため、拒否メッセージに request 種別 (publish / subscribe) を含める

### セッション

- `SessionConfig` に `subscribe_tracks` を追加し、SETUP 完了後に SUBSCRIBE を送る
  (`Session::send_subscribe`)
- `SUBSCRIBE_OK` (`RequestOkReceived { request_kind: Subscribe }`) で受理を記録し、
  `Subscription::track_alias` を保存する
- 受信した uni data stream は stream ごとのタスクで読み、`recv_data_stream_type` →
  `recv_subgroup_header` → `recv_subgroup_object` をメインタスクから呼ぶ
  (`Session` は `&mut self` を要求するため複数 stream を select! で直接読めない)。
  `SubgroupStreamDecoder` で SUBGROUP_HEADER と SUBGROUP_OBJECT をデコードし、payload は
  `try_read_payload` で読み出す
- 受信した stream の Track Alias を覚えておき、object を購読へ帰属させる
- object 数・バイト数・payload 不一致数を購読ごとと全体で数え、5 秒ごとのログと終了時の
  ログ (`MOQT finished: publish=... subscribe=... received-objects=...`) に出す

### 確認

- `cargo test --locked --workspace` が通る (`zakuro-moq` 47 件)
- 実 relay に対して publish 側 (1 仮想クライアント・video 30 objects/sec) と subscribe 側
  (同じトラックを購読・`--verify-payload`) を別プロセスで同時に動かし、10 秒で 299 object
  (約 30 objects/sec)、299,000 バイト、payload 不一致 0 を確認した
- 2 仮想クライアント × 2 トラック (video 30 + audio 50 objects/sec) で、各仮想クライアントが
  807 object・403,800 バイトを受信し、payload 不一致 0 を確認した
- publish と subscribe を同じ仮想クライアントで同時に行い、160 object 送信に対して
  159 object を受信 (payload 不一致 0) することを確認した
- JSONC (`subscribe-tracks` 配列と `verify-payload`) でも購読でき、539 object・107,900 バイトを
  受信し、payload 不一致 0 を確認した
  (購読開始時に relay が現在の Group の先頭から配送するため、購読時間 × レートより
  少し多く届くことがある)
- 1 publish / 10 subscribe の負荷を確認した。配信側 1 仮想クライアント (video 30 objects/sec)、
  購読側 10 仮想クライアント (hatch rate 5) を別プロセスで同時に動かし、10 件すべての
  SUBSCRIBE が受理され、各購読者が 300 - 325 object (300,000 - 325,000 バイト) を受信、
  合計 3,124 object・3,124,000 バイトを 10 購読者で受信し、payload 不一致 0 を確認した
- 疎通確認に使った relay のホストはリポジトリに書かない
