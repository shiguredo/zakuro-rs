# 複数 Zakuro インスタンスの起動 (instances 配列と instance-hatch-rate) を実装する

- Priority: Medium
- Created: 2026-06-24
- Completed:
- Model: Opus 4.7
- Branch: feature/add-instance-hatch-rate
- Polished: 2026-06-24

## 目的

zakuro (C++) の `--instance-hatch-rate` と JSONC `instances` 配列に相当する機能を zakuro-rs で実現する。
1 プロセスで複数の Zakuro 設定 (役割・チャネル ID・vcs 数・コーデックなどが異なる) を同時に動かしつつ、各インスタンスの起動タイミングを `i / instance-hatch-rate` 秒ずつ遅らせて段階的に起動できるようにする。

C++ 版の中核は `zakuro/src/main.cpp:303-321` のループで、`std::thread` を起動して `i / instance_hatch_rate` 秒スリープ後に `Zakuro::Run()` を呼ぶ。zakuro-rs では `tokio::spawn` + `tokio::time::sleep_until` で同等のセマンティクスを実現する。

各インスタンスは独立した `SoraConnectionContext` と `vcs` 群を持つ。例えば「インスタンス A は sendonly 50 個・VP8」「インスタンス B は recvonly 100 個」を同一プロセスで並列に走らせ、起動タイミングだけずらすという用途。

C++ 版・zakuro-rs ともにシングルプロセス・マルチスレッド実装 (C++ 版は `std::thread`、zakuro-rs は tokio タスクで実現)。

## 優先度根拠

Medium。

- C++ 版互換性ギャップ (`docs/ZAKURO.md:269` で未対応と明記)
- 1 プロセスで複数の負荷パターンを並行試験でき、統計集計・JSON-RPC を 1 プロセスに集約できる
- 代替手段 (複数 zakuro プロセスを並べる) で目的の負荷自体は出せるため即時必須ではない

## 現状

`src/args.rs` の `parse_args()` は CLI と JSONC を「単一の Zakuro 設定」として解析し、`src/main.rs` で 1 つの `SoraConnectionContext` を作って `args.vcs` 個の仮想クライアントを起動する。`--instance-hatch-rate` 相当の引数は実装されておらず、複数の Zakuro 設定を受け取る入り口も存在しない。

現在の `load_jsonc_config()` (`src/args.rs:9-54`) は JSONC のトップオブジェクトを `--key value` 形式のフラットな CLI 引数列に変換するだけで、ネスト構造や配列展開には対応していない。

`docs/ZAKURO.md:269` は未対応であることを明記している。同 306 行の設計差分表が C++ 版を「マルチプロセス」と記述しているのは事実誤認 (C++ 版もシングルプロセス・マルチスレッド)。完了条件で同時に訂正する。

## 設計方針

### 1. JSONC スキーマ

C++ 版 (`zakuro/src/main.cpp:147-176`, `zakuro/src/util.cpp:493-640`) のスキーマを基本とし、配列キーは `"instances"`、各 instance 内では `sora` オブジェクトでネストする。

```jsonc
{
  // 全インスタンス共通設定 (CommonArgs)
  "instance-hatch-rate": 1.0,
  "http-host": "127.0.0.1",
  "http-port": 8080,
  "openh264": "/path/to/libopenh264.so",
  "insecure": false,
  "client-cert": "/path/to/cert.pem",
  "client-key": "/path/to/key.pem",

  // 個別インスタンス設定の配列 (InstanceArgs)
  "instances": [
    {
      "vcs": 50,
      "vcs-hatch-rate": 1.0,
      "sora": {
        "signaling-url": "wss://sora.example.com/signaling",
        "channel-id": "zakuro-send",
        "role": "sendonly",
        "video-codec-type": "vp8",
        "video-bit-rate": 500,
        "metadata": {"access_token": "<TOKEN>"}
      }
    },
    {
      "vcs": 100,
      "vcs-hatch-rate": 5.0,
      "sora": {
        "signaling-url": ["wss://sora1.example.com/signaling", "wss://sora2.example.com/signaling"],
        "channel-id": "zakuro-recv",
        "role": "recvonly"
      }
    }
  ]
}
```

#### 1.1 C++ 版との互換性差分

| 観点 | C++ 版 | zakuro-rs |
|---|---|---|
| `instances` キー欠如時 | エラー (`main.cpp:147-149`) | 1 要素の `instances` として扱う (zakuro-rs 既存 JSONC との後方互換) |
| 最上位の `InstanceArgs` 関連キー | テンプレート機能なし | 「全 instance への共通テンプレート」として扱う (既存フラット JSONC との互換) |
| `instances[i]` 内の `CommonArgs` キー | サイレントに無視 | パースエラー (誤設定の早期検出) |
| `openh264` / `insecure` / `client-cert` / `client-key` | per-instance に書ける | `CommonArgs` に分類 (単一プロセス前提との整合性) |
| `--instance-hatch-rate` の範囲 | `[0.1, 100.0]` (`util.cpp:76-78`) | `> 0.0` のみ (zakuro-rs の `vcs-hatch-rate` バリデーション `src/args.rs:479-481` と統一) |
| `sora.signaling-url` 配列 | 複数引数で push (`util.cpp:585-602`) | カンマ結合して単一値にする (zakuro-rs の `--sora-signaling-url` は `split(',')` 仕様) |

#### 1.2 初期実装の対象外

`instances[i]` 内に以下のキーを書いた場合は `rtc_log_warning!` で警告を出して当該キーを無視する (パースは継続)。最上位に書いた場合も同じ扱い。

- `instance-num` (同一設定の N 複製)
- `${}` を含む値 (`ConvertEnv` 環境変数置換)
- `name` (instance 名指定。ログプレフィックスに使う C++ 版の機能)

これらは関連 issue として後続で対応する。

#### 1.3 エラー条件

- `instances` 配列が空 / 配列以外: パースエラー
- `instances[i]` 内に `CommonArgs` キー (`sora` 配下ではない直下キー): パースエラー `"common option <key> cannot be specified inside instances[]"`
- JSONC 内 (最上位・instances[i] 直下のどちらも) に `config` キー: パースエラー (再帰ロード防止)
- `instances[i]` 内に書ける有効キーは `InstanceArgs` フィールドに対応するキーと `sora` ネストのみ。未知キーは noargs パース時に「unknown option」エラー

### 2. CommonArgs と InstanceArgs の分割

`Args` 構造体 (`src/args.rs:72-115`) を以下に分割する。`CommonArgs` と `InstanceArgs` のどちらにも `#[derive(Clone)]` を付ける。

#### CommonArgs (プロセス全体で 1 つ)

| フィールド | CLI 引数 | 備考 |
|---|---|---|
| `instance_hatch_rate` | `--instance-hatch-rate` | 新規追加。デフォルト `1.0` |
| `http_host` | `--http-host` | |
| `http_port` | `--http-port` | |
| `openh264` | `--openh264` | パス文字列 |
| `insecure` | `--insecure` | bool フラグ |
| `client_cert` | `--client-cert` | パス文字列 (PEM 読み込みは main 側で実施) |
| `client_key` | `--client-key` | パス文字列 (PEM 読み込みは main 側で実施) |

#### InstanceArgs (インスタンスごと)

`CommonArgs` 以外のすべての `Args` フィールド (`src/args.rs:72-115` を参照)。

#### キー判定関数

```rust
fn is_common_key(key: &str) -> bool {
    matches!(
        key,
        "instance-hatch-rate"
            | "http-host"
            | "http-port"
            | "openh264"
            | "insecure"
            | "client-cert"
            | "client-key"
    )
}
```

`CommonArgs` にフィールドを追加するときはこの関数も更新する。

### 3. CLI 引数と JSONC のマージ規則

優先順位 (上位ほど優先、noargs の後勝ちセマンティクス):

1. CLI 引数
2. JSONC `instances[i]` の値
3. JSONC 最上位の `InstanceArgs` 関連キー (テンプレート)
4. デフォルト値

CLI 引数を `is_common_key()` で `common_cli_argv` と `instance_cli_argv` に分割する。

- 値付きオプション (`--key value` / `--key=value`): `=` 区切りで key を取り出し `is_common_key(key)` で振り分け、value も同じ側に積む
- bool フラグ単体 (`--insecure`, `--no-video-device`, `--sandstorm` 等): フラグを `is_common_key(key)` で振り分け。現状 `CommonArgs` の bool フラグは `--insecure` のみ
- `--config` / `--help` / `--version` は pre-parse で消費するため分割対象外

連結順序 (テンプレートは 4 節の `load_jsonc_config()` 内で各 `instance_argvs[i]` の先頭に焼き込み済み):

- `CommonArgs` パース用 argv: `[program_name, ...common_argv, ...common_cli_argv]`
- `InstanceArgs[i]` パース用 argv: `[program_name, ...instance_argvs[i], ...instance_cli_argv]`

`program_name` は `std::env::args().next()` から取得する。`--help` / `--version` の処理は `parse_args()` の冒頭で `std::env::args()` ベースに pre-parse し、ヘルプ表示時には `CommonArgs` 用の `RawArgs.finish()` から得たヘルプテキストを出力する (`InstanceArgs` パスでは未消費オプションを通常通りエラーにする)。

JSON オブジェクト値 (`sora.metadata` 等) はテンプレートと `instances[i]` で同名キーがあると `instances[i]` 側が文字列単位で後勝ちする (deep merge しない)。`sora` 配下は `--sora-{subkey}` にフラット展開されるため、サブキー単位で後勝ち上書きが成立する (テンプレートで `sora.signaling-url`、`instances[i]` で `sora.channel-id` のみ書く構成は動作する)。

### 4. データ構造と関数シグネチャ

```rust
pub(crate) struct JsoncConfig {
    pub(crate) common_argv: Vec<String>,
    pub(crate) instance_argvs: Vec<Vec<String>>,  // テンプレート焼き込み済み
}

// JSONC ファイルをロードしてパースする
pub(crate) fn load_jsonc_config(path: &str) -> Result<JsoncConfig>;

// JSONC 文字列からパースする内部関数 (単体テスト用、ライフタイム引数なし)
fn parse_jsonc_config(content: &str) -> Result<JsoncConfig>;

// プロセス入口
pub(crate) fn parse_args() -> Result<(CommonArgs, Vec<InstanceArgs>)>;

// 単体テスト用の純粋関数 (env IO を持たない)
fn parse_args_from_argv(
    program_name: &str,
    common_argv: Vec<String>,
    instance_argvs: Vec<Vec<String>>,
) -> Result<(CommonArgs, Vec<InstanceArgs>)>;
```

`parse_jsonc_config()` は `nojson::RawJson` の借用を内部に閉じ込め、戻り値 `JsoncConfig` は `Vec<String>` 系のフィールドで構成する (`as_raw_str()` の戻り値は `.to_string()` で必ず所有化する)。`parse_args()` は env / JSONC I/O / pre-parse を実行した後、最終 argv を組み立てて `parse_args_from_argv()` に委譲する。

`load_jsonc_config()` の動作:

1. JSONC のトップオブジェクトを取得
2. 各キーを以下に振り分ける
   - `config`: エラー
   - `instances`: 配列値を別途保持
   - `is_common_key()` で true: `common_argv` に `--{key} {value}` で追加
   - 1.2 節の未対応キー: 警告ログ + 無視
   - それ以外: ローカル `instance_template_argv` に追加
3. JSON 値の展開規則
   - `Boolean`: `true` ならフラグ単体 `--{key}` を push、`false` ならキー無視 (現状 `load_jsonc_config` と同規則)
   - `String` / `Integer` / `Float`: `--{key} {value}` で push
   - `Array` / `Object`: `as_raw_str().to_string()` で JSON 文字列化して 1 引数 (現状 `load_jsonc_config` と同規則)。例外として `sora.signaling-url` が配列ならカンマ結合
   - `sora` ネスト配下は `--sora-{subkey}` に展開し、配下の各値に同じ規則を適用
4. `instances` 配列を検査
   - 配列でない / 空ならエラー
   - 各要素 (オブジェクト) について:
     - `is_common_key()` で true の直下キーがあればエラー
     - 1.2 節の未対応キーは警告ログ
     - 残りキーを `--{key} {value}` 化し、`sora` ネストはフラット展開
   - 結果を `instance_template_argv.clone()` の末尾に連結して `instance_argvs[i]` に格納 (instance がテンプレートを後勝ちで上書きする順)
5. `instances` キーが無ければ `instance_argvs = vec![instance_template_argv]`

### 5. インスタンス起動を tokio タスクに分離する

既存の `vcs-hatch-rate` 実装 (`src/main.rs:331-348`) と同じ「`Instant::now()` ベースの `sleep_until` + `tokio::select!` でキャンセル可能」のパターンに揃える。Ctrl+C ハンドラは for ループの前に起動し、hatch sleep 中のキャンセルを確実にする。

```rust
// Ctrl+C ハンドラを先に起動 (hatch sleep 中のキャンセル経路を確保)
let shutdown_token = token.clone();
tokio::spawn(async move {
    let _ = tokio::signal::ctrl_c().await;
    shutdown_token.cancel();
});

let hatch_start = tokio::time::Instant::now();
let interval = Duration::from_secs_f64(1.0 / common.instance_hatch_rate);
let mut instances: JoinSet<Result<()>> = JoinSet::new();

for (i, instance) in instance_args_vec.into_iter().enumerate() {
    if token.is_cancelled() { break; }

    let target = hatch_start + interval * (i as u32);  // i: usize → u32 (instance 数は u32 上限を超えない)
    let task_token = token.child_token();
    let openh264_lib = openh264_lib.clone();           // Option<Openh264Library> の clone は安価 (内部 Arc<DynLib>)
    let common = common.clone();
    let client_cert_pem = client_cert_pem.clone();
    let client_key_pem = client_key_pem.clone();
    let stats_tx = stats_tx.clone();

    instances.spawn(async move {
        if i > 0 {
            tokio::select! {
                biased;
                _ = task_token.cancelled() => return Ok(()),
                _ = tokio::time::sleep_until(target) => {}
            }
        }
        run_zakuro_instance(
            i as u32, common, instance, openh264_lib,
            client_cert_pem, client_key_pem, task_token, stats_tx,
        ).await
    });
}

// for ループ完了後 (break 含む) に main 側の stats_tx を drop し、aggregator が channel close で停止できるようにする
drop(stats_tx);

while let Some(result) = instances.join_next().await {
    match result {
        Ok(Err(e)) => rtc_log_warning!("Zakuro instance failed: {}", e),
        Err(e) => rtc_log_warning!("Zakuro instance panicked: {}", e),
        _ => {}
    }
}

// reporter (定期統計出力) を停止する (aggregator は上記の drop で channel close により停止済み)
token.cancel();
```

タイムテーブル例 (3 instance、`instance-hatch-rate=1.0`、各 `vcs=2`、`vcs-hatch-rate=2.0`):

- t=0.00s: instance 0 起動 → vc(0,0) 即起動 / vc(0,1) は t=0.50s
- t=1.00s: instance 1 起動 → vc(1,0) 即起動 / vc(1,1) は t=1.50s
- t=2.00s: instance 2 起動 → vc(2,0) 即起動 / vc(2,1) は t=2.50s

### 6. `run_zakuro_instance` のシグネチャ

```rust
async fn run_zakuro_instance(
    instance_id: u32,
    common: CommonArgs,
    instance: InstanceArgs,
    openh264_lib: Option<Openh264Library>,
    client_cert_pem: Option<String>,   // main 側で読み込み済み
    client_key_pem: Option<String>,    // main 側で読み込み済み
    token: CancellationToken,
    stats_tx: mpsc::Sender<StatsEvent>,
) -> Result<()>;
```

main の責務 (プロセスに 1 つ): 引数パース、OpenH264 ロード、mTLS PEM 読み込み (`CommonArgs.client_cert` のパスから `read_to_string` で PEM 文字列を取得して `run_zakuro_instance` に渡す)、`StatsCollector::new(total, instances_count, token)` (`total = Σ instance.vcs`、`instances_count = instance_args_vec.len() as u32`)、HTTP サーバー起動 (両方ある場合)、Ctrl+C ハンドラ (for ループの前)、instance スポーン (5 節擬似コードに従う)。

`run_zakuro_instance` の責務 (インスタンスごと): `Mp4SampleReader` 構築 + コーデック検証 (`args::parse_video_codec_type` 呼び出し)、`FakeAudioCapturer` + `BeepTrigger` 初期化、`SoraConnectionContext` 構築 (NopVideoDecoder / OpenH264 / MP4 パススルー登録)、video capturer と `video_source` 構築、メタデータ JSON パース、`VirtualClientConfig` 構築、既存 `vcs-hatch-rate` ループ実行 (内部 `JoinSet`)、vc 完了待機。

`_fake_audio_capturer` / `_fake_capturer` / `_device_capturer` / `_mp4_capturer` のキャプチャ変数は Drop で thread join するため、`run_zakuro_instance` 内ローカル変数として保持し、`SoraConnectionContext` より後 (`Drop` 順は宣言順の逆) に宣言して context Drop 後に capturer Drop が走る順序にする。

### 7. 共有リソースの扱い

| リソース | 扱い |
|---|---|
| HTTP サーバー | プロセスに 1 つ |
| Ctrl+C ハンドラ | プロセスに 1 つ。for ループの前に起動 |
| 親 `CancellationToken` | プロセスに 1 つ。instance ごとに `child_token()` を派生 |
| `StatsCollector` | プロセスに 1 つ |
| Openh264 ライブラリ | プロセスに 1 回ロード。`shiguredo_openh264::Openh264Library` は `#[derive(Clone)]` で内部 `Arc<DynLib>` を持つため、`Option<Openh264Library>` を `.clone()` で各 instance に配布する (`Arc` 二重ラップは不要) |
| mTLS PEM 文字列 | main で 1 度だけ `read_to_string` し、`Option<String>` を `.clone()` で各 instance に渡す |
| `SoraConnectionContext` | インスタンスごとに新規生成 |
| `FakeAudioCapturer` / `BeepTrigger` | インスタンスごとに新規生成 (`BeepTrigger` は内部 `Arc<AtomicBool>` の `swap(false)` トリガで、共有すると take 競合のため必ず instance ごとに `new()`) |
| `FakeVideoCapturer` / `VideoDeviceCapturer` / `Mp4VideoCapturer` および `VideoTrackSource` | インスタンスごとに `run_zakuro_instance` 内で構築。`video_source.clone()` を vc に配る |
| `Mp4SampleReader` | インスタンスごとに新規生成 (同じ MP4 パス N 指定でメモリ N 倍) |

`--video-input-device` で同じ物理カメラを複数 instance で指定すると OS レベルで多重オープン制限に抵触する可能性があるが、当該 instance のみ失敗扱いとし他 instance には影響させない (10 節)。

### 8. `StatsCollector` のインスタンス対応

- `StatsEvent` の各 variant に `instance_id: u32` を追加し、現行 `id: u32` を `vc_id: u32` に改名する
  ```rust
  pub(crate) enum StatsEvent {
      Connected { instance_id: u32, vc_id: u32 },
      Disconnected { instance_id: u32, vc_id: u32 },
      Retrying { instance_id: u32, vc_id: u32, retry_count: u32 },
      Stopped { instance_id: u32, vc_id: u32 },
  }
  ```
- `StatsSnapshot` に `instances: u32` フィールドを追加し、`StatsSnapshot::initial(total: u32, instances: u32)` に変更 (`instances` は起動時の総 instance 数で reporter 全期間で不変)
- `StatsCollector::new(total: u32, instances_count: u32, token: CancellationToken)` にシグネチャ変更
- `reporter` ログを `"[stats] instances={} total={} connected={} retrying={} stopped={}"` に変更
- `StatsSnapshot::apply` の vc 単位ログプレフィックスを `"[stats] i{}/vc-{}"` 形式に変更
- vc id はインスタンス内で `0..instance.vcs` の独立採番 (instance 間で衝突するが `instance_id` で識別可能)

### 9. ログプレフィックスと既存日本語ログの英語化

複数 instance の vc 識別を確保するため、ログプレフィックスを `[i{instance_id}/vc-{vc_id}]` に統一する。`CLAUDE.md` 「ログメッセージは全て英語にすること」規約に整合させるため、プレフィックス変更で触る行は本文も英語化する (`[英語プレフィックス + 日本語本文]` の混在状態を作らない)。

- `src/virtual_client.rs`: 既存日本語ログ (`run` 内の `"接続しました"` 等、`src/virtual_client.rs:68, 71, 89, 124, 132, 143, 154, 169` 付近) を英語化しプレフィックスを統一する。`run(id, ...)` のシグネチャを `run(instance_id: u32, vc_id: u32, ...)` に変更
- `src/data_channel.rs`: `run_messaging(id, ...)` のシグネチャを `run_messaging(instance_id: u32, vc_id: u32, ...)` に変更。ログプレフィックス (本文は元から英語) を統一
- `src/data_channel.rs:197` の xorshift32 seed を `0xDEAD_BEEF ^ vc_id.wrapping_mul(2654435761) ^ instance_id.wrapping_mul(0x9E3779B9)` に変更する。これは「複数 instance で同じ `vc_id` の DataChannel メッセージ payload 乱数列が一致する」問題を避ける目的 (ZAKURO ヘッダ内の counter / time_us / connection_id は別計算なので衝突しない)。`instance_id=0` (シングルインスタンス時) は既存 seed と完全一致するため後方互換
- `src/stats.rs`: 8 節のフォーマット文字列変更とプレフィックス変更のみ (本文は元から英語)
- `src/main.rs`: 起動ログ (`src/main.rs:107-113`) を解決方法の新ログに置換。`src/main.rs:350` の `"仮想クライアント {} を起動します"` は `run_zakuro_instance` 内に移動し英語化。main 側に残るログ (`src/main.rs:381` Ctrl+C 受信、`388` パニック、`395` 全終了) は instance プレフィックスを付けず本文のみ英語化する

### 10. CLI 引数追加と起動失敗時の挙動

`--instance-hatch-rate <RATE>` を新規追加する。

- デフォルト: `1.0`
- バリデーション: `> 0.0`。0 以下や負数はパースエラー `"instance-hatch-rate は正の数で指定してください"` (`vcs-hatch-rate` と統一)
- 上限値は設けない
- ヘルプテキストは日本語 (既存 `vcs-hatch-rate` のトーン)。「複数インスタンスは JSONC `instances` 配列でのみ指定可能」を含める

起動失敗時:

- `run_zakuro_instance` が `Err` を返したら warning ログを出し他インスタンスを継続
- パニックは `JoinSet::join_next()` の `Err` でキャッチし warning ログ
- hatch sleep 中の Ctrl+C で起動順未到達 instance は起動せず `Ok(())` で終了
- vc 単位の `max_retry` / `retry_interval` は instance 内で従来通り機能。instance 単位 retry は本 issue では実装しない
- プロセス終了コード: Ctrl+C / 全 instance 自然終了で 0、引数解析エラーで非 0

## 完了条件

- JSONC 設定で `instances` 配列を指定すると、各要素が独立した `SoraConnectionContext` と `vcs` 群を持つ Zakuro インスタンスとして起動する
- 各インスタンスは i 番目なら `i / instance-hatch-rate` 秒の遅延後に起動する。新規ログ `"Starting zakuro instance {} at +{:.2}s"` の `+{:.2}s` 値が想定値に対し ±200ms 以内 (手動 E2E で確認)
- `instances` 配列が無い JSONC、および `--config` 無しの CLI 単独起動では従来通り単一インスタンスで動作する (後方互換)
- 起動 hatch sleep 中の Ctrl+C で当該 instance は起動せず、全インスタンスがグレースフルにシャットダウンする
- HTTP サーバーは全インスタンスで 1 つ共有する
- 1 インスタンスの起動失敗は他を巻き込まず warning ログのみで継続する
- `StatsEvent` の各 variant に `instance_id` が追加され、`id` が `vc_id` にリネームされる
- `StatsSnapshot` に `instances` フィールドが追加され、reporter ログが新フォーマット `"[stats] instances={} ..."` で出力される
- 9 節対象ファイルのログプレフィックスが `[i{instance_id}/vc-{vc_id}]` 形式に統一され、対象行の本文も英語化される。main 側に残るログ (`src/main.rs:381` Ctrl+C 受信、`388` パニック、`395` 全終了) も本文英語化される
- `data_channel.rs` の xorshift32 seed が `instance_id` を混ぜた式に変更される
- `virtual_client::run` と `data_channel::run_messaging` のシグネチャに `instance_id: u32` が追加される
- `--instance-hatch-rate` のバリデーション・`CommonArgs` キーが `instances[i]` 内にある場合のエラー・`instances` 配列が空の場合のエラーが実装される
- `parse_video_codec_type()` が `src/args.rs` に `pub(crate)` で移動され、`use shiguredo_webrtc::VideoCodecType;` が追加され、唯一の呼び出し元 (現 `src/main.rs:119`) が `run_zakuro_instance` 内に移動して `args::parse_video_codec_type(...)` で呼ぶ
- テスト戦略に挙げた単体テストが `cargo test` でグリーンになる
- `docs/ZAKURO.md` を以下のように更新する:
  - `269` 行付近の TODO 行を `- [x] instance-hatch-rate (instances 配列と組み合わせて使用)` に変更 (シナリオセクションのまま)
  - `306` 行付近の設計差分表「インスタンス起動」行を「C++ 版・Rust 版いずれもシングルプロセス。C++ は `std::thread`、zakuro-rs は tokio タスク」に修正 (マルチプロセス事実誤認の訂正)
  - 主要 CLI 引数表 (`136` 行付近) の `--instance-hatch-rate` の説明を最新化

## 解決方法

設計方針 (1〜10 節) に従い `src/args.rs` / `src/main.rs` / `src/stats.rs` / `src/virtual_client.rs` / `src/data_channel.rs` を修正する。`src/json_rpc.rs` は本 issue では変更しない (現状の `GetVersion` は引数なしで instance 概念に無関係)。

新規追加・置換ログ (英語、`f64` は `{}` Display、`Option<f64>` は `{:?}` Debug、経過秒は `{:.2}`):

- main 側起動ログ (`src/main.rs:107-113` を置換): `"zakuro: instances={} instance-hatch-rate={} total-vcs={}"`
- `run_zakuro_instance` 内起動ログ: `"Zakuro instance {}: vcs={} vcs-hatch-rate={} duration={:?} repeat_interval={:?}"`
- hatch sleep 終了後: `"Starting zakuro instance {} at +{:.2}s"` (`hatch_start.elapsed().as_secs_f64()` を出力)
- 正常終了: `"Zakuro instance {} finished"`
- エラー: `"Zakuro instance {} failed: {}"` (warning)
- パニック: `"Zakuro instance {} panicked: {}"` (warning)
- main 側残存ログ (`src/main.rs:381, 388, 395`) の英語化: 例 `"Ctrl+C received, shutting down..."`, `"Zakuro instance task panicked: {}"`, `"zakuro: all Zakuro instances finished"`

## テスト戦略

単体テストは `src/args.rs` 末尾の `#[cfg(test)] mod tests` に書く。`parse_jsonc_config(content: &str)` と `parse_args_from_argv(...)` を直接呼ぶことで env / I/O 依存を排除する。

検証項目:

- `parse_jsonc_config()` が `instances` 配列を per-instance argv に展開する
- `sora` ネストが `--sora-{key}` にフラット展開される
- `sora.signaling-url` の string と string 配列の両形式を受け付ける (配列はカンマ結合)
- `sora.metadata` 等のオブジェクト値が JSON 文字列として 1 引数に渡る
- `Boolean true` がフラグ単体、`false` がキー無視として展開される
- `CommonArgs` キー (`http-host` 等) が `instances[i]` 内にある場合にエラー
- `instances` 配列が空ならエラー
- `instances` キーが無い JSONC が 1 インスタンス分の argv として扱われる (後方互換)
- JSONC 最上位の `InstanceArgs` キーがテンプレートとして適用され、`instances[i]` で後勝ち上書きされる (例: テンプレートに `vcs=10`、`instances[0]` に `vcs=20` で最終的に `vcs=20`)
- `parse_args_from_argv()` で `instance_hatch_rate <= 0.0` がエラー
- 未対応キー (`instance-num` 等) が警告ログを出し処理は継続する

PBT は本 issue では採用しない (検証対象が決定論的な変換で、列挙的テストで主要分岐を網羅可能)。

手動 E2E:

- 2 インスタンス JSONC (`instance-hatch-rate=0.5`、instance0 = sendonly 2vc、instance1 = recvonly 5vc) で起動し、`"Starting zakuro instance 1 at +{:.2}s"` の値が 2.0 ± 0.2s 以内であることを確認
- ログプレフィックスが `[i0/vc-*]`, `[i1/vc-*]` で区別され、両者が同時に存在することを確認
- `instances` キーが無い JSONC で従来通り動作することを確認
- CLI 単独起動で従来通り動作することを確認
- 起動 hatch sleep 中の Ctrl+C で起動順未到達 instance が起動せず、全 instance がシャットダウンすることを確認
- 1 instance を意図的に起動失敗させたとき (無効な signaling URL を指定) 他 instance が継続することを確認

## 関連 issue

- 0005 (add-duckdb-stats-writer): インスタンス別 stats の DuckDB 書き出しは本 issue 完了後に対応。`StatsEvent.instance_id` を活用する
- 0006 (add-rpc-query-method): JSON-RPC `Query` メソッドで DuckDB 任意 SQL クエリを実行する。本 issue で `instance_id` を持つため、`Query` 経由で `WHERE instance_id=...` の SQL を投げれば instance 別集計が可能
- 0008 (add-ui-proxy): HTTP サーバーが全 instance 共有である前提を共有する
- 本 issue 完了後に CLI 引数を追加する後続 issue (`is_common_key()` の更新と `CommonArgs` / `InstanceArgs` への分類が必要): 0010 (fixed-resolution → InstanceArgs)、0011 (fake-audio-generate → InstanceArgs)、0012 (wav-audio-capture → InstanceArgs)、0015 (degradation-preference → InstanceArgs)、0018 (log-level → CommonArgs)
- 別 issue として新規予約 (本 issue 完了後に必要性を確認したうえで起票、SEQUENCE は起票時に消費):
  - `instance-num` キーによる同一設定 N 複製対応
  - `${}` 環境変数置換 (`ConvertEnv` 互換) 対応
  - `name` キーによる instance 名指定対応 (ログプレフィックスを `[{instance_name}/vc-{vc_id}]` 形式に切り替える)
