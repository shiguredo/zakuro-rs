# Zakuro 実行情報・接続情報・WebRTC 統計情報を DuckDB ファイルに保存する機能を追加する

- Priority: Medium
- Created: 2026-03-27
- Completed: 2026-06-27
- Branch: feature/add-duckdb-stats-writer
- Polished: 2026-06-27

## 目的

Zakuro の起動情報・シナリオ設定・接続情報・WebRTC RTCStats を 1 プロセスにつき 1 つの DuckDB ファイル (`zakuro_YYYYMMDD_HHMMSS_mmm.db`) に定期的に保存し、負荷試験の結果を永続化して SQL / BI / Python ノートブックで分析可能にする。

`--duckdb-output-dir <DIR>` / `--duckdb-interval <SEC>` / `--no-duckdb-output` の 3 引数で制御する。書き込みは VirtualClient (= 1 Sora connection) ごとに `sora_sdk::SoraConnectionHandle::get_stats()` を `--duckdb-interval` 秒間隔で呼び、戻り JSON の `type` で振り分けて対応テーブルに INSERT する。

zakuro (C++) の `feature/add-duckdb` ブランチ (PR #76 / #78 の集合) で確立されたスキーマを基準とし、Rust 版固有の差分は本 issue 7 節で明示する。

本 issue では `connection` テーブルは offer 受信時の 1 行スナップショットのみを書き、動的状態 (再接続後の WebSocket 切断 / DataChannel open 等) の更新は `## 非対応` に分離する。

## 優先度根拠

Medium。

- C++ 版互換性ギャップ。`Cargo.toml` と `docs/ZAKURO.md` が `duckdb` を「将来の統計記録用」とコメントしたまま放置されており、機能未投入が明示されている
- 負荷試験ツールの結果分析・BI 連携・複数試験間の差分検証を Rust 版でも可能にする
- 後続 issue 0006 (RPC Query) の前提となる (issue 0006 の依存欄に本 issue が明記)
- 代替手段 (`rtc_log_info!` ログを grep で集計) でも一定の分析はできるため即時必須ではない

## 現状

`grep -rn 'duckdb\|DuckDB' src/` は 0 件で、`src/duckdb_stats.rs` は未作成。`Cargo.toml` に `duckdb = "1.10504"` が追加済みだが `features` 指定が無く、デフォルトではシステム DuckDB が必要で開発者 / CI 環境でビルドが通らない可能性がある。

closed/0021 (instance-hatch-rate) のマージにより以下が確定済み:

- `src/args.rs` に `CommonArgs` / `InstanceArgs` 分割、`is_common_key()` / `is_flag()` 実体、`split_cli_argv()` / `dedupe_argv_last_wins()` あり
- `src/main.rs` は `LocalSet::block_on(&rt, async_main())` 構成
- `src/virtual_client.rs:51-59` の `run(instance_id, vc_id, context, video_source, vc_config, token, stats_tx)` シグネチャが確定 (引数末尾に追加する形が既に確立)
- `src/error.rs` に `pub(crate) enum AppError` あり (`From<duckdb::Error>` バリアントを追加する対象)
- `src/args.rs:28-29` の `InstanceArgs.metadata` / `signaling_notify_metadata` は `Option<String>` 型 (`JsonString` への変換は `src/main.rs::run_zakuro_instance` 内で行う)

`sora_sdk` 2026.1.0-canary.11 で本機能に関係する公開 API (実コードを検証):

- `SoraConnectionHandle::get_stats() -> Result<JsonString>`
- `SoraConnectionHandle` は `Send + Sync` (内部 `mpsc::UnboundedSender` のみ保持)。既存 `src/virtual_client.rs:110` が `tokio::spawn` で `handle.clone()` を渡せている事実が証拠
- `sora_sdk::Role::as_sora_role(self) -> &'static str` (`"sendonly"` / `"recvonly"` / `"sendrecv"`)
- `sora_sdk::JsonString` には `impl DisplayJson` 済み (内部 `RawJsonOwned` への delegate)。`raw` フィールドへの public accessor は無く、再 parse が必要
- `SoraConnectionBuilder` のハンドラ:
  - `on_signaling_message(handler)` — handler は `Fn(SignalingType, SignalingDirection, &str) + Send + Sync + 'static`。第 1 引数が `SignalingType`、第 2 引数が `SignalingDirection`
  - `on_notify(handler)` — 現状 `src/virtual_client.rs:264` で空ハンドラ
- `SignalingDirection` / `SignalingType` 列挙子は `sora_sdk-2026.1.0-canary.11/src/types.rs` 配下
- `SoraConnectionHandle` に `connection_id` / `session_id` / `channel_id` getter は無い (本 issue 最大の論点。`src/data_channel.rs:249` の既存コメントが証拠)
- 公開 `version()` 関数は無い → `sora_sdk_version` は NULL bind
- `SoraConnectionHandle` には `IsConnectedWebsocket()` / `IsConnectedDataChannel()` 相当の同期 getter も無い (本 issue 5 節で offer 受信時固定値 bind、動的追跡は `## 非対応`)

`duckdb-rs` 1.10504 (実コードを検証):

- `Connection` は `Send` だが `Sync` ではない (`RefCell<InnerConnection>` 内包)
- `Connection::version() -> Result<String>`
- 複数行 SELECT は `Connection::prepare(sql)?.query([])?` または `Connection::query_row(sql, params, |row| ...)?`
- `Connection::execute_batch(sql)` で `BEGIN; CREATE ...; COMMIT;` を 1 発実行可能 (DDL 投入に採用)
- `features = ["bundled"]` 無しではビルド時にシステム DuckDB が必要

`shiguredo_webrtc` 0.150.2 / `shiguredo_openh264` 2026.1.0:

- `shiguredo_webrtc::version() -> &'static str` あり
- `shiguredo_openh264::Openh264Library::runtime_version(&self) -> String` あり (ロード後に呼べる)

Rust バージョン: `Cargo.toml` の `rust-version = "1.96"` が MSRV。`std::sync::OnceLock` は Rust 1.70 で安定、本リポジトリで利用可。

`tests/` ディレクトリは存在せず、リポジトリ実態は `src/<mod>.rs` の `#[cfg(test)] mod tests` に単体テストを置く構成 (同モジュールの private 関数も `super::` で呼べるため `pub(crate)` 格上げは不要)。

`docs/ZAKURO.md` の `## zakuro-rs 実装状況` には DuckDB 機能を示すチェックリスト項目が無い。

## 設計方針

### 1. コマンドライン引数

`src/args.rs` の `CommonArgs` に以下 3 引数を追加する。

| 引数 | 種別 | デフォルト | バリデーション | is_common_key | is_flag |
|---|---|---|---|---|---|
| `--duckdb-output-dir <DIR>` | 値付き | カレントディレクトリ (`.`) | `!help_mode && !no_duckdb_output` のとき `Path::new(&dir).is_dir()` で存在検証。`help_mode` か `--no-duckdb-output` 指定時はスキップ | yes | no |
| `--duckdb-interval <SEC>` | 値付き | `1.0` | `0.1 <= sec <= 86400` の inclusive 範囲。範囲外はパースエラー | yes | no |
| `--no-duckdb-output` | フラグ | 未指定 | 単独 | yes | yes |

`is_common_key()` への追加: `"duckdb-output-dir" | "duckdb-interval" | "no-duckdb-output"`。
`is_flag()` への追加: `"no-duckdb-output"`。

C++ 版 (`zakuro/src/main.cpp:141-148`) の `--duckdb-output-dir` / `--no-duckdb-output` に名前を揃える。`--duckdb-interval` は C++ 版に同等引数が無く Rust 版で新規導入 (デフォルト `1.0s` は C++ 版の内部固定値に合わせる)。

`--no-duckdb-output` と他の `--duckdb-*` 引数の併用: 引数の登場順を問わず `--no-duckdb-output` が指定されていれば `rtc_log_warning!("--no-duckdb-output specified, ignoring other --duckdb-* options")` を 1 度出して `--no-duckdb-output` を優先する。

### 2. モジュール構成と Error 型

`src/duckdb_stats.rs` を新規作成。

公開型 (`pub(crate)` 揃え):

```rust
pub(crate) struct DuckDBWriterConfig {
    pub(crate) db_path: PathBuf,  // main で生成済みの絶対パス
    pub(crate) interval: Duration,
    pub(crate) enabled: bool,
}

pub(crate) struct DuckDBStatsWriter {
    join_handle: Option<tokio::task::JoinHandle<()>>,  // disabled 時は None
    client: DuckDBClient,
}

#[derive(Clone)]
pub(crate) struct DuckDBClient {
    sender: Option<tokio::sync::mpsc::Sender<WriteCommand>>,
    dropped_count: Arc<AtomicU64>,
}

impl DuckDBClient {
    pub(crate) fn noop() -> Self { /* sender = None、is_enabled() == false */ }
    pub(crate) fn is_enabled(&self) -> bool { self.sender.is_some() }
    pub(crate) fn try_send(&self, cmd: WriteCommand) { /* 満杯 / 無効時は dropped_count++ */ }
    pub(crate) async fn send(&self, cmd: WriteCommand) { /* shutdown 経路用、無効時は何もしない */ }
}

pub(crate) struct ConnectionIds {
    pub(crate) connection_id: String,
    pub(crate) session_id: String,
}

pub(crate) enum WriteCommand {
    InsertZakuro(Box<InsertZakuroRow>),
    UpdateZakuroStop { stop_timestamp: SystemTime },
    InsertZakuroScenario(Box<InsertZakuroScenarioRow>),
    InsertConnection(Box<InsertConnectionRow>),
    InsertRtcStatsCodec(Box<RtcStatsCodecRow>),
    InsertRtcStatsInboundRtp(Box<RtcStatsInboundRtpRow>),
    InsertRtcStatsOutboundRtp(Box<RtcStatsOutboundRtpRow>),
    InsertRtcStatsMediaSource(Box<RtcStatsMediaSourceRow>),
    InsertRtcStatsRemoteInboundRtp(Box<RtcStatsRemoteInboundRtpRow>),
    InsertRtcStatsRemoteOutboundRtp(Box<RtcStatsRemoteOutboundRtpRow>),
    InsertRtcStatsDataChannel(Box<RtcStatsDataChannelRow>),
}
```

各 `*Row` 構造体は 7 節の DDL と 1:1 対応。WebRTC stats 列は `Option<i64>` / `Option<f64>` / `Option<String>` / `Option<bool>` でラップする。共通列 (`instance_id: u32` / `timestamp: SystemTime` / `channel_id: String` / `session_id: String` / `connection_id: String` / `type: String` / `id: String`) は非 Option (固定で埋まる)。`rtc_timestamp: Option<f64>` のみ Option (RTCStats 仕様で欠落しうる)。

`SystemTime` を DuckDB `TIMESTAMP` に bind する形式: `duration_since(UNIX_EPOCH).expect(...).as_micros() as i64` を `duckdb::types::Value::Timestamp(TimeUnit::Microsecond, micros)` に bind。

エラー型は `src/error.rs` の `AppError` に新バリアント `DuckDb(duckdb::Error)` を追加し、`impl From<duckdb::Error> for AppError` / `Display` 腕 / `source` 腕 を実装する。

### 3. 書き込み task の駆動方式

`Connection` は `Send + !Sync` のため複数 task から共有不可。

採用設計:

1. `main.rs::async_main` で `tokio::task::spawn_blocking` を 1 回呼び、その OS スレッド内で `tokio::runtime::Handle::current().block_on(async { tokio::select! { ... } })` を回す
2. VirtualClient (LocalSet 上の `spawn_local`) からは `DuckDBClient::try_send` で `Send` 可能な `WriteCommand` を投げる
3. mpsc チャネル容量: 典型運用 (`instances <= 4` × `vcs <= 100`) で 1 秒あたり ~2,800 commands を 2 秒分超バッファできる `8192` を採用。最大スケール (`instances = 64` × `vcs = 1000` × stats type 7 ≒ 448,000 commands/秒) では drop が発生しうるが「サンプリング欠落の許容」を運用ポリシーとして明示する
4. 満杯時は `try_send` で drop し `dropped_count` をインクリメント。writer task 内の reporter task (= writer 本体とは独立した `tokio::spawn` で起動した async task。3 節の writer 本体 select に並べると最大スケール時に reporter が starvation するため別 task) が 5 秒ごとに `rtc_log_warning!("[i*/vc-*][duckdb] dropped commands: total={}, since_last={}", total, delta)` を出力 (delta = 0 ならスキップ)

prepared statement キャッシュ: writer task 内で `HashMap<&'static str, duckdb::Statement<'_>>` を保持し、毎 INSERT で再準備しない。

### 4. SDK 識別子の取得方針 (C++ 版互換: `type:offer` 経由)

`sora_sdk::SoraConnectionHandle` に identifier getter は無い。

採用方針: `SoraConnectionBuilder::on_signaling_message(handler)` を購読し、`SignalingDirection::Received` かつ JSON `type == "offer"` のメッセージから `connection_id` / `session_id` を抽出する。C++ 版 `zakuro/src/virtual_client.cpp::OnSetOffer` と等価経路で、自分の signaling 経路から確実に取得できる (`on_notify` の `connection.created` は同一チャネル内の他 client 接続でも届きうるため不採用)。

`parse_offer_ids` 純粋関数を `pub(crate)` で切り出し、ハンドラ本体は薄いラッパーに留める (= 単体テスト容易):

```rust
/// type == "offer" メッセージから connection_id / session_id を抽出する
/// type != "offer"、JSON 不正、いずれかのキー欠落で None
pub(crate) fn parse_offer_ids(text: &str) -> Option<ConnectionIds> {
    let raw = nojson::RawJson::parse(text).ok()?;
    let v = raw.value();
    let ty: String = v.to_member("type").ok()?.required().ok()?.try_into().ok()?;
    if ty != "offer" { return None; }
    let connection_id: String = v.to_member("connection_id").ok()?.required().ok()?.try_into().ok()?;
    let session_id: String = v.to_member("session_id").ok()?.required().ok()?.try_into().ok()?;
    Some(ConnectionIds { connection_id, session_id })
}
```

スタイルは既存 `src/data_channel.rs::parse_data_channels` (`to_member().required().try_into()` シーケンス) に揃える。

C++ 版 `OnSetOffer` は `channel_id` / `audio` / `video` も同 offer から抽出するが、Rust 版は `channel_id` を `InstanceArgs.channel_id`、`audio` / `video` を `InstanceArgs` から取れるため `connection_id` / `session_id` のみ抽出する (offer 内重複情報より自プロセスの入力値を信頼する)。

ハンドラ内では抽出と同時に `WriteCommand::InsertConnection` も発行する (11 節):

```rust
let ids: Arc<std::sync::Mutex<Option<ConnectionIds>>> = Arc::new(std::sync::Mutex::new(None));
let ids_for_sig = ids.clone();
let duckdb_for_sig = duckdb_client.clone();
let channel_id_for_sig = config.channel_id.clone();
let role_str = config.role.as_sora_role().to_string();
let audio_value = !instance.no_audio_device && instance.audio;
let video_value = !instance.no_video_device;
builder = builder.on_signaling_message(move |_type_, direction, text| {
    if direction != SignalingDirection::Received { return; }
    let Some(parsed) = parse_offer_ids(text) else { return; };
    let row = InsertConnectionRow {
        instance_id, vc_id,
        timestamp: SystemTime::now(),
        channel_id: channel_id_for_sig.clone(),
        connection_id: parsed.connection_id.clone(),
        session_id: parsed.session_id.clone(),
        role: role_str.clone(),
        audio: audio_value,
        video: video_value,
        websocket_connected: true,    // 5 節参照
        datachannel_connected: false, // 5 節参照
    };
    // lock を取らずに try_send → その後 lock を取って set する (lock 保持中の try_send は呼ばない)
    duckdb_for_sig.try_send(WriteCommand::InsertConnection(Box::new(row)));
    *ids_for_sig.lock().expect("connection_ids mutex poisoned") = Some(parsed);
});
```

closure capture: `instance_id` / `vc_id` (Copy)、`channel_id_for_sig` (String clone、1 接続あたり 1 回しか走らないため `Arc<str>` 化は不要)、`role_str` (String clone)、`audio_value` / `video_value` (bool 事前算出)、`Arc<Mutex<Option<ConnectionIds>>>` clone、`DuckDBClient` clone。

`type == "re-offer"` は本 issue で無視 (`## 非対応` 参照)。`--repeat-interval` 再接続は VirtualClient 側で新規 `SoraConnectionBuilder` を構築するため、新規 `ids = Arc::new(...)` から作り直され自然に新 connection_id が記録される。

### 5. WebSocket / DataChannel 接続状態

`websocket_connected` / `datachannel_connected` は offer 受信時のスナップショットを固定値で書く:

- `websocket_connected = true` (offer は WebSocket 経由で届くため必ず true)
- `datachannel_connected = false` (offer 時点では DataChannel SCTP handshake 未完了)

注意事項として `docs/DUCKDB.md` に「現バージョンでは固定値 (`websocket_connected = true`、`datachannel_connected = false`) しか入らない。動的追跡は別 issue 化」と明記し、利用者が `WHERE datachannel_connected = true` で集計を組む罠を回避する (19 節)。

### 6. DuckDB ファイル生成

- ファイル名: `zakuro_{YYYYMMDD}_{HHMMSS}_{mmm}.db` (UTC、`mmm` はミリ秒 3 桁)
- C++ 版 `feature/add-duckdb:src/duckdb_stats_writer.cpp::GenerateFileName` 規約に揃える
- main 側で `output_dir.join(generate_filename())` を作って `DuckDBWriterConfig.db_path: PathBuf` で writer に渡す (= `start_timestamp` の `SystemTime::now()` とファイル名タイムスタンプを同じソースから生成)
- UTC 暦時刻整形に `jiff` クレートを使う (17 節依存追加)。`format!("zakuro_{}_{}_{:03}.db", date_str, time_str, millis)` で組み立てる
- 同名ファイル存在は起動エラー。`--no-duckdb-output` で回避可能
- writer task 起動後、`Connection::open(path)` → `conn.execute_batch("BEGIN; CREATE SEQUENCE ...; CREATE TABLE ...; CREATE INDEX ...; COMMIT;")` を 1 発実行 (途中失敗時は DuckDB が自動 rollback、明示 ROLLBACK 不要)。失敗時はファイル削除 + init readiness oneshot に `Err(AppError::DuckDb(...))` を送って main 側起動エラー (9 節 init flow)

### 7. テーブルスキーマ

C++ 版 `feature/add-duckdb:doc/DUCKDB.md` および `feature/add-duckdb:src/duckdb_stats_writer.cpp` を正本とし、以下の差分を加える:

- 全 stats テーブル (`connection` および `rtc_stats_*`) に `instance_id INTEGER` 列を `pk` 列の直後に挿入。`zakuro` は 1 行レコード前提のため `instance_id` 列を持たない。`zakuro_scenario` は元々 instance 単位 1 行で `instance_id` を 1 列目に持つ
- `config_mode` の値は `"ARGS"` / `"JSONC"` (C++ 版は `"ARGS"` / `"YAML"`)
- `zakuro` テーブルに `environment` 列を残す (`format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)`)
- C++ 版固有の `boost_version` / `cli11_version` / `cmake_version` / `blend2d_version` / `yaml_cpp_version` 列は省略

主要 4 テーブルの完全 DDL:

```sql
-- zakuro: 起動情報 (1 行のみ、instance_id 列なし)
CREATE TABLE zakuro (
    version VARCHAR,
    sora_sdk_version VARCHAR,
    webrtc_version VARCHAR,
    openh264_version VARCHAR,
    duckdb_version VARCHAR,
    environment VARCHAR,
    config_mode VARCHAR,
    config_json JSON,
    start_timestamp TIMESTAMP,
    stop_timestamp TIMESTAMP
);

-- zakuro_scenario: 各 instance のシナリオ設定
CREATE TABLE zakuro_scenario (
    instance_id INTEGER,
    vcs INTEGER,
    duration DOUBLE,
    repeat_interval DOUBLE,
    max_retry INTEGER,
    retry_interval DOUBLE,
    sora_signaling_urls VARCHAR[],
    sora_channel_id VARCHAR,
    sora_role VARCHAR
);

-- connection: 接続情報 (各 Sora connection 1 行、offer 受信時にのみ書く)
CREATE TABLE connection (
    pk BIGINT PRIMARY KEY DEFAULT nextval('connection_pk_seq'),
    instance_id INTEGER,
    vc_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    connection_id VARCHAR,
    session_id VARCHAR,
    role VARCHAR,
    audio BOOLEAN,
    video BOOLEAN,
    websocket_connected BOOLEAN,
    datachannel_connected BOOLEAN
);

-- rtc_stats_codec: codec 統計 (重複は ON CONFLICT で抑制)
CREATE TABLE rtc_stats_codec (
    pk BIGINT PRIMARY KEY DEFAULT nextval('rtc_stats_codec_pk_seq'),
    instance_id INTEGER,
    timestamp TIMESTAMP,
    channel_id VARCHAR,
    session_id VARCHAR,
    connection_id VARCHAR,
    rtc_timestamp DOUBLE,
    type VARCHAR,
    id VARCHAR,
    mime_type VARCHAR,
    payload_type BIGINT,
    clock_rate BIGINT,
    channels BIGINT,
    sdp_fmtp_line VARCHAR,
    UNIQUE(connection_id, id, mime_type, payload_type, clock_rate, channels, sdp_fmtp_line)
);
```

残り 6 テーブル (`rtc_stats_inbound_rtp` / `rtc_stats_outbound_rtp` / `rtc_stats_media_source` / `rtc_stats_remote_inbound_rtp` / `rtc_stats_remote_outbound_rtp` / `rtc_stats_data_channel`) は C++ 版 `feature/add-duckdb:doc/DUCKDB.md` を正本とし、`pk` 列の直後に `instance_id INTEGER` 列を挿入する機械的変換のみ適用する。実装時に DDL を取得し `docs/DUCKDB.md` (19 節) と `src/duckdb_stats.rs` の文字列定数の 2 箇所に転記する。

各 `rtc_stats_*` テーブルの共通列順序: `pk` / `instance_id` / `timestamp` / `channel_id` / `session_id` / `connection_id` / `rtc_timestamp` / `type` / `id`。

`rtc_stats_codec` UNIQUE 制約に `connection_id` が含まれる (接続ごとに同一 codec が 1 行ずつ) のは C++ 版仕様の踏襲。`channels` NULL ケースは SQL 標準で「NULL は複数許容」のため重複行が生じうる旨を `docs/DUCKDB.md` 注意事項に記す。

### 8. シーケンスとインデックス

シーケンス 8 個:

```sql
CREATE SEQUENCE connection_pk_seq;
CREATE SEQUENCE rtc_stats_codec_pk_seq;
CREATE SEQUENCE rtc_stats_inbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_outbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_media_source_pk_seq;
CREATE SEQUENCE rtc_stats_remote_inbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_remote_outbound_rtp_pk_seq;
CREATE SEQUENCE rtc_stats_data_channel_pk_seq;
```

インデックス 9 個 (C++ 版に揃える):

```sql
CREATE INDEX idx_connection_id ON connection(connection_id);
CREATE INDEX idx_connection_composite ON connection(channel_id, timestamp);
CREATE INDEX idx_rtc_stats_codec_composite ON rtc_stats_codec(channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_inbound_rtp_composite ON rtc_stats_inbound_rtp(channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_outbound_rtp_composite ON rtc_stats_outbound_rtp(channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_media_source_composite ON rtc_stats_media_source(channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_remote_inbound_rtp_composite ON rtc_stats_remote_inbound_rtp(channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_remote_outbound_rtp_composite ON rtc_stats_remote_outbound_rtp(channel_id, connection_id, timestamp);
CREATE INDEX idx_rtc_stats_data_channel_composite ON rtc_stats_data_channel(channel_id, connection_id, timestamp);
```

### 9. zakuro テーブルへの起動・終了情報の記録

#### writer 起動 + init readiness ハンドシェイク

`DuckDBStatsWriter::start(config) -> Result<(Self, String), AppError>` は内部で init readiness oneshot と DuckDB version 取得を兼ねる:

```rust
pub(crate) async fn start(config: DuckDBWriterConfig) -> Result<(Self, String), AppError> {
    if !config.enabled {
        return Ok((DuckDBStatsWriter { join_handle: None, client: DuckDBClient::noop() }, String::new()));
    }
    let (init_tx, init_rx) = tokio::sync::oneshot::channel::<Result<String, AppError>>();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<WriteCommand>(8192);
    let dropped_count = Arc::new(AtomicU64::new(0));
    let dropped_for_writer = dropped_count.clone();
    let join_handle = tokio::task::spawn_blocking(move || {
        // Connection::open + execute_batch でスキーマ投入 + version 取得 →
        //   成功時 init_tx.send(Ok(version)) で main へ通知
        //   失敗時 init_tx.send(Err(AppError::DuckDb(...))) で main へ通知
        //   Connection::version() 失敗時は "unknown" 文字列を Ok で送り writer ループへ進む
        // Handle::current().block_on(async { recv ループ }) で WriteCommand を処理
    });
    let duckdb_version = init_rx.await
        .map_err(|_| AppError::Message(ErrorMessage::new("duckdb writer aborted during init".into())))??;
    let client = DuckDBClient { sender: Some(cmd_tx), dropped_count };
    Ok((DuckDBStatsWriter { join_handle: Some(join_handle), client }, duckdb_version))
}
```

#### 起動時 INSERT (main 側)

```rust
let (writer, duckdb_version) = DuckDBStatsWriter::start(config).await?;
let row = InsertZakuroRow {
    version: env!("CARGO_PKG_VERSION").to_string(),
    sora_sdk_version: None,   // 公開 API 無し → NULL bind
    webrtc_version: Some(shiguredo_webrtc::version().to_string()),
    openh264_version: openh264_runtime_version,  // --openh264 指定時のみ Some
    duckdb_version: Some(duckdb_version),
    environment: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
    config_mode: if config_path.is_some() { "JSONC" } else { "ARGS" }.to_string(),
    config_json,
    start_timestamp: SystemTime::now(),
};
writer.client.try_send(WriteCommand::InsertZakuro(Box::new(row)));
```

`openh264_runtime_version` は `async_main` の OpenH264 ロード直後 (`openh264_lib` が `Some` の場合) に `Openh264Library::runtime_version()` を呼んで `Option<String>` で保持し InsertZakuro に渡す。

`config_path` は `parse_args()` で pre-parse した `Option<String>` を `parse_args()` の戻り値に含める (現状は `parse_args()` 内のローカル変数)。

`InsertZakuroRow` は version 系列を全て `Option<String>` で揃える (DDL は VARCHAR / nullable のため Option 統一が妥当)。

#### 終了時 UPDATE

shutdown ハンドシェイク (14 節) で main が `WriteCommand::UpdateZakuroStop { stop_timestamp: SystemTime::now() }` を `send().await` で送る (`try_send` だと満杯時に drop されるため、shutdown 経路は blocking で確実性を取る)。writer task は recv ループで処理してから drop する。

```sql
-- 1 行レコード前提のため WHERE 句なし
UPDATE zakuro SET stop_timestamp = ?;
```

panic / SIGKILL では `stop_timestamp` が NULL のまま残る (C++ 版同挙動として許容)。

### 10. zakuro_scenario テーブルへの記録

`async_main` で writer 起動 + `InsertZakuro` の後、各 `InstanceArgs` ごとに `WriteCommand::InsertZakuroScenario { instance_id: i, ... }` を 1 行 INSERT。

- `instance_id`: `0..instances_count`
- `sora_signaling_urls`: `InstanceArgs.signaling_urls` (`Vec<String>`、`src/args.rs:23`) を `duckdb::types::Value::List(urls.into_iter().map(duckdb::types::Value::Text).collect())` で bind
- `sora_role`: `InstanceArgs.role.as_sora_role().to_string()`

### 11. connection テーブルへの記録

`InsertConnection` の発行は 4 節の `on_signaling_message` ハンドラ内で `parse_offer_ids` が `Some(_)` を返した瞬間に 1 度だけ行う。再接続時は `SoraConnectionBuilder` 再構築でハンドラも作り直されるため、別 connection_id で再発行される。

各列の値は 4 節のコード例 (InsertConnectionRow) を参照。Rust 版は「1 接続 1 行」、C++ 版 `WriteStats` は「毎周期 1 行」で頻度が異なる旨を `docs/DUCKDB.md` 注意事項に記す。

### 12. config_json の構築 (DisplayJson 手書き、機密情報マスク)

`config_json` のトップレベル JSON 構造:

```json
{
  "common": { /* CommonArgs */ },
  "instances": [ { /* InstanceArgs[0] */ }, { /* InstanceArgs[1] */ }, ... ]
}
```

`shiguredo-rust` 規約に従い、`nojson::DisplayJson` を以下に手書き実装する:

- `args::CommonArgs` (7 フィールド、`src/args.rs:9-18`)
- `args::InstanceArgs` (37 フィールド、`src/args.rs:22-60`)
- マスク用ラッパー `MaybeMaskedJson<'a>`

`MaybeMaskedJson` の型定義 (`Option<String>` フィールドに対応するため `&'a str` 参照):

```rust
enum MaybeMaskedJson<'a> {
    Visible(&'a str),
    Masked,
}

impl<'a> nojson::DisplayJson for MaybeMaskedJson<'a> {
    fn fmt(&self, f: &mut nojson::JsonFormatter<'_, '_>) -> std::fmt::Result {
        match self {
            MaybeMaskedJson::Visible(s) => s.fmt(f),  // &str の DisplayJson 実装 (= JSON 文字列としてエスケープ出力)
            MaybeMaskedJson::Masked => f.write_str(r#""<masked>""#),
        }
    }
}
```

実装ルール:

- `Option<T>::None` のフィールドは JSON 出力から省略する (キー自体を出さない)
- `Option<String>` の機密フィールド (`metadata` / `signaling_notify_metadata` / `client_cert` / `client_key`): `Some(_) → MaybeMaskedJson::Masked` でキーと共に `"<masked>"` を出力、`None` ならキー省略
- `metadata` / `signaling_notify_metadata` は `InstanceArgs` 段階では `Option<String>` (生 JSON 文字列、`src/args.rs:28-29`)。`JsonString` 化は `run_zakuro_instance` 内で行うため `config_json` 構築時点では `String` のまま扱える
- `sora_sdk::Role` → `role.as_sora_role()` 文字列値
- `(i32, i32)` 解像度 (`InstanceArgs.resolution`) → `{"width": w, "height": h}`

機密情報マスク方針 (`shiguredo-no-secrets` 規約):

- `client_cert` / `client_key` / `metadata` / `signaling_notify_metadata`: 全マスク
- それ以外のパス系 (`openh264` / `input_y4m` / `input_mp4` / `input_wav` / `video_input_device`): マスクしない。DuckDB ファイル (`zakuro_*.db`) はランタイム生成物でリポジトリ成果物ではないため `shiguredo-no-secrets` の対象外。試験再現に必要で機密性は低い

DisplayJson 手書きは `CommonArgs` 7 + `InstanceArgs` 37 = 計 44 フィールドに渡る。本 issue では `config_json` 用途で含める。汎用 JSON 出力が必要になった段階で別 issue で切り出す。

`config_json` サイズ: `InstanceArgs` 37 フィールド × `instances <= 64` で 1 行 INSERT で 1 MB 以下に収まる見込み。

### 13. rtc_stats_* テーブルへの記録

各 VirtualClient で接続中は以下のループを回す:

```rust
use tokio_stream::{StreamExt, wrappers::IntervalStream};
use tokio::time::MissedTickBehavior;

let mut interval = tokio::time::interval(config.duckdb_interval);
interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
let mut ticks = IntervalStream::new(interval);
let mut skipped_iters: u32 = 0;
loop {
    tokio::select! {
        biased;
        _ = token.cancelled() => break,
        _ = ticks.next() => {
            let Some(ids) = connection_ids.lock().expect("connection_ids mutex poisoned").clone() else {
                skipped_iters += 1;
                continue;
            };
            if skipped_iters > 0 {
                rtc_log_info!("[i{}/vc-{}][duckdb] connection identifiers confirmed after {} skipped iterations", instance_id, vc_id, skipped_iters);
                skipped_iters = 0;
            }
            let stats = match handle.get_stats().await {
                Ok(s) => s,
                Err(e) => {
                    rtc_log_warning!("[i{}/vc-{}][duckdb] get_stats failed: {}", instance_id, vc_id, e);
                    continue;
                }
            };
            // JsonString から RawJsonOwned への抽出は再 parse 経由 (sora_sdk に as_raw() 等が無いため):
            //   let s = stats.to_string();
            //   let json = nojson::RawJsonOwned::parse(&s)?;
            // 各 entry の "type" で振り分けて WriteCommand を try_send (既存 src/data_channel.rs::parse_data_channels の to_member シーケンスを参考にする)
        }
    }
}
```

`MissedTickBehavior::Skip` 採用理由: デフォルト (`Burst`) では書き込みが詰まった後にバーストして負荷試験本体に影響を与えるため (既存 `src/stats.rs:142` の reporter と同方針)。

未知 type のログ抑制: `static UNKNOWN_TYPES: OnceLock<Mutex<HashSet<String>>>` で初回のみ warn。WebRTC stats には `transport` / `candidate-pair` / `local-candidate` / `remote-candidate` / `certificate` / `peer-connection` 等の未対応 type が頻出するため、毎秒 warn を出すとログが洪水化する。テストはモック禁止 (CLAUDE.md) のため warn 回数の直接検証は不能。代わりに `UNKNOWN_TYPES` の `HashSet` サイズ確認で抑制動作を検証する (18 節 C)。

`rtc_stats_codec` の INSERT は SQL に `ON CONFLICT (connection_id, id, mime_type, payload_type, clock_rate, channels, sdp_fmtp_line) DO NOTHING` を付与する。

`DuckDBClient::is_enabled() == false` (`--no-duckdb-output`) のときは本ループを起動しない (14 節)。

### 14. VirtualClient との連携と shutdown ハンドシェイク

#### VirtualClient シグネチャ拡張

`virtual_client::run` の末尾に `duckdb_client: DuckDBClient` を追加:

```rust
pub(crate) async fn run(
    instance_id: u32,
    vc_id: u32,
    context: Arc<SoraConnectionContext>,
    video_source: Option<VideoTrackSource>,
    vc_config: VirtualClientConfig,
    token: CancellationToken,
    stats_tx: mpsc::Sender<StatsEvent>,
    duckdb_client: DuckDBClient,
)
```

`run_zakuro_instance` のシグネチャにも `duckdb_client: DuckDBClient` を追加する。

`virtual_client::run` 内では `build_client` (= `SoraConnectionBuilder` 構築点、`src/virtual_client.rs:253` 周辺) に渡す前に 4 節の `Arc<std::sync::Mutex<Option<ConnectionIds>>>` を生成し、`on_signaling_message` ハンドラに clone を渡す (= `build_client` のシグネチャに `ids: Arc<Mutex<Option<ConnectionIds>>>` / `duckdb_client: DuckDBClient` 等を追加する形を採る)。

13 節の `get_stats` ループは既存 `src/virtual_client.rs:104-119` の `data_channel::run_messaging` の spawn 位置 (`stats_tx.send(StatsEvent::Connected)` の後、`Box::pin(client.run())` の前) に並べて起動する。

```rust
if duckdb_client.is_enabled() {
    let stats_client = duckdb_client.clone();
    let stats_ids = ids.clone();
    let stats_handle = handle.clone();
    let stats_token = connection_token.child_token();
    let interval = vc_config.duckdb_interval;
    tokio::task::spawn_local(async move {
        // 13 節のループ
    });
}
```

`tokio::task::spawn_local` を選ぶ理由: `SoraConnectionHandle` / `Arc<Mutex<...>>` / `DuckDBClient` は全て `Send` なので `tokio::spawn` でも動くが、`virtual_client::run` 全体が LocalSet 上の `spawn_local` で動いており、`connection_token` 寿命を `client.run()` と同 LocalSet 内で揃えるため `spawn_local` を採用する。

`--no-duckdb-output` 時 (`is_enabled() == false`) はループを spawn しない (`get_stats` の libwebrtc 内部コストを払わない)。

#### shutdown 順序

`async_main` 末尾 (既存 `src/main.rs:232-258` への追加):

1. main loop 抜け (既存)
2. `drop(stats_tx)` (既存)
3. `instances.join_next().await` (既存)
4. `token.cancel()` (既存、idempotent)
5. main 側で `duckdb_client.send(WriteCommand::UpdateZakuroStop { stop_timestamp: SystemTime::now() }).await` (disabled 時は no-op)
6. `drop(duckdb_client_main)` (main 側の最後の clone を drop、Sender が全 drop され writer の `recv().await` が `None` を返す)
7. `if let Some(h) = writer.join_handle { h.await? }` (writer task の完了待ち、`stop_timestamp` UPDATE 完了を保証)

writer task 側:

1. `tokio::runtime::Handle::current().block_on(async { tokio::select! { ... } })` で内側ループを回す
2. `tokio::select!` で `mpsc::Receiver::recv()` (本筋) のみを待ち、`Some(cmd)` なら処理、`None` で break (drop reporter は別 task に分離して reporter starvation を回避、3 節参照)
3. break 後に `Connection` を明示 `drop(conn)` してファイルを close (DuckDB は drop 時に自動 flush するため `CHECKPOINT` 明示不要)

### 15. エラー処理

- 起動時エラー (ファイル生成 / `CREATE TABLE` 失敗 / `--duckdb-output-dir` 不在): プロセスを起動エラーで終了 (9 節 init readiness oneshot 経由)。`--no-duckdb-output` で回避可能
- 実行時エラー (prepared statement / INSERT エラー / Connection ロスト): warning ログを出して該当 command を drop、プロセス継続
- mpsc 満杯 drop (`try_send`): reporter task で 5 秒ごとに累積件数を warn 出力
- mpsc Closed (writer task が panic 等で死亡): VirtualClient 側の `try_send` は静かに失敗、別カウンタは設けず `dropped_count` に同居
- `--no-duckdb-output` 指定時: DuckDB モジュール自体を起動しない
- panic / SIGKILL: `stop_timestamp` が NULL のまま残る (許容)

### 16. ログメッセージ

ログプレフィックスは既存 `src/stats.rs` の `[stats]` 規則 (= 末尾サフィックス) に揃え、DuckDB 系統には `[duckdb]` を末尾に付ける。vc 識別子付きは `[i{}/vc-{}][duckdb]` 形式、グローバル writer ログは `[duckdb]` 単独形式。

| 種別 | メッセージ | レベル |
|---|---|---|
| writer 起動 | `[duckdb] writer started: path={:?}, interval={:.2}s` | info |
| writer 停止 | `[duckdb] writer stopped` | info |
| writer 書き込み失敗 | `[duckdb] write failed: table={}, error={}` | warning |
| バックプレッシャ | `[duckdb] dropped commands: total={}, since_last={}` | warning |
| identifier 確定通知 | `[i{}/vc-{}][duckdb] connection identifiers confirmed after {} skipped iterations` | info |
| 未知 type 初回 | `[duckdb] unknown rtc stats type seen first time: {}` | warning |
| get_stats 失敗 | `[i{}/vc-{}][duckdb] get_stats failed: {}` | warning |
| no-duckdb-output 警告 | `--no-duckdb-output specified, ignoring other --duckdb-* options` | warning |

すべて `rtc_log_info!` / `rtc_log_warning!` 経由。Display (`{}`)、パスは Debug (`{:?}`)、浮動小数点は `{:.2}`。

### 17. Cargo.toml 依存追加

```toml
[dependencies]
# DuckDB バインディング (統計記録に利用)
duckdb = { version = "1.10504", features = ["bundled"] }
# UTC タイムスタンプ整形 (DuckDB ファイル名生成用)
jiff = "0.2"

[dev-dependencies]
# テストで一時ディレクトリを使う
tempfile = "3.20"
```

既存 `Cargo.toml:21` のコメント `# DuckDB バインディング (将来の統計記録用)` を `# DuckDB バインディング (統計記録に利用)` に同期 (19 節の `docs/ZAKURO.md` 更新と文言一致)。

`bundled` 採用理由: CI / 開発者環境で外部 DuckDB のインストールを不要にする。

PBT (`proptest`) は本 issue では採用しない。範囲外検証は単体テストで十分カバーできる。

### 18. テスト戦略

`shiguredo-rust` 規約 (モック / スタブ禁止)。配置はリポジトリ実態 (`src/<mod>.rs` の `#[cfg(test)] mod tests`)。

#### A. parse 系 (`src/args.rs::tests` への追加)

- `--duckdb-output-dir` 値付きパース成功 / 不在ディレクトリでエラー (`tempfile::TempDir::new()` で「存在する一時ディレクトリ」と「`tempdir.path().join("missing")` 不在」のペア)
- `--duckdb-interval` 境界値 (`0.1` / `86400` inclusive ok、`0.099` / `86400.1` でエラー)
- `--no-duckdb-output` 単独フラグパース成功
- `--no-duckdb-output` + `--duckdb-output-dir` 両順序で warning + `--no-duckdb-output` 優先 + ディレクトリ検証スキップ
- `is_common_key()` / `is_flag()` 追加項目の網羅テスト
- `split_cli_argv` / `dedupe_argv_last_wins` に `--duckdb-output-dir` (値付き) と `--no-duckdb-output` (フラグ) を含むケース

#### B. スキーマ生成 (`src/duckdb_stats.rs::tests`)

- `tempfile::TempDir::new()` で実 `.db` ファイル作成 (テスト並列対応)
- `Connection::prepare("SELECT COUNT(*) FROM duckdb_tables() WHERE schema_name='main'")?.query_row([], |row| row.get::<_, i64>(0))? == 10` で 10 テーブル
- 同形式で `duckdb_indexes()` / `duckdb_sequences()` (9 / 8)
- `PRAGMA table_info('connection')` 等で 2 列目が `instance_id`
- 各 `rtc_stats_*` テーブルの全列名・型一覧を expected と比較 (6 ケース、手動転記の silent miss 防止)
- `rtc_stats_codec` UNIQUE + ON CONFLICT: 同一値で 2 回 INSERT して 1 行のみ

#### C. RTCStats JSON 振り分け (`src/duckdb_stats.rs::tests`)

- 既知 type のサンプル JSON (テスト内 `&'static str` 定数):
  - `codec` / `inbound-rtp` / `outbound-rtp` / `media-source` / `remote-inbound-rtp` / `remote-outbound-rtp` / `data-channel` の各 1 件 + `transport` / `candidate-pair` (未対応) 各 1 件
- 投入後 `SELECT COUNT(*)` で各対応テーブルに 1 行、未対応 type は 0 行
- 未知 type の warn 抑制: テストごとに `format!("unknown-type-{}-{}", module_path!(), line!())` で一意化した type 名を使い、同一未知 type を 100 回投入して `UNKNOWN_TYPES` の `HashSet` サイズが 1 になることを確認 (`pub(crate) fn unknown_types_size_for_test() -> usize` を `#[cfg(test)]` で公開)

#### D. zakuro テーブル shutdown フロー (`src/duckdb_stats.rs::tests`)

- `DuckDBStatsWriter::start` → `InsertZakuro` 送信 → `InsertZakuroScenario` × 2 → `UpdateZakuroStop` → drop(client) → join_handle.await → `prepare("SELECT start_timestamp, stop_timestamp FROM zakuro")?.query_row(...)` で両方 NOT NULL
- `DuckDBStatsWriter` は `client` フィールドを `pub(crate)` で公開するか `into_parts(self) -> (Option<JoinHandle<()>>, DuckDBClient)` でテスト用 API を出す (実装時にどちらか選択)

#### E. config_json マスク (`src/args.rs::tests` または `src/duckdb_stats.rs::tests`)

- `client_cert` / `client_key` / `metadata` / `signaling_notify_metadata` に値を設定して `DisplayJson` 出力が `"<masked>"` を含み実値を含まない
- `None` 時にキー自体が JSON 出力に含まれない
- `Role::as_sora_role()` の文字列が含まれる

#### F. parse_offer_ids (`src/duckdb_stats.rs::tests`)

- 正常 offer JSON (`{"type":"offer","connection_id":"c1","session_id":"s1","sdp":"v=0\\n..."}`) から抽出成功 (sdp に escape を含む現実形のサンプル含む)
- `type != "offer"` (`update` / `re-offer` / `notify`) で `None`
- `connection_id` 欠落 / `session_id` 欠落 で `None`
- `connection_id` / `session_id` が文字列以外 (整数 / null) で `None`
- JSON 不正 で `None`

### 19. ドキュメント

- `docs/DUCKDB.md` を新規作成 (Markdown)
  - 目次:
    1. 概要 (生成タイミング / ファイル名規約 / 1 プロセス 1 ファイル)
    2. テーブル一覧 (10 テーブル)
    3. 各テーブルのスキーマ (CREATE TABLE 文 + 列説明、C++ 版から転記した 6 テーブルもここに完全 DDL を記載)
    4. シーケンスとインデックス
    5. 共通列の意味
    6. 注意事項
       - タイムスタンプは UTC
       - panic 時の `stop_timestamp` は NULL
       - `rtc_stats_outbound_rtp.psnrSum` / `psnrMeasurements` 未対応
       - 機密情報マスク方針
       - `rtc_timestamp` は WebRTC の `RTCStats.timestamp` 相当
       - `config_mode = 'JSONC'` は Rust 版固有。C++ 版資産との互換は `IN ('ARGS','YAML','JSONC')` で吸収
       - `connection` テーブルは Rust 版では 1 接続 1 行 (C++ 版は毎周期 1 行)
       - `websocket_connected` / `datachannel_connected` は **現バージョンでは固定値 (`true` / `false`)** しか入らない。動的追跡は別 issue 化
       - `rtc_stats_codec` の UNIQUE 制約で `channels` NULL ケースは複数行が許容される
       - `dropped_count > 0` の試験では `connection` 行が欠落しうるため `SELECT DISTINCT connection_id FROM rtc_stats_codec EXCEPT SELECT connection_id FROM connection` で orphan を検出可能
  - サンプルクエリ (5 件、本 issue 内に転記された列のみを参照する):
    - instance 別接続数: `SELECT instance_id, channel_id, COUNT(*) FROM connection GROUP BY instance_id, channel_id`
    - role 別接続数: `SELECT role, COUNT(*) FROM connection GROUP BY role`
    - 試験全体期間: `SELECT version, config_mode, start_timestamp, stop_timestamp FROM zakuro`
    - instance 別シナリオ概要: `SELECT instance_id, vcs, duration, sora_role FROM zakuro_scenario ORDER BY instance_id`
    - codec 別接続数: `SELECT mime_type, COUNT(DISTINCT connection_id) FROM rtc_stats_codec GROUP BY mime_type`
- `README.md` 「主な機能」に `DuckDB ファイルへの統計情報出力 (--duckdb-output-dir / --duckdb-interval / --no-duckdb-output)` を追加
- `docs/ZAKURO.md` の `## zakuro-rs 実装状況` 配下に新セクション `### 統計情報出力` を追加し `[x] DuckDB ファイル出力 (--duckdb-output-dir / --duckdb-interval / --no-duckdb-output)` を置く。同ファイル内 `duckdb` 依存表のコメントも `(統計記録に利用)` に同期
- `CHANGES.md` の `## develop` に `[ADD] DuckDB ファイルへの統計情報出力 (`--duckdb-output-dir` / `--duckdb-interval` / `--no-duckdb-output`) に対応する` を追加 (CLI オプション名はバッククォートで囲む既存スタイル)、`  - @voluntas` 行を続ける (インデント半角スペース 2)

## 完了条件

- `--duckdb-output-dir` 指定 / 未指定 (= カレント) で `zakuro_YYYYMMDD_HHMMSS_mmm.db` が生成される
- `--no-duckdb-output` でファイル生成 / writer task 起動が走らない
- `zakuro` テーブルに起動時 INSERT + 終了時 UPDATE (`stop_timestamp` NOT NULL は 18 節 D で自動検証)
- 各 `InstanceArgs` ごとに `zakuro_scenario` に 1 行 INSERT
- VirtualClient が `type:offer` を受信するたびに `connection` に 1 行 INSERT (`connection_id` / `session_id` / `role` が NOT NULL になることは推奨確認手順として手動 E2E で実 Sora 接続によって確認)
- 接続中は `--duckdb-interval` 秒ごとに `rtc_stats_*` 各テーブルに INSERT (推奨確認手順として手動 E2E)
- 12 節の DisplayJson 手書きが 44 フィールド網羅され、機密 4 フィールドが `"<masked>"` で出力される (18 節 E で検証)
- `--duckdb-interval` の境界値挙動が 18 節 A で自動検証
- ディレクトリ不在エラーと `--no-duckdb-output` 優先が 18 節 A で自動検証
- バックプレッシャ drop / 未知 type 抑制 / `parse_offer_ids` 単体動作が 18 節 C / F で自動検証
- 19 節のドキュメント更新が反映されている

## 非対応 (後続 issue 候補)

- HTTP / RPC API 経由での DuckDB クエリ機能 (issue 0006 の領域)
- DuckDB ファイルのローテーション / サイズ上限 / 試験 ID / タグ列
- 切断時の `connection` 更新カラム (`closed_at` / `disconnect_reason` 等)
- WebSocket / DataChannel 動的追跡 (`on_websocket_close` / `on_data_channel_open` / `on_data_channel_close` 購読) と `connection` テーブル UPDATE 経路の追加
- `rtc_stats_outbound_rtp.psnrSum` / `psnrMeasurements` (record<DOMString, double>)
- 既存 `stats.rs` (接続カウンタ集計) との統合 / 重複整理
- `sora_sdk` への `connection_id()` / `session_id()` getter 追加 (上流 PR)
- `sora_sdk::JsonString` への `as_raw()` / `into_raw()` getter 追加 (上流 PR、再 parse オーバーヘッド解消用)
- DuckDB バインドの bundled ビルド時間短縮 / バイナリサイズ削減 (`features = ["loadable-extension"]` 等)
- `--duckdb-output-dir` 不在時の自動作成
- 1 プロセスで複数 DuckDB ファイル分割
- `re-offer` 受信時の `connection_id` / `session_id` 更新追従
- `rtc_stats_codec` の UNIQUE 制約見直し (プロセス全体で codec 1 行)
- 大規模試験 (`instances=64 × vcs=1000`) での mpsc バッファ拡張 / Appender API による高速 bulk insert
- `nojson::DisplayJson` 実装を `args` 配下の汎用 JSON 出力機能 (CLI `--show-config` 等) として切り出す
- `run_zakuro_instance` の引数集約 (`InstanceTaskCtx` 構造体への束ね)

## 解決方法

### 新規ファイル

- `src/duckdb_stats.rs` — DuckDB 統計書き込みモジュール (Writer / Client / WriteCommand / Row 構造体 / DDL 投入 / INSERT 実行 / RTCStats JSON 振り分け / offer からの connection_id 抽出 / config_json 構築 + テスト)
- `src/duckdb_schema.sql` — DuckDB スキーマ (シーケンス 8 個 + テーブル 10 個 + インデックス 9 個)
- `docs/DUCKDB.md` — DuckDB スキーマドキュメント

### 変更ファイル

- `Cargo.toml` — `duckdb` に `features = ["bundled"]` を追加、`jiff = "0.2"` を追加、`tempfile = "3.10"` を dev-dependencies に追加
- `src/error.rs` — `AppError` に `DuckDb(duckdb::Error)` バリアントを追加
- `src/args.rs` — `CommonArgs` に `duckdb_output_dir` / `duckdb_interval` / `no_duckdb_output` の 3 フィールドを追加、`is_common_key()` / `is_flag()` を更新、`parse_args()` の戻り値に `config_path` を追加、DuckDB 引数のテストを追加
- `src/main.rs` — `DuckDBStatsWriter` の起動 / `InsertZakuro` / `InsertZakuroScenario` / shutdown ハンドシェイク / `run_zakuro_instance` への `duckdb_client` 受け渡し
- `src/virtual_client.rs` — `VirtualClientConfig` に `duckdb_client` / `duckdb_interval` を追加、`on_signaling_message` ハンドラで offer 受信時に `connection` テーブルへ INSERT、`run_stats_collection` で `get_stats` ループを起動
- `CHANGES.md` — `[ADD]` エントリを追加
- `README.md` — 主な機能とオプション表に DuckDB 系を追加
- `docs/ZAKURO.md` — 依存ライブラリ表に `jiff` を追加、実装状況に DuckDB 出力を追加

### 設計の要点

- `Connection` は `Send` だが `!Sync` のため、`spawn_blocking` 内で `Handle::current().block_on` して mpsc 受信ループを回す
- VirtualClient からは `DuckDBClient::try_send` で `Send` 可能な `WriteCommand` を投げる
- mpsc バッファ容量 8192、満杯時は `try_send` で drop し `dropped_count` をインクリメント、reporter task が 5 秒ごとに warn 出力
- `type:offer` メッセージから `connection_id` / `session_id` を抽出し `connection` テーブルに 1 行 INSERT
- `get_stats` の戻り JSON を `type` で振り分けて各 `rtc_stats_*` テーブルに INSERT
- 未知 type は `OnceLock<Mutex<HashSet>>` で初回のみ warn
- `config_json` は `nojson::DisplayJson` 手書きで構築、機密 4 フィールドは `"<masked>"` で出力
- `--no-duckdb-output` 指定時は writer task を起動せず noop クライアントを使用

### テスト

- スキーマ生成 (テーブル 10 / シーケンス 8 / インデックス 9 / `instance_id` 列位置)
- `rtc_stats_codec` UNIQUE 制約の重複排除
- RTCStats JSON 振り分け (既知 7 type + 未知 1 type)
- 未知 type の warn 抑制 (投入前後の差分で検証)
- `zakuro` テーブルの INSERT + UPDATE フロー
- `config_json` の機密マスクと None 省略
- `parse_offer_ids` の正常 / 異常系
- ファイル名生成のパターン
- DuckDB 引数のパース / 境界値 / ディレクトリ検証 / `--no-duckdb-output` 優先
