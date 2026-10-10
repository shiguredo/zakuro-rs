# zakuro-rs DuckDB スキーマドキュメント

zakuro-rs は WebRTC の統計情報を DuckDB データベースファイルに保存します。

## ファイル生成

- 1 プロセスにつき 1 つの DuckDB ファイルを生成します
- ファイル名は `zakuro_YYYYMMDD_HHMMSS_mmm.db` 形式 (UTC)
- `--duckdb-output-dir <DIR>` で出力ディレクトリを指定します (デフォルトはカレントディレクトリ)
- `--duckdb-interval <SEC>` で統計の書き込み間隔を指定します (デフォルトは 1.0 秒)
- `--no-duckdb-output` で DuckDB 出力を無効化できます

## 書き込み

- 起動情報、シナリオ、`connection`、`connection_lifecycle`、`rtc_stats_codec`、ICE 候補は制御コマンドとして送り、統計の書き込みが遅れても欠落しません
- `rtc_stats_codec` と ICE 候補は接続ごとに同じ内容が続くため、最初の 1 回だけ書きます
- それ以外の RTC 統計は、接続 1 本の `get_stats` 1 回を 1 サンプルとしてまとめ、DuckDB の Appender でテーブルごとにバルク INSERT します
- writer が追いつかない間に同じ接続のサンプルが複数溜まった場合は、最新の 1 回だけを残します。特定の接続の時系列だけが残ることはありません
- サンプル用チャネルの容量を超えた tick は、その接続のその 1 回ぶんを捨てます。ログは `[duckdb] dropped stats samples` です
- 30 秒ごとに `[duckdb] wrote samples=... rows=... dropped_total=...` を出します
- 収集間隔が設定の 2 倍を超えて空いた接続は `[duckdb] stats collection gap` を出します

## テーブル一覧

- `zakuro` - Zakuro 起動情報 (1 行のみ)
- `zakuro_scenario` - 各インスタンスのシナリオ設定
- `connection` - 接続情報 (各 Sora connection 1 行)
- `connection_lifecycle` - 接続 1 本のライフサイクル (接続終了時に 1 行)
- `rtc_stats_codec` - コーデック統計
- `rtc_stats_inbound_rtp` - 受信 RTP ストリーム統計
- `rtc_stats_outbound_rtp` - 送信 RTP ストリーム統計
- `rtc_stats_media_source` - メディアソース統計
- `rtc_stats_remote_inbound_rtp` - リモート受信 RTP 統計
- `rtc_stats_remote_outbound_rtp` - リモート送信 RTP 統計
- `rtc_stats_data_channel` - データチャネル統計
- `rtc_stats_transport` - トランスポート統計 (選択中の candidate pair、DTLS 状態、送受信バイト)
- `rtc_stats_candidate_pair` - ICE candidate pair 統計 (RTT、可用帯域、nominated)
- `rtc_stats_local_candidate` - ローカル ICE 候補 (接続ごとに 1 回)
- `rtc_stats_remote_candidate` - リモート ICE 候補 (接続ごとに 1 回)

## 共通列

各 `rtc_stats_*` テーブル (および `connection` テーブル) は以下の共通列を持ちます:

| 列名 | 型 | 説明 |
|------|-----|------|
| `pk` | BIGINT | 主キー (シーケンス自動生成) |
| `instance_id` | INTEGER | Zakuro インスタンス ID (0 から始まる) |
| `timestamp` | TIMESTAMP | Zakuro 側の記録時刻 (UTC) |
| `channel_id` | VARCHAR | Sora チャネル ID |
| `session_id` | VARCHAR | Sora セッション ID |
| `connection_id` | VARCHAR | Sora コネクション ID |
| `rtc_timestamp` | DOUBLE | WebRTC の `RTCStats.timestamp` 相当 (ミリ秒) |
| `type` | VARCHAR | RTCStats type |
| `id` | VARCHAR | RTCStats id |

`zakuro` テーブルは 1 行レコード前提のため `instance_id` 列を持ちません。
`zakuro_scenario` は `instance_id` を 1 列目に持ちます。
`connection_lifecycle` は共通列を持たず、専用の列だけを持ちます。

`zakuro_scenario` は Sora 版 (`zakuro`) だけが書き、`sora_*` 列には Sora への接続設定が入ります
(接続しないインスタンスでは `sora_signaling_urls` が空配列になります)。
MOQ 版 (`zakuro-moq`) は DuckDB 出力を持ちません。

## connection_lifecycle の列

| 列名 | 型 | 説明 |
|------|-----|------|
| `pk` | BIGINT | 主キー (シーケンス自動生成) |
| `instance_id` | INTEGER | Zakuro インスタンス ID |
| `vc_id` | INTEGER | 仮想クライアント ID |
| `channel_id` | VARCHAR | Sora チャネル ID |
| `role` | VARCHAR | `sendonly` / `recvonly` / `sendrecv` |
| `connection_id` | VARCHAR | Sora コネクション ID (未確立なら NULL) |
| `session_id` | VARCHAR | Sora セッション ID (未確立なら NULL) |
| `attempt_started_at` | TIMESTAMP | 接続の試行を開始した時刻 |
| `offer_received_at` | TIMESTAMP | offer を受信した時刻 (シグナリング成立) |
| `webrtc_connected_at` | TIMESTAMP | PeerConnection が Connected になった時刻 |
| `ice_connected_at` | TIMESTAMP | ICE が接続済みになった時刻 |
| `ice_gathering_complete_at` | TIMESTAMP | ICE の候補収集が完了した時刻 |
| `first_video_sent_at` | TIMESTAMP | 映像のパケットが流れたことを最初に観測した時刻 |
| `first_video_received_at` | TIMESTAMP | 映像のパケットが届いたことを最初に観測した時刻 |
| `first_audio_sent_at` | TIMESTAMP | 音声のパケットが流れたことを最初に観測した時刻 |
| `first_audio_received_at` | TIMESTAMP | 音声のパケットが届いたことを最初に観測した時刻 |
| `first_delivery_report_at` | TIMESTAMP | SFU からのレポート (remote-inbound-rtp) を最初に観測した時刻 |
| `samples` | INTEGER | 判定に使った統計サンプル数 |
| `last_media_activity_at` | TIMESTAMP | 最後にメディアの増加を観測した時刻 |
| `max_idle_samples` | INTEGER | 連続して増加が観測されなかったサンプル数の最大値 |
| `disconnected_at` | TIMESTAMP | 接続が終了した時刻 |
| `peer_connection_state` | VARCHAR | 最後に観測した PeerConnection の状態 |
| `ice_connection_state` | VARCHAR | 最後に観測した ICE 接続の状態 |
| `ice_gathering_state` | VARCHAR | 最後に観測した ICE 候補収集の状態 |
| `signaling_state` | VARCHAR | 最後に観測したシグナリングの状態 |
| `end_reason` | VARCHAR | 接続が終了した理由 |
| `outcome` | VARCHAR | 接続の判定結果 (`success` / `failure` / `unjudged`) |
| `failure_reason` | VARCHAR | 失敗理由 (成功と判定不能の場合は NULL) |
| `stalled` | BOOLEAN | メディアが止まった状態か |

## シーケンスとインデックス

各 stats テーブルの `pk` 列用に 13 個のシーケンスが作成されます。また、`connection_id` での検索と `(channel_id, connection_id, timestamp)` での複合検索用に 14 個のインデックスが作成されます。

## サンプルクエリ

### instance 別接続数

```sql
SELECT instance_id, channel_id, COUNT(*) FROM connection
GROUP BY instance_id, channel_id;
```

### role 別接続数

```sql
SELECT role, COUNT(*) FROM connection GROUP BY role;
```

### 試験全体期間

```sql
SELECT version, config_mode, start_timestamp, stop_timestamp FROM zakuro;
```

### instance 別シナリオ概要

```sql
SELECT instance_id, vcs, duration, sora_role FROM zakuro_scenario
ORDER BY instance_id;
```

### codec 別接続数

```sql
SELECT mime_type, COUNT(DISTINCT connection_id) FROM rtc_stats_codec
GROUP BY mime_type;
```

### 合否の集計

```sql
SELECT outcome, failure_reason, COUNT(*) FROM connection_lifecycle
GROUP BY outcome, failure_reason ORDER BY COUNT(*) DESC;
```

`failure_reason` は `build-failed` / `connect-failed` / `no-media-sent` /
`no-delivery-report` / `no-media-received` / `unexpected-disconnect` のいずれかです。

### 接続の確立状況

`connection_lifecycle` の 1 行が接続 1 本に対応します。

```sql
SELECT end_reason,
       COUNT(*) AS connections,
       COUNT(offer_received_at) AS signaling_ok,
       COUNT(webrtc_connected_at) AS webrtc_ok
FROM connection_lifecycle
GROUP BY end_reason;
```

### 接続確立時間の概要

試行開始から WebRTC の確立までの時間を秒で出します。

```sql
SELECT instance_id,
       COUNT(*) AS connections,
       ROUND(AVG(epoch(webrtc_connected_at - attempt_started_at)), 3) AS avg_seconds
FROM connection_lifecycle
WHERE webrtc_connected_at IS NOT NULL
GROUP BY instance_id;
```

## 注意事項

- すべてのタイムスタンプは UTC で記録されます
- `rtc_timestamp` は WebRTC の `performance.timeOrigin + performance.now()` の値 (ミリ秒) です
- `zakuro` テーブルの `stop_timestamp` はプロセス正常終了時に記録されます。panic / SIGKILL では NULL のまま残ります
- `connection` テーブルは Rust 版では 1 接続 1 行を記録します (offer 受信時に 1 度だけ INSERT)
- `connection_lifecycle` は接続 1 本につき 1 行を、接続が終了した時点で記録します。接続の構築に失敗した試行も 1 行として残り、`connection_id` は NULL になります
- `connection_lifecycle` の `end_reason` は `build-failed` / `shutdown` / `duration-expired` / `scenario-disconnect` / `scenario-exit` / `unexpected` のいずれかです
- `connection_lifecycle` の `offer_received_at` は Sora のシグナリングが成立した時刻、`webrtc_connected_at` は PeerConnection が Connected になった時刻です。前者だけがあれば ICE / DTLS で止まったことになります
- `connection_lifecycle` の `*_state` 列は最後に観測した状態です (`peer_connection_state` は `connected` / `failed` など)
- `connection_lifecycle` の `first_*_at` は RTCStats の累積カウンタから観測した時刻です。映像と音声を分けてあるため、「音声だけ流れている」といった失敗を切り分けられます
- `connection_lifecycle` の `samples` はメディアの統計を含むサンプル数です。`samples` が 0 の接続は送受信の有無を判定できていません (接続が短命だった場合に起こります)
- メディアの観測には統計の収集が要るため、`--no-duckdb-output` を指定しても接続確立後は `--duckdb-interval` ごとに `get_stats` を呼びます (ファイルは生成しません)
- `outcome` は「接続が確立し、有効な種別のメディアが流れていることを観測できたか」を表します。`sendonly` は送信と SFU からのレポート到着、`recvonly` は受信、`sendrecv` はその両方を満たす必要があります
- `outcome` が `unjudged` になるのは、統計サンプルが 1 件も無い場合、映像も音声も無効な場合、確立から猶予 (10 秒) 未満で終了した場合です。成功接続率を出すときは分母から外してください
- `stalled` は「動いていたメディアが 3 サンプル連続で増加しなかった」状態です。成功 / 失敗とは別の軸なので、`outcome = 'success'` かつ `stalled = true` の接続は「メディアは流れたが途中で止まった」ことを表します
- `websocket_connected` は常に `true`、`datachannel_connected` は常に `false` が記録されます (offer 受信時のスナップショット固定値)。動的追跡は未対応です
- `config_mode` は `ARGS` (CLI 引数) または `JSONC` (`--config` 指定) です
- `config_json` の機密フィールド (`client_cert` / `client_key` / `sora_metadata` / `sora_signaling_notify_metadata`) は `"<masked>"` で出力されます
- `rtc_stats_codec` の UNIQUE 制約で `channels` が NULL の場合は複数行が許容されます (SQL 標準で NULL は複数許容されるため)
- `rtc_stats_outbound_rtp` の `psnrSum` / `psnrMeasurements` は `record<DOMString, double>` 型のため未対応です
- `--no-duckdb-output` 指定時は DuckDB ファイルは生成されず、writer task も起動しません
- `--no-duckdb-output` と他の `--duckdb-*` 引数を併用した場合、`--no-duckdb-output` が優先されます
- 統計サンプルは接続 1 本の 1 tick を 1 メッセージにして Appender でまとめて書く。writer が遅れると、同じ接続の未書き込み分は最新の 1 tick だけが残る。チャネル容量 (8192 サンプル) を超えた tick は捨て、`[duckdb] dropped stats samples` に件数が出る。`connection`、`connection_lifecycle`、`rtc_stats_codec`、ICE 候補はこの欠落の対象にならない
- `rtc_stats_candidate_pair` の `nominated = true` がその時点の経路。`available_outgoing_bitrate` と `current_round_trip_time` で送信レートを見る
