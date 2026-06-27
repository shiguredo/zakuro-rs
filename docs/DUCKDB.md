# zakuro-rs DuckDB スキーマドキュメント

zakuro-rs は WebRTC の統計情報を DuckDB データベースファイルに保存します。

## ファイル生成

- 1 プロセスにつき 1 つの DuckDB ファイルを生成します
- ファイル名は `zakuro_YYYYMMDD_HHMMSS_mmm.db` 形式 (UTC)
- `--duckdb-output-dir <DIR>` で出力ディレクトリを指定します (デフォルトはカレントディレクトリ)
- `--duckdb-interval <SEC>` で統計の書き込み間隔を指定します (デフォルトは 1.0 秒)
- `--no-duckdb-output` で DuckDB 出力を無効化できます

## テーブル一覧

- `zakuro` - Zakuro 起動情報 (1 行のみ)
- `zakuro_scenario` - 各インスタンスのシナリオ設定
- `connection` - 接続情報 (各 Sora connection 1 行)
- `rtc_stats_codec` - コーデック統計
- `rtc_stats_inbound_rtp` - 受信 RTP ストリーム統計
- `rtc_stats_outbound_rtp` - 送信 RTP ストリーム統計
- `rtc_stats_media_source` - メディアソース統計
- `rtc_stats_remote_inbound_rtp` - リモート受信 RTP 統計
- `rtc_stats_remote_outbound_rtp` - リモート送信 RTP 統計
- `rtc_stats_data_channel` - データチャネル統計

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

## シーケンスとインデックス

各 stats テーブルの `pk` 列用に 8 個のシーケンスが作成されます。また、`connection_id` での検索と `(channel_id, connection_id, timestamp)` での複合検索用に 9 個のインデックスが作成されます。

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

## 注意事項

- すべてのタイムスタンプは UTC で記録されます
- `rtc_timestamp` は WebRTC の `performance.timeOrigin + performance.now()` の値 (ミリ秒) です
- `zakuro` テーブルの `stop_timestamp` はプロセス正常終了時に記録されます。panic / SIGKILL では NULL のまま残ります
- `connection` テーブルは Rust 版では 1 接続 1 行を記録します (offer 受信時に 1 度だけ INSERT)
- `websocket_connected` は常に `true`、`datachannel_connected` は常に `false` が記録されます (offer 受信時のスナップショット固定値)。動的追跡は未対応です
- `config_mode` は `ARGS` (CLI 引数) または `JSONC` (`--config` 指定) です
- `config_json` の機密フィールド (`client_cert` / `client_key` / `sora_metadata` / `sora_signaling_notify_metadata`) は `"<masked>"` で出力されます
- `rtc_stats_codec` の UNIQUE 制約で `channels` が NULL の場合は複数行が許容されます (SQL 標準で NULL は複数許容されるため)
- `rtc_stats_outbound_rtp` の `psnrSum` / `psnrMeasurements` は `record<DOMString, double>` 型のため未対応です
- `--no-duckdb-output` 指定時は DuckDB ファイルは生成されず、writer task も起動しません
- `--no-duckdb-output` と他の `--duckdb-*` 引数を併用した場合、`--no-duckdb-output` が優先されます
- 大規模試験 (`instances=64 × vcs=1000`) では mpsc バッファ満杯により統計のサンプリング欠落が発生しうます。`dropped_count > 0` の試験では `SELECT DISTINCT connection_id FROM rtc_stats_codec EXCEPT SELECT connection_id FROM connection` で orphan を検出可能です
