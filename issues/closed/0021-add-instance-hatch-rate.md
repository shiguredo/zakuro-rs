# 複数 Zakuro インスタンスの起動 (instances 配列と instance-hatch-rate) を実装する

- Priority: Medium
- Created: 2026-06-24
- Completed: 2026-06-26
- Model: Opus 4.7
- Branch: feature/add-instance-hatch-rate
- Polished: 2026-06-26

## 目的

zakuro (C++) の `--instance-hatch-rate` と JSONC `instances` 配列に相当する機能を zakuro-rs で実現する。
1 プロセスで複数の Zakuro 設定 (役割・チャネル ID・vcs 数・コーデックなどが異なる) を同時に動かしつつ、各インスタンスの起動タイミングを `i / instance-hatch-rate` 秒ずつ遅らせて段階的に起動できるようにする。

C++ 版の中核は `zakuro/src/main.cpp:303-321` のループで、`std::thread` を `i / instance_hatch_rate` 秒スリープ後に起動する。zakuro-rs は tokio タスクで同等のセマンティクスを 5 節の方針 (`DelayQueue` + `JoinSet`) で実現する。

各インスタンスは独立した `SoraConnectionContext` と `vcs` 群を持つ。例えば「インスタンス A は sendonly 50 個・VP8」「インスタンス B は recvonly 100 個」を同一プロセスで並列に走らせ、起動タイミングだけずらすという用途。

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
- `${...}` 形式の文字列値 (`ConvertEnv` 環境変数置換)。検出は「value が `String` で `s.contains("${")` を満たすもの」とする (実装はシンプルな部分文字列マッチで足りる)
- `name` (instance 名指定。ログプレフィックスに使う C++ 版の機能)

これらは関連 issue として後続で対応する。

警告ログのフォーマットは以下の 2 系統に分ける (どの instance で起きたか追跡できるようにするため):

- 最上位に書かれた場合: `"Unsupported top-level config key '{key}', ignoring"`
- `instances[i]` 内に書かれた場合: `"Unsupported config key '{key}' in instances[{i}], ignoring"`

#### 1.3 エラー条件

- `instances` 配列が空 / 配列以外: パースエラー
- `instances` 配列の長さが 1..=64 の範囲外: パースエラー `"instances は 1 から 64 の範囲で指定してください"` (per-instance `vcs` 上限 1000 と合わせ、総 vcs 上限を実用範囲に保つ)
- `instances[i]` 内に `CommonArgs` キー (`sora` 配下ではない直下キー): パースエラー `"common option '{key}' cannot be specified inside instances[]"`
- JSONC 内 (最上位・`instances[i]` 直下のどちらも) に `config` キー: パースエラー (再帰ロード防止)
- JSONC 内 (最上位・`instances[i]` 直下のどちらも) の `sora` キーが Object 以外: パースエラー `"'sora' must be a JSON object"` (4 節の `--sora-{subkey}` フラット展開と `sora.signaling-url` 配列ハンドリングは Object 前提のため)
- `instances[i]` 直下に `sora-` プレフィックスを持つフラットキー (`sora-signaling-url` 等) が書かれた場合: パースエラー `"'{key}' must be nested under 'sora' object inside instances[]"` (フラット形式が Object 形式のサブキーをサイレントに上書きするのを防ぐ。最上位 (テンプレート) では従来通り許容)
- `instances[i]` 内に書ける有効キーは `InstanceArgs` フィールドに対応するキーと `sora` ネストのみ。未知キーは noargs パース時に「unknown option」エラー

`instances.len()` の上限 64 の根拠: 典型ユースケースは 1〜10 instance (sendonly / recvonly を分けるなど数種類のロールを混在試験する用途)。実装上は `SoraConnectionContext` / `FakeAudioCapturer` / `FakeVideoCapturer` などが instance ごとに OS リソース (libwebrtc factory / OS thread) を消費するため、開発機 1 台で安全に扱える目安として 64 を採用する。後日上限引き上げが必要になれば別 issue で対応する。

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

すべて args.rs 内 private (`fn ...`) で定義する。テストは同モジュール内 `mod tests` から直接呼び出せる。

```rust
/// CommonArgs に分類すべきキーかどうか
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

/// 値を伴わない bool フラグ単体のキー全体 (CommonArgs / InstanceArgs を問わない)
///
/// 3 節の CLI argv 分割ロジックで「次トークンを値として取るか取らないか」の判別に用いる。
/// 振り分け先 (common / instance) の判定は別途 `is_common_key()` で行うこと。
fn is_flag(key: &str) -> bool {
    matches!(
        key,
        "insecure"           // CommonArgs
            | "no-video-device"  // InstanceArgs
            | "no-audio-device"  // InstanceArgs
            | "sandstorm"        // InstanceArgs
    )
}
```

`CommonArgs` にフィールドを追加するときは `is_common_key()` を、bool フラグを追加するときは `is_flag()` も更新する。両関数の doc コメントにもこの運用ルールを残す。

### 3. CLI 引数と JSONC のマージ規則

優先順位 (上位ほど優先、noargs の後勝ちセマンティクス):

1. CLI 引数
2. JSONC `instances[i]` の値
3. JSONC 最上位の `InstanceArgs` 関連キー (テンプレート)
4. デフォルト値

CLI 引数を 2 節の `is_common_key()` で `common_cli_argv` と `instance_cli_argv` に分割する。「次トークンを値として取るか取らないか」の判別は 2 節の `is_flag()` で行う (振り分け先と独立)。分割ロジック (順序保持、`--key value` / `--key=value` / `--key` の 3 形式に対応):

1. トークンが `--key=value` 形式 (1 トークン内に `=` を含む): `=` の前を `key`、後ろを `value` とみなす。`is_common_key(key)` で振り分け、トークン全体を該当側に push (`=` 形式のまま)。次トークンには進まない
2. トークンが `--key` 形式かつ `is_flag(key)` が true: bool フラグ単体として `is_common_key(key)` で振り分け側に push。次トークンには進まない
3. トークンが `--key` 形式かつ `is_flag(key)` が false: 値付きオプションとして該当側に push し、次トークン (= value) も同じ側に push、index を 1 つ余分に進める
4. 上記以外の素のトークン (= positional) は本 issue では発生しない想定 (`noargs` の `RawArgs` は positional を許可するが、zakuro CLI は positional を持たない)

`is_common_key()` で true かつ `is_flag()` で true のフラグ (現状 `--insecure`) は `common_cli_argv` の単独 push に振り分けられる。`InstanceArgs` 側の bool フラグ (`--no-video-device` / `--no-audio-device` / `--sandstorm`) は `is_common_key()` が false なので `instance_cli_argv` の単独 push に振り分けられる。

`--config` / `--help` / `--version` は `parse_args()` 冒頭の pre-parse で消費されるため、分割対象 argv からも除去しておく (`InstanceArgs` 側 `RawArgs.finish()` で unknown option エラーにならないようにするため)。

連結順序 (テンプレートは 4 節の `load_jsonc_config()` 内で各 `instance_argvs[i]` の先頭に焼き込み済み):

- `CommonArgs` パース用 argv: `[program_name, ...common_argv, ...common_cli_argv]`
- `InstanceArgs[i]` パース用 argv: `[program_name, ...instance_argvs[i], ...instance_cli_argv]`

`program_name` は `std::env::args().next()` から取得する。`std::env::args()` は呼び出すたびに iterator を新規生成するため pre-parse 後でも値は変わらない。

`--help` / `--version` の扱い: `parse_args()` 冒頭で `std::env::args()` を 1 度走査し `--help` または `--version` の有無を判定する。

- `--version` 検出時: バージョンを表示して `exit(0)`
- `--help` 検出時: `CommonArgs` 用と「ダミー InstanceArgs」用の `RawArgs` をそれぞれ `[program_name, "--help"]` の argv で構築する (noargs 0.4 では `take_help()` が `--help` フラグの presence を見て help_mode を立てる仕様。空 argv では help_mode が立たず、必須 opt の `then()` が `MissingOpt` で早期 return するため `finish()` が `Some(help_text)` を返すパスに到達しない。よって `--help` を明示的に argv に含める)。両 `RawArgs` の `finish()` から取得したヘルプテキストを `CommonArgs ヘルプ → InstanceArgs ヘルプ` の順に連結して表示し `exit(0)`
- どちらも検出しない場合: 通常パスに進み、両 `RawArgs` を実 argv で構築する

`--help` / `--version` 検出後は argv 分割に進まないため、`common_cli_argv` / `instance_cli_argv` は両者を含まない (pre-parse で消費して以降のロジックには渡さない)。

JSON オブジェクト値 (`sora.metadata` 等) はテンプレートと `instances[i]` で同名キーがあると `instances[i]` 側が文字列単位で後勝ちする (deep merge しない)。`sora` 配下は `--sora-{subkey}` にフラット展開されるため、サブキー単位で後勝ち上書きが成立する (テンプレートで `sora.signaling-url`、`instances[i]` で `sora.channel-id` のみ書く構成は動作する)。`nojson::RawJson::to_object()` はキー登場順を保持するため、テンプレート + `instances[i]` argv の連結順序で「後勝ち」が決定論的に成立する。

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
    common_argv: Vec<String>,          // JSONC 最上位由来 (CommonArgs キー)
    common_cli_argv: Vec<String>,      // CLI 由来 (CommonArgs キー)
    instance_argvs: Vec<Vec<String>>,  // JSONC 最上位テンプレート + instances[i] 焼き込み済み
    instance_cli_argv: Vec<String>,    // CLI 由来 (InstanceArgs キー、全 instance に同じものを末尾連結)
) -> Result<(CommonArgs, Vec<InstanceArgs>)>;
```

`parse_jsonc_config()` は `nojson::RawJson` の借用を内部に閉じ込め、戻り値 `JsoncConfig` は `Vec<String>` 系のフィールドで構成する (`as_raw_str()` の戻り値は `.to_string()` で必ず所有化する)。`parse_args()` は env / JSONC I/O / pre-parse を実行した後、CLI 引数を 3 節のロジックで分割し、`parse_args_from_argv()` に渡す。連結順序は 3 節のとおり。

`nojson::RawJson::to_object()` のキー順序保持に依存している (テンプレート + `instances[i]` の連結順序で後勝ち上書きが決まる)。nojson 0.3 のドキュメントおよび実装 (`RawJsonValue::to_object()` の戻り値はキー登場順の `impl Iterator<Item = (RawJsonValue<'_>, RawJsonValue<'_>)>`) でキー順保持が明示されているのを利用する。将来 nojson が順序非保持に変わった場合、本実装は壊れる (検出はテンプレートと `instances[i]` で同名キーを書いて後勝ちを検査する単体テストで担保)。

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

`tokio_util::time::DelayQueue<u32>` に全 instance の起動オフセット (`i / instance-hatch-rate` 秒) を一括登録し、main 側ループで `tokio_stream::StreamExt::next()` を `tokio::select!` の biased アームで poll する。`Ctrl+C` ハンドラはループ前に起動し、hatch スケジュール待ち中のキャンセルを確実にする。`DelayQueue` が空になれば全 instance 起動完了として loop を抜ける。

`vcs-hatch-rate` (instance 内ループ) も同じ `DelayQueue` パターンに揃える。これにより instance / vcs の hatch スケジュールが宣言的・一貫した記述になる (6 節参照)。

```rust
use tokio_stream::StreamExt;
use tokio_util::time::DelayQueue;

// Ctrl+C ハンドラを先に起動 (DelayQueue poll 中のキャンセル経路を確保)
let shutdown_token = token.clone();
tokio::spawn(async move {
    let _ = tokio::signal::ctrl_c().await;
    shutdown_token.cancel();
});

let hatch_start = tokio::time::Instant::now();
let interval = Duration::from_secs_f64(1.0 / common.instance_hatch_rate);
let mut delay: DelayQueue<u32> = DelayQueue::new();
// instance 配列の長さは args バリデーションにより u32 範囲内
for i in 0..(instance_args_vec.len() as u32) {
    delay.insert(i, interval * i);
}

// StatsCollector は `instance_args_vec` を消費する前に長さと総 vcs を控えてから new する
let instances_count = instance_args_vec.len() as u32;
let total_vcs: u32 = instance_args_vec.iter().map(|i| i.vcs).sum();
let stats = StatsCollector::new(total_vcs, instances_count, token.clone());
let stats_tx = stats.event_tx();

// instance 引数は起動時に 1 度だけ消費するため Option<InstanceArgs> でラップして take する
let mut pending: Vec<Option<InstanceArgs>> = instance_args_vec.into_iter().map(Some).collect();
// Item を (instance_id, Result<()>) としておくことで JoinSet::join_next() の Ok 経路で
// instance_id を取り出せる。panic 経路 (JoinError) では取得不可。
let mut instances: JoinSet<(u32, Result<()>)> = JoinSet::new();

loop {
    tokio::select! {
        biased;
        _ = token.cancelled() => break,
        maybe_expired = delay.next() => {
            // DelayQueue が空になれば全 instance 起動完了
            let Some(expired) = maybe_expired else { break };
            let i = expired.into_inner();
            rtc_log_info!(
                "Starting zakuro instance {} at +{:.2}s",
                i,
                hatch_start.elapsed().as_secs_f64(),
            );
            let instance = pending[i as usize]
                .take()
                .expect("logical invariant: each instance_id is dispatched once via DelayQueue and taken on first dispatch");
            let task_token = token.child_token();
            let common = common.clone();              // Clone は安価 (パス文字列等のみ)
            let openh264_lib = openh264_lib.clone();  // Option<Openh264Library> の clone は内部 Arc<DynLib>
            let client_cert_pem = client_cert_pem.clone();
            let client_key_pem = client_key_pem.clone();
            let stats_tx = stats_tx.clone();
            // JoinSet の Item を (instance_id, Result<()>) にすることで、正常終了 / Err 経路で
            // instance_id を取り出せる。JoinError 経路 (panic) は instance_id 取得不可。
            instances.spawn(async move {
                let result = run_zakuro_instance(
                    i,
                    common,
                    instance,
                    openh264_lib,
                    client_cert_pem,
                    client_key_pem,
                    task_token,
                    stats_tx,
                ).await;
                (i, result)
            });
        }
    }
}

// loop を抜けた経路は 2 通り:
//   (1) token.cancelled() (Ctrl+C 等): aggregator は token.cancelled で先に break、
//       reporter も同様。後続の drop(stats_tx) と token.cancel() は idempotent。
//   (2) DelayQueue::next() が None (全 instance 起動完了): 以降は aggregator が
//       channel close を見て break する必要があるため、main 側の stats_tx を drop する。
drop(stats_tx);

while let Some(joined) = instances.join_next().await {
    match joined {
        Ok((id, Ok(()))) => rtc_log_info!("Zakuro instance {} finished", id),
        Ok((id, Err(e))) => rtc_log_warning!("Zakuro instance {} failed: {}", id, e),
        Err(e) => rtc_log_warning!("Zakuro instance task panicked: {}", e),
    }
}

// 経路 (2) で reporter (定期統計出力) を停止する。経路 (1) では既に cancel 済みだが
// token.cancel() は idempotent なため二度呼び出しても問題ない。
token.cancel();
```

HTTP サーバー起動は Ctrl+C ハンドラ起動と DelayQueue 構築の間に配置する (上記擬似コードでは省略しているが、`if let (Some(host), Some(port)) = (&common.http_host, common.http_port) { ... tokio::spawn(server.run(handler)); }` を hatch loop 前に置く)。これにより `instances` 起動を待たずに HTTP 応答可能になる。

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

戻り値は `Result<()>` のみで `instance_id` を返さない (引数として既に受け取っているため)。main 側で JoinSet に spawn する際に `(instance_id, result)` の形式に wrap する (5 節擬似コード参照)。

main の責務 (プロセスに 1 つ): 引数パース、OpenH264 ロード、mTLS PEM 読み込み (`CommonArgs.client_cert` のパスから `read_to_string` で PEM 文字列を取得して `run_zakuro_instance` に渡す)、`StatsCollector::new(total, instances, token)` (`total = Σ instance.vcs`、`instances = instance_args_vec.len() as u32`)、HTTP サーバー起動 (両方ある場合、Ctrl+C ハンドラ起動と DelayQueue 構築の間)、Ctrl+C ハンドラ (DelayQueue 起動ループ前)、instance スポーン (5 節擬似コードに従う)。

MP4 パススルーのコーデック一致検証は `run_zakuro_instance` 内で 1 回だけ行う (main 側では行わない)。理由: sora_sdk 2026.1 の `Mp4SampleReader::new(path)` はコンストラクタで `std::fs::read(path)` でファイル全体をメモリに展開し全サンプルのメタデータを事前解析する重い操作。同じパスを N instance が指す場合に main 側 + `run_zakuro_instance` で 2N 回開くとファイルサイズ × 2N のメモリ・I/O が起動レイテンシに直撃する。`run_zakuro_instance` 側で 1 回だけ実行し、不一致 (`reader.codec_type() != args::parse_video_codec_type(...)`) なら当該 instance のみ `Err` を返して他 instance に影響させない (10 節の起動失敗時挙動)。設定ミスの早期検出は引数バリデーション (`InstanceArgs` パース時) で次の範囲に絞る:

- ファイル存在: 現状の `src/args.rs:303-306` (`input-mp4: file not found`) のチェックをそのまま継続
- `video_codec_type` 文字列の妥当性 (`vp8`/`vp9`/`av1`/`h264`/`h265` のいずれか): 現状の `src/args.rs:325-326` のチェックをそのまま継続
- `input_mp4` 指定時に `video_codec_type` と `video_bit_rate` が必須: 現状の `src/args.rs:530-541` のチェックを per-instance に維持

`run_zakuro_instance` 内で発覚しうる残検証は「MP4 中身のコーデックと `video_codec_type` の不一致」「MP4 ファイル破損」「ファイル削除 (引数バリデーション後の TOCTOU)」のみ。これらは当該 instance のみ起動失敗扱い (10 節)。

`run_zakuro_instance` の責務 (インスタンスごと): `Mp4SampleReader::new()` 構築 + コーデック一致検証 (`args::parse_video_codec_type(...)` 呼び出し)、`use_fake_audio` 判定 (`!instance.no_audio_device && instance.audio && instance.role.wants_send() && instance.input_mp4.is_none() && instance.video_input_device.is_none()`、現状 `src/main.rs:143-147` と同条件を per-instance に評価)、`FakeAudioCapturer` + `BeepTrigger` 初期化、`SoraConnectionContext` 構築 (NopVideoDecoder / OpenH264 / MP4 パススルー登録)、video capturer と `video_source` 構築、メタデータ JSON パース、`VirtualClientConfig` 構築、`vcs-hatch-rate` の `DelayQueue<u32>` 起動ループ実行 (内部 `JoinSet`、5 節と同じ DelayQueue + biased select! パターン。コードは 5 節と同形のため省略)、vc 完了待機。

ローカル変数の宣言順序と Drop の関係: Rust は宣言順の逆で Drop が走る。capturer 側の thread が `SoraConnectionContext` から派生したリソース (`AudioDeviceModule` / `VideoTrackSource`) を参照しているため、**capturer を先に Drop し、その後で `SoraConnectionContext` を Drop する** ことが安全な順序。`run_zakuro_instance` 内では `context` を先に宣言、capturer を後に宣言する。`FakeAudioCapturer` は `SoraConnectionContextConfig::adm_config = AdmConfig::UseExternal(capturer.audio_device_module())` を介して構築時に `context_config` へ参照を渡す必要があるため、以下の「late-bind」パターンで宣言順を保つ:

```rust
// 1. context_config を構築するブロック内で capturer を一時生成し、Option<FakeAudioCapturer> として持ち出す
let (context_config, pending_audio_capturer): (SoraConnectionContextConfig, Option<FakeAudioCapturer>) = {
    let mut config = SoraConnectionContextConfig {
        adm_config: AdmConfig::NoAudioDevice,
        ..Default::default()
    };
    let pending = if use_fake_audio {
        let mut capturer = FakeAudioCapturer::new(beep_trigger.clone().expect("guarded by use_fake_audio"));
        capturer.start();
        config.adm_config = AdmConfig::UseExternal(capturer.audio_device_module());
        Some(capturer)
    } else {
        None
    };
    // MP4 パススルー / OpenH264 / NopVideoDecoder の登録はここで continue
    (config, pending)
};

// 2. context を先に宣言 (= Drop は最後)
let context = SoraConnectionContext::new_with_config(context_config)?;

// 3. context より「後に」 capturer 系を宣言する (= Drop は context より先)
//    pending_audio_capturer を late-bind することで宣言順序を保つ
let _fake_audio_capturer = pending_audio_capturer;
let mut _fake_capturer: Option<FakeVideoCapturer> = None;
let mut _device_capturer: Option<VideoDeviceCapturer> = None;
let mut _mp4_capturer: Option<Mp4VideoCapturer> = None;
// (以降、video capturer 系を構築して Some(...) を代入)
```

### 7. 共有リソースの扱い

| リソース | 扱い |
|---|---|
| HTTP サーバー | プロセスに 1 つ |
| 親 `CancellationToken` | プロセスに 1 つ。instance ごとに `child_token()` を派生 (instance 内では vc ごとにさらに `child_token()`) |
| `StatsCollector` | プロセスに 1 つ |
| Openh264 ライブラリ | プロセスに 1 回ロード。`shiguredo_openh264::Openh264Library` は `#[derive(Clone)]` で内部 `Arc<DynLib>` を持つため、`Option<Openh264Library>` を `.clone()` で各 instance に配布する (`Arc` 二重ラップは不要) |
| mTLS PEM 文字列 | main で 1 度だけ `read_to_string` し、`Option<String>` を `.clone()` で各 instance に渡す |
| `SoraConnectionContext` | インスタンスごとに新規生成 |
| `FakeAudioCapturer` / `BeepTrigger` | インスタンスごとに新規生成 (`BeepTrigger` は内部 `Arc<AtomicBool>` の `swap(false)` トリガで、共有すると take 競合のため必ず instance ごとに `new()`) |
| `FakeVideoCapturer` / `VideoDeviceCapturer` / `Mp4VideoCapturer` および `VideoTrackSource` | インスタンスごとに `run_zakuro_instance` 内で構築。`video_source.clone()` を vc に配る |
| `Mp4SampleReader` | インスタンスごとに `run_zakuro_instance` 内で 1 回新規生成 (コンストラクタでファイル全体をメモリ展開する重い操作のため main 側では生成しない。同じ MP4 パスを N 指定するとメモリは N 倍 = ファイルサイズ × N) |

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
- `StatsCollector::new(total: u32, instances: u32, token: CancellationToken)` にシグネチャ変更 (引数名は `StatsSnapshot::initial` と統一する)
- `reporter` 全体ログを `"[stats] instances={} total={} connected={} retrying={} stopped={}"` に変更 (instance/vc 識別子を持たない集計サマリのため `[i.../vc-...]` プレフィックスは付けない)
- `StatsSnapshot::apply` の vc 単位ログプレフィックスは 9 節の統一プレフィックス `[i{instance_id}/vc-{vc_id}]` に揃え、本文末尾に `[stats]` を付ける形式 `"[i{}/vc-{}][stats] connected"` 等に統一する (grep で `[stats]` 全体・`[i0/vc-3]` 単独どちらも引けるようにするため)
- vc id はインスタンス内で `0..instance.vcs` の独立採番 (instance 間で衝突するが `instance_id` で識別可能)

#### 8.1 aggregator / reporter の Stream 化

`StatsCollector` の内部タスクは `tokio_stream` のラッパーで `mpsc::Receiver<StatsEvent>` と `tokio::time::Interval` をいずれも `Stream` として扱い、`tokio_stream::StreamExt::next()` で消費する。シグネチャ (引数・戻り値) は変えず、内部実装のみ Stream ベースに置き換える。

aggregator (event 受信):

aggregator のシグネチャ (`event_rx, snapshot_tx, total, instances, token`) は `total` と `instances` を受け取り、`StatsSnapshot::initial(total, instances)` で初期化する。`mpsc::Receiver` のラップは関数本体内で `ReceiverStream::new(event_rx)` を呼ぶ形で行う (関数引数の型は変えない)。

```rust
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

let mut snapshot = StatsSnapshot::initial(total, instances);
let mut events = ReceiverStream::new(event_rx);
loop {
    tokio::select! {
        biased;
        _ = token.cancelled() => break,
        maybe_event = events.next() => {
            // channel が close されたら終了 (main の stats_tx drop 後 + 全 instance の Sender clone drop 後に発火)
            let Some(event) = maybe_event else { break };
            snapshot.apply(event);
            let _ = snapshot_tx.send(snapshot.clone());
        }
    }
}
```

reporter (定期出力):

```rust
use tokio_stream::StreamExt;
use tokio_stream::wrappers::IntervalStream;
use tokio::time::MissedTickBehavior;

let mut interval = tokio::time::interval(Duration::from_secs(5));
interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
let mut ticks = IntervalStream::new(interval);
loop {
    tokio::select! {
        biased;
        _ = token.cancelled() => break,
        _ = ticks.next() => {
            let snap = snapshot_rx.borrow_and_update().clone();
            rtc_log_info!(
                "[stats] instances={} total={} connected={} retrying={} stopped={}",
                snap.instances, snap.total, snap.connected, snap.retrying, snap.stopped,
            );
        }
    }
}
```

`watch::channel` (snapshot 通知) は引き続き使用する (Stream 化は不要)。

### 9. ログプレフィックスと既存日本語ログの英語化

複数 instance の vc 識別を確保するため、ログプレフィックス基本形を `[i{instance_id}/vc-{vc_id}]` に統一する。stats 系のログのみ末尾に `[stats]` サフィックスを追加して `[i{}/vc-{}][stats]` 形式とする (grep で `[stats]` 全体と `[i.../vc-...]` 単独のどちらでも引けるようにするため。8 節の `StatsSnapshot::apply` 出力のみが対象、reporter 全体ログは識別子を持たず `[stats]` 単独形式)。`CLAUDE.md` 「ログメッセージは全て英語にすること」規約に整合させるため、プレフィックス変更で触る行は本文も英語化する (`[英語プレフィックス + 日本語本文]` の混在状態を作らない)。

- `src/virtual_client.rs`: 既存日本語ログ (`run` 内の `"接続しました"` 等、`src/virtual_client.rs:68, 71, 89, 124, 132, 143, 154, 169` 付近) を英語化しプレフィックスを統一する。`run(id, ...)` のシグネチャを `run(instance_id: u32, vc_id: u32, ...)` に変更。具体的な置換テーブルは「解決方法」末尾参照
- `src/data_channel.rs`: `run_messaging(id, ...)` のシグネチャを `run_messaging(instance_id: u32, vc_id: u32, ...)` に変更。ログプレフィックス (本文は元から英語) を統一 (置換テーブルは「解決方法」末尾参照)
- `src/data_channel.rs:197` の xorshift32 seed 計算ロジックを `pub(crate) fn compute_seed(instance_id: u32, vc_id: u32) -> u32` として切り出し (テストから直接呼べるようにするため)、以下の式で計算する:

  ```rust
  pub(crate) fn compute_seed(instance_id: u32, vc_id: u32) -> u32 {
      let state = 0xDEAD_BEEF
          ^ vc_id.wrapping_mul(2654435761)            // 黄金比 * 2^32 (= 0x9E3779B1) Linux カーネル多用定数
          ^ instance_id.wrapping_mul(0x9E3779B9);     // 黄金比 * 2^32 (Knuth multiplicative hash 定数)。+ 8 ずらすことで vc_id 側との衝突確率を下げる
      if state == 0 { 1 } else { state }
      // xorshift32 は state=0 のとき永久に 0 を返す LFSR 性質を持つ。後述の唯一の vc_id で発火しうるため補正
  }
  ```

  `run_messaging` 側は `let mut xorshift_state = compute_seed(instance_id, vc_id);` で呼び出す。

  目的: 「複数 instance で同じ `vc_id` の DataChannel メッセージ payload 乱数列が一致する」問題を避ける (ZAKURO ヘッダ内の counter / time_us / connection_id は別計算なので衝突しない)。

  後方互換: `instance_id=0` のとき `instance_id.wrapping_mul(0x9E3779B9) == 0` で XOR が単位元として作用し、`state == 0xDEAD_BEEF ^ vc_id.wrapping_mul(2654435761)` となる。これは数学的には `vc_id == 0xDEAD_BEEF.wrapping_mul(inv(2654435761)) == 416_041_631` の 1 点で 0 になる (`2654435761` は奇数で mod 2^32 の逆元を持つ) が、10 節バリデーションで `vcs <= 1000` のため実用 `vc_id ∈ 0..=999` の範囲ではこの値に到達しない。よって `instance_id=0` の実用域では現行 seed と完全一致する。`state == 0` 補正は仮に到達した場合のフェイルセーフであり、現行実装の LFSR 退化挙動 (永久 0 を返す) を本実装で 1 始まりに是正する副作用がある (実用範囲外なので影響なし)。
- `src/stats.rs`: 8 節のフォーマット文字列変更とプレフィックス変更のみ (本文は元から英語)
- `src/main.rs`: 起動ログ (`src/main.rs:107-113`) を解決方法の新ログに置換。`src/main.rs:350` の `"仮想クライアント {} を起動します"` は `run_zakuro_instance` 内に移動し英語化。main 側に残るログ (`src/main.rs:381` Ctrl+C 受信、`388` パニック、`395` 全終了) は instance プレフィックスを付けず、かつ本文にも `instance_id` を含めず一括的なメッセージとして英語化する (パニック行は `JoinSet::join_next()` 経路で発生し instance_id が判明しない場合があるため、特定 id を含めない形式に統一)

### 10. CLI 引数追加とバリデーション、起動失敗時の挙動

`--instance-hatch-rate <RATE>` を新規追加する。

- デフォルト: `1.0`
- バリデーション: `> 0.0`。0 以下や負数はパースエラー `"instance-hatch-rate は正の数で指定してください"` (`vcs-hatch-rate` と統一)
- 上限値は設けない
- ヘルプテキストは日本語 (既存 `vcs-hatch-rate` のトーン)。「複数インスタンスは JSONC `instances` 配列でのみ指定可能」を含める

バリデーションの担当 (CommonArgs / InstanceArgs 分割後の整理):

- `CommonArgs` (`parse_args_from_argv()` の `CommonArgs` パース後に検査):
  - `instance_hatch_rate > 0.0`
  - `--http-host` と `--http-port` は両方指定 or 両方未指定 (現状 `src/args.rs:518-522` を CommonArgs パース後に再検査)
  - `--client-cert` と `--client-key` は両方指定 or 両方未指定 (現状 `src/args.rs:523-528` を CommonArgs パース後に再検査)
- `InstanceArgs[i]` (各 i について):
  - `vcs` は per-instance のまま `1..=1000` を継続適用 (現状 `src/args.rs:476-477` のレンジを `InstanceArgs` パース後に各 i について再検査)
  - `vcs_hatch_rate > 0.0` (現状 `src/args.rs:479-481` を `InstanceArgs` パース後に各 i について再検査)
  - その他の既存バリデーション (`framerate` 1..=60、`sandstorm`/`fake_video_capture` 排他、`input_mp4` と `video_codec_type`/`video_bit_rate` 必須 等) もすべて per-instance に維持
- 総 vcs 数 (`Σ instances[i].vcs`) には上限を設けない (1.3 節の `instances.len() <= 64` と per-instance `vcs <= 1000` で実用上 64000 まで)

起動失敗時:

- `run_zakuro_instance` が `Err` を返したら warning ログを出し他インスタンスを継続
- パニックは `JoinSet::join_next()` の `Err` でキャッチし warning ログ
- hatch sleep 中の Ctrl+C で起動順未到達 instance は起動せず `Ok(())` で終了
- vc 単位の `max_retry` / `retry_interval` は `InstanceArgs` 配下のまま instance 内で従来通り機能。instance 単位 retry は本 issue では実装しない
- プロセス終了コード: Ctrl+C / 全 instance 自然終了で 0、引数解析エラーで非 0

### 11. 採用する tokio エコシステムのクレートと利用箇所

| クレート / 型 | 利用箇所 | 用途 |
|---|---|---|
| `tokio::task::JoinSet` | main / `run_zakuro_instance` | instance / vc の spawn と結果回収 |
| `tokio::sync::{mpsc, watch}` | `stats.rs` | `StatsEvent` channel / `StatsSnapshot` 通知 |
| `tokio::signal::ctrl_c` | main | グレースフルシャットダウン契機 |
| `tokio::time::{Instant, Interval, MissedTickBehavior}` | main / stats | hatch 基準時刻 / 定期 reporter |
| `tokio_util::sync::CancellationToken` | 全体 | 階層的キャンセル (main → instance → vc) |
| `tokio_util::time::DelayQueue<u32>` | main / `run_zakuro_instance` | instance / vc の hatch スケジュール一括登録 |
| `tokio_stream::wrappers::ReceiverStream` | stats aggregator | `mpsc::Receiver<StatsEvent>` を `Stream` として消費 |
| `tokio_stream::wrappers::IntervalStream` | stats reporter | `tokio::time::Interval` を `Stream` として消費 |
| `tokio_stream::StreamExt` | main / stats | `.next()` を `tokio::select!` 内で使用 |

### 12. Cargo.toml 依存追加

```toml
[dependencies]
# 既存 (features 変更なし)
tokio = { version = "1.52", features = ["io-util", "macros", "net", "rt-multi-thread", "signal", "sync", "time"] }
# tokio ユーティリティ (CancellationToken, DelayQueue)
tokio-util = { version = "0.7", features = ["time"] }
# tokio Stream ラッパー (ReceiverStream, IntervalStream)
tokio-stream = "0.1"
```

`tokio-util` 0.7 では `default-features = []` で `sync` モジュール (`CancellationToken` 含む) は feature 設定不要で公開されている (`sync` という feature 名そのものが定義されていない)。本 issue では `DelayQueue` を利用するため `time` feature のみ追加する。

`tokio-stream` 0.1 は `[features] default = ["time"]` のため `tokio-stream = "0.1"` だけで `IntervalStream` (`time` feature gate 配下) と `ReceiverStream` (feature gate なし、無条件公開) と `StreamExt` (default 公開) がすべて利用できる。本 issue では features 指定は不要。

## 完了条件

### 機能

- JSONC 設定で `instances` 配列を指定すると、各要素が独立した `SoraConnectionContext` と `vcs` 群を持つ Zakuro インスタンスとして起動する
- 各インスタンスは i 番目なら `i / instance-hatch-rate` 秒の遅延後に起動する。新規ログ `"Starting zakuro instance {} at +{:.2}s"` の `+{:.2}s` 値が想定値に対し ±200ms 以内 (手動 E2E で確認)
- `instances` 配列が無い JSONC、および `--config` 無しの CLI 単独起動では従来通り単一インスタンスで動作する (後方互換)
- 起動 hatch sleep 中の Ctrl+C で未起動 instance は起動せず、全インスタンスがグレースフルにシャットダウンする
- HTTP サーバーは全インスタンスで 1 つ共有する
- 1 インスタンスの起動失敗は他を巻き込まず warning ログのみで継続する

### 構造変更

- `Args` 構造体が `CommonArgs` と `InstanceArgs` に分割される (2 節)
- `parse_args()` / `parse_args_from_argv()` / `load_jsonc_config()` / `parse_jsonc_config()` のシグネチャが 4 節のとおり実装される
- `parse_video_codec_type()` が `src/args.rs` に `pub(crate)` で移動し、`run_zakuro_instance` 内の MP4 コーデック一致検証 (6 節) で `args::parse_video_codec_type(...)` として呼ばれる (現状 `src/main.rs:119` の呼び出しを `run_zakuro_instance` 内に移動)
- `virtual_client::run` と `data_channel::run_messaging` のシグネチャに `instance_id: u32` が追加される (9 節)
- `StatsEvent` の各 variant に `instance_id` が追加され、`id` が `vc_id` にリネームされる (8 節)
- `StatsSnapshot::initial(total, instances)` と `StatsCollector::new(total, instances, token)` のシグネチャが変更される (8 節)
- instance hatch ループと vcs hatch ループの双方が `tokio_util::time::DelayQueue<u32>` + `tokio::select!` の biased アーム + `StreamExt::next()` パターンで実装される (5/6 節)
- `data_channel.rs` に `pub(crate) fn compute_seed(instance_id: u32, vc_id: u32) -> u32` が切り出され、`run_messaging` から呼び出される (state=0 回避付き、`instance_id` を混ぜた式、9 節)
- main 側 instance JoinSet の型を `JoinSet<(u32, Result<()>)>` とし、`spawn` 時に `(instance_id, run_zakuro_instance(...).await)` の形で wrap して `join_next()` 経路で instance_id を取得できるようにする (5 節)

### 依存追加

- `Cargo.toml` で `tokio-util` を `{ version = "0.7", features = ["time"] }` に変更し、コメントを `# tokio ユーティリティ (CancellationToken, DelayQueue)` に更新する
- `tokio-stream = "0.1"` を新規追加し、コメントを `# tokio Stream ラッパー (ReceiverStream, IntervalStream)` とする

### ログ

- 9 節対象ファイルのログプレフィックスが `[i{instance_id}/vc-{vc_id}]` 形式に統一され、対象行の本文も英語化される (置換テーブルは「解決方法」末尾に従う)
- main 側に残るログ (`src/main.rs:381, 388, 395`) は instance プレフィックス・instance_id ともに含めず、本文のみ英語化する
- stats reporter ログが `"[stats] instances={} total={} ..."` 形式で出力される (8 節)
- stats の vc 単位ログが `"[i{}/vc-{}][stats] ..."` 形式で出力される (8 節)

### ドキュメント・テスト

- テスト戦略に挙げた単体テストが `cargo test` でグリーンになる
- `docs/ZAKURO.md` を以下のように更新する:
  - `269` 行付近の `- [ ] instance-hatch-rate ...` を `- [x] instance-hatch-rate (instances 配列と組み合わせて使用)` に変更。現状 `vcs-hatch-rate` と同じ「### シナリオ」セクション配下に並べる (歴史的経緯による配置に揃える)
  - `306` 行付近の設計差分表「インスタンス起動」行を「C++ 版・Rust 版いずれもシングルプロセス。C++ は `std::thread`、zakuro-rs は tokio タスク」に修正 (マルチプロセス事実誤認の訂正)
  - 主要 CLI 引数表 (`136` 行付近) の `--instance-hatch-rate` の説明を最新化

## 解決方法

### 変更ファイル

`Cargo.toml` / `src/args.rs` / `src/main.rs` / `src/stats.rs` / `src/virtual_client.rs` / `src/data_channel.rs` / `docs/ZAKURO.md` を設計方針 (1〜12 節) に従い修正した。`src/json_rpc.rs` は本対応では変更していない (現状の `GetVersion` は引数なしで instance 概念に無関係)。

### 主要変更点

- `Cargo.toml`: `tokio-util = { version = "0.7", features = ["time"] }` に変更 (DelayQueue 用)、`tokio-stream = "0.1"` を新規追加 (ReceiverStream / IntervalStream 用)
- `src/args.rs`: `Args` 構造体を `CommonArgs` と `InstanceArgs` に分割。JSONC `instances` 配列のパースを `load_jsonc_config()` / `parse_jsonc_config()` で実装。CLI 引数の Common/Instance 分割を `split_cli_argv()`、後勝ち重複除去を `dedupe_argv_last_wins()` で実装 (理由: noargs::OptSpec::take は先勝ち消費で残りが「unexpected argument」エラーになるため、テンプレート + CLI の連結後に dedupe する必要があった)。`--instance-hatch-rate` を新規追加し `> 0.0` でバリデーション。`parse_video_codec_type()` を `pub(crate)` に格上げ
- `src/stats.rs`: `StatsEvent` の各 variant に `instance_id` を追加し `id` を `vc_id` にリネーム。`StatsSnapshot::initial(total, instances)` と `StatsCollector::new(total, instances, token)` のシグネチャ変更。aggregator / reporter を `tokio_stream::wrappers::ReceiverStream` / `IntervalStream` に置き換え
- `src/virtual_client.rs`: `run(id, ...)` を `run(instance_id, vc_id, ...)` に変更、日本語ログを英語化、プレフィックスを `[i{}/vc-{}]` に統一
- `src/data_channel.rs`: `run_messaging` のシグネチャに `instance_id` を追加。`compute_seed(instance_id, vc_id) -> u32` を `pub(crate)` で切り出し (state=0 回避付き、`instance_id` と `vc_id` で別の係数を XOR)
- `src/main.rs`: `main` を `fn main()` + `LocalSet::block_on(&rt, async_main())` に変更し、`async_main` 内で instance hatch スケジューリングを `tokio_util::time::DelayQueue<u32>` + `JoinSet<(u32, Result<()>)>` で実装。`run_zakuro_instance()` を新設し、`SoraConnectionContext` 構築 / 映像音声キャプチャ初期化 / vc 群の起動 (DelayQueue + JoinSet) を担当。`FakeAudioCapturer` などが `!Send` のため `JoinSet::spawn_local` を使用
- `docs/ZAKURO.md`: instance-hatch-rate を `[x]` に変更、設計差分表のマルチプロセス記述を「シングルプロセス・マルチスレッド」に修正、主要 CLI 引数表の説明を更新

### 設計方針との差異

- 設計方針 3 節「noargs の後勝ちセマンティクス」は実態と異なっていた (noargs::OptSpec::take は先勝ち)。これに対し `dedupe_argv_last_wins()` を追加して、argv 連結後に後勝ち重複除去を行う形で対応した
- 設計方針 5 節は `JoinSet::spawn` 前提だが、`FakeAudioCapturer` などの !Send オブジェクトを future が保持するため `JoinSet::spawn_local` + `tokio::task::LocalSet` パターンに変更した。`#[tokio::main]` も `fn main() -> Result<()>` + `LocalSet::block_on` に置き換えている
- 設計方針の `--help` / `--version` ハンドリングは、`parse_common_args` / `parse_instance_args` の冒頭で `noargs::HELP_FLAG.take_help` を呼び、ファイル存在チェックを `help_mode` で skip する設計に変更した

### テスト

- `src/args.rs` 末尾の `#[cfg(test)] mod tests` に単体テスト 13 件 (parse_jsonc_config 系 9 件、parse_args_from_argv 系 4 件)
- `src/data_channel.rs` 末尾の `#[cfg(test)] mod tests` に compute_seed テスト 3 件 (旧実装一致網羅、state=0 回避、instance 間 seed 分離)
- 合計 16 件、`cargo test --workspace` で全件パス、`cargo clippy --all-targets --all-features -- -D warnings` 通過、`cargo fmt --all -- --check` 通過

### 残課題 (別 issue として起票候補)

- shiguredo-rust 規約「単体テストは `tests/test_<module>.rs` に配置」「PBT は proptest で書く」未対応。本対応では issue 設計方針 (`src/<module>.rs` の `#[cfg(test)] mod tests` 配置、PBT 不採用) に従ったが、規約準拠の整備は別途必要
- `Duration::from_secs_f64()` の NaN / inf / 極小値防御 (現バリデーションは `> 0.0` のみ)
- `dedupe_argv_last_wins()` の未知オプション扱い (未知 `--key` を値付きとして 2 トークン化する挙動の妥当性検証)
- 設計改善 (run_zakuro_instance の引数構造体化、is_common_key/is_flag の運用ルール一元化など、観点 2 レビューで指摘された設計改善)

新規追加・置換ログ (英語、`f64` は `{}` Display、`Option<f64>` は `{:?}` Debug、経過秒は `{:.2}`):

- main 側起動ログ (`src/main.rs:107-113` を置換): `"zakuro: instances={} instance-hatch-rate={} total-vcs={}"`
- `run_zakuro_instance` 内起動ログ: `"Zakuro instance {}: vcs={} vcs-hatch-rate={} duration={:?} repeat_interval={:?}"`
- hatch sleep 終了後 (main の DelayQueue 起動ループ内): `"Starting zakuro instance {} at +{:.2}s"` (`hatch_start.elapsed().as_secs_f64()` を出力)
- 正常終了: `"Zakuro instance {} finished"`
- エラー: `"Zakuro instance {} failed: {}"` (warning)
- パニック (`JoinSet::join_next` の `Err` 経由、特定 instance_id 不明な経路あり): `"Zakuro instance task panicked: {}"` (warning)
- main 側残存ログ (`src/main.rs:381, 388, 395`) の英語化 (instance_id を含めない): `"Ctrl+C received, shutting down..."` / `"Zakuro instance task panicked: {}"` / `"zakuro: all Zakuro instances finished"`

`src/virtual_client.rs` の日本語ログ英語化置換テーブル (9 節対応):

| 行 (現状) | 現状本文 | 置換後 |
|---|---|---|
| `:68` | `"[vc-{}] クライアント構築に失敗: {}"` | `"[i{}/vc-{}] failed to build client: {}"` |
| `:71` | `"[vc-{}] 最大リトライ回数 ({}) に達しました"` | `"[i{}/vc-{}] reached max retry count ({})"` |
| `:89` | `"[vc-{}] 接続しました"` | `"[i{}/vc-{}] connected"` |
| `:124` | `"[vc-{}] シャットダウンします"` | `"[i{}/vc-{}] shutting down"` |
| `:132` | `"[vc-{}] duration が経過しました"` | `"[i{}/vc-{}] duration expired"` |
| `:143` | `"[vc-{}] {:.1} 秒後に再接続します"` | `"[i{}/vc-{}] reconnecting in {:.1}s"` |
| `:154` | `"[vc-{}] シナリオにより切断します"` | `"[i{}/vc-{}] disconnecting per scenario"` |
| `:169` | `"[vc-{}] 予期しない切断: {}"` | `"[i{}/vc-{}] unexpected disconnect: {}"` |
| `:174-178` | `"[vc-{}] 最大リトライ回数 ({}) に達しました"` (Unexpected 分岐) | `"[i{}/vc-{}] reached max retry count ({})"` |
| `:183-188` | `"[vc-{}] {:.1} 秒後にリトライします ({}/{})"` | `"[i{}/vc-{}] retrying in {:.1}s ({}/{})"` |

`src/data_channel.rs` の vc 単位ログ (本文は元から英語、プレフィックスのみ更新):

| 行 (現状) | 現状 | 置換後 |
|---|---|---|
| `:234` | `"[vc-{}] Send DataChannel label={} counter={} size={}"` | `"[i{}/vc-{}] Send DataChannel label={} counter={} size={}"` |
| `:244` | `"[vc-{}] DataChannel send failed: label={} error={}"` | `"[i{}/vc-{}] DataChannel send failed: label={} error={}"` |

`run_zakuro_instance` 内の vc 起動ログ (`src/main.rs:350` 「仮想クライアント {} を起動します」を移動): `"[i{}/vc-{}] starting virtual client"`。

## テスト戦略

単体テストは `src/args.rs` 末尾の `#[cfg(test)] mod tests` に書く。`parse_jsonc_config(content: &str)` と `parse_args_from_argv(...)` を直接呼ぶことで env / I/O 依存を排除する。テスト内の `assert!` / `panic!` メッセージは AGENTS.md 規約に従い日本語で記述する。

検証項目 (parse_jsonc_config):

- `instances` 配列を per-instance argv に展開する
- `sora` ネストが `--sora-{key}` にフラット展開される
- `sora.signaling-url` の string と string 配列の両形式を受け付ける (配列はカンマ結合)
- `sora.metadata` 等のオブジェクト値が JSON 文字列として 1 引数に渡る
- `Boolean true` がフラグ単体、`false` がキー無視として展開される
- `CommonArgs` キー (`http-host` 等) が `instances[i]` 内にある場合にパースエラー
- `instances[i]` 直下に `sora-` プレフィックスのフラットキーがある場合にパースエラー
- `sora` キーが Object 以外の場合にパースエラー
- `instances` 配列が空 / 配列以外 / 長さ 65 以上の場合にパースエラー
- `instances` キーが無い JSONC が 1 インスタンス分の argv として扱われる (後方互換)
- JSONC 最上位の `InstanceArgs` キーがテンプレートとして適用され、`instances[i]` で後勝ち上書きされる
- `config` キーが最上位・`instances[i]` 直下どちらにあってもパースエラー
- 未対応キー (`instance-num` / `name` / `${...}` 含む文字列) が警告ログを出し処理は継続する

検証項目 (parse_args_from_argv):

- 末尾連結で後勝ちが成立する (例: テンプレート由来の `instance_argvs[0] = vec!["--vcs", "10"]`、CLI 由来の `instance_cli_argv = vec!["--vcs", "20"]` を渡すと最終的に `InstanceArgs.vcs = 20` となる)
- `instance_hatch_rate <= 0.0` がパースエラー
- 各 `InstanceArgs[i]` について `vcs` が 0 または 1001 以上ならパースエラー
- 各 `InstanceArgs[i]` について `vcs_hatch_rate <= 0.0` ならパースエラー

サンプルケース (実装中の解釈ブレを防ぐため最低限の骨子として用意する。テスト本文は CLAUDE.md 規約に従い日本語で記述する):

```rust
#[test]
fn parse_jsonc_config_with_two_instances() {
    let content = r#"{
        "instance-hatch-rate": 2.0,
        "vcs": 5,
        "instances": [
            { "sora": { "channel-id": "a", "role": "sendonly" } },
            { "vcs": 10, "sora": { "channel-id": "b", "role": "recvonly" } }
        ]
    }"#;
    let cfg = parse_jsonc_config(content).expect("有効な JSONC のパースに失敗してはならない");
    assert_eq!(cfg.common_argv, vec!["--instance-hatch-rate".to_string(), "2.0".to_string()],
               "common_argv に instance-hatch-rate が含まれていない");
    assert_eq!(cfg.instance_argvs.len(), 2, "instance 数が 2 でない");
    // 1 つ目はテンプレートの vcs=5 を継承、channel-id=a / role=sendonly
    // 2 つ目は instances[1] の vcs=10 で上書き、channel-id=b / role=recvonly
}

#[test]
fn parse_args_from_argv_three_layer_override() {
    // テンプレート (instance_argvs[0] 先頭) で vcs=10
    // CLI 由来 (instance_cli_argv 末尾) で vcs=20
    // 期待: 連結順序が [program_name, ...instance_argvs[0], ...instance_cli_argv] なので
    //       noargs 後勝ちで InstanceArgs.vcs = 20
    let common_argv = vec!["--instance-hatch-rate".into(), "1.0".into()];
    let common_cli_argv = vec![];
    let instance_argvs = vec![vec![
        "--sora-signaling-url".into(), "wss://example.com/".into(),
        "--sora-channel-id".into(), "ch".into(),
        "--sora-role".into(), "sendonly".into(),
        "--vcs".into(), "10".into(),
    ]];
    let instance_cli_argv = vec!["--vcs".into(), "20".into()];
    let (_common, instances) = parse_args_from_argv(
        "zakuro", common_argv, common_cli_argv, instance_argvs, instance_cli_argv,
    ).expect("有効な argv のパースに失敗してはならない");
    assert_eq!(instances[0].vcs, 20, "CLI 側の vcs=20 がテンプレートの vcs=10 を上書きできていない");
}
```

検証項目 (xorshift32 seed 後方互換):

- `data_channel.rs` の seed 計算を pub(crate) 関数として切り出し、`compute_seed(instance_id=0, vc_id=K)` が現行式 `0xDEAD_BEEF ^ K.wrapping_mul(2654435761)` と一致することを `vc_id ∈ 0..=1000` (per-instance バリデーション範囲) の全域 assert する (網羅 1001 ケース。state=0 回避後の値も含めて一致を確認)
- `compute_seed(instance_id, vc_id) != 0` を `vcs <= 1000` バリデーション範囲の代表値 (境界値 + ランダム数件) について assert する (state=0 回避ロジックの動作確認)

PBT は本 issue では採用しない (検証対象が決定論的な変換で、列挙的テストで主要分岐を網羅可能)。

手動 E2E:

- 2 インスタンス JSONC (`instance-hatch-rate=0.5`、instance0 = sendonly 2vc、instance1 = recvonly 5vc) で起動し、`"Starting zakuro instance 1 at +{:.2}s"` の値が 2.0 ± 0.2s 以内であることを確認
- ログプレフィックスが `[i0/vc-*]`, `[i1/vc-*]` で区別され、両者が同時に存在することを確認
- `instances` キーが無い JSONC で従来通り動作することを確認
- CLI 単独起動で従来通り動作することを確認
- 起動 hatch sleep 中の Ctrl+C で未起動 instance が起動せず、全 instance がシャットダウンすることを確認
- 1 instance を意図的に起動失敗させたとき (無効な signaling URL を指定) 他 instance が継続することを確認

## 関連 issue

- 0005 (add-duckdb-stats-writer): インスタンス別 stats の DuckDB 書き出しは本 issue 完了後に対応。`StatsEvent.instance_id` を活用する
- 0006 (add-rpc-query-method): JSON-RPC `Query` メソッドで DuckDB 任意 SQL クエリを実行する。本 issue で `instance_id` を持つため、`Query` 経由で `WHERE instance_id=...` の SQL を投げれば instance 別集計が可能
- 0008 (add-ui-proxy): HTTP サーバーが全 instance 共有である前提に加え、本 issue 後は `instance_id` 単位のフィルタリング・サマリ API が必要になる前提を共有する
- 本 issue 完了後に CLI 引数を追加する後続 issue (`is_common_key()` の更新と `CommonArgs` / `InstanceArgs` への分類が必要): 0010 (fixed-resolution → InstanceArgs)、0011 (fake-audio-generate → InstanceArgs)、0012 (wav-audio-capture → InstanceArgs)、0015 (degradation-preference → InstanceArgs)、0018 (log-level → CommonArgs)
- 別 issue として新規予約 (本 issue 完了後に必要性を確認したうえで起票、SEQUENCE は起票時に消費):
  - `instance-num` キーによる同一設定 N 複製対応
  - `${}` 環境変数置換 (`ConvertEnv` 互換) 対応
  - `name` キーによる instance 名指定対応 (ログプレフィックスを `[{instance_name}/vc-{vc_id}]` 形式に切り替える)
