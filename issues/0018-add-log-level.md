# ログレベル制御 (`--log-level`) を追加する

- Priority: Medium
- Created: 2026-03-27
- Model: Opus 4.6
- Branch: feature/add-log-level
- Polished: 2026-07-14

## 目的

`--log-level` オプションを追加し、libwebrtc 経由のログ出力閾値を実行時に制御できるようにする。

zakuro (C++) は `--log-level {verbose,info,warning,error,none}` で `webrtc::LogMessage::LogToDebug` の閾値を切り替えられる。zakuro-rs は現状 `log::log_to_debug(log::Severity::Info)` がハードコードされており、デバッグ時の詳細ログ取得や、大量 VC 起動時の stderr 抑制ができない。C++ 版と同じ選択肢を提供し、運用時のログ量を制御できるようにする。

## 優先度根拠

Medium。

- C++ 版互換性ギャップ。`docs/ZAKURO.md:277` で未対応と明記されている
- 負荷試験で多数の VirtualClient を起動すると Info ログが stderr を埋め尽くすことがあり、`--log-level warning` / `error` / `none` による抑制が実務で必要
- `--log-level verbose` は接続・コーデック周りの切り分けに有用
- 代替手段 (ソースを一時改変して再ビルド) でも目的は達成できるため、即時必須ではない

## 現状

### ログ初期化

`src/main.rs:117-119` でログを初期化し、`:121` で `parse_args()` を呼ぶ (順序はこのとおり)。

```rust
log::log_to_debug(log::Severity::Info);
log::enable_timestamps();
log::enable_threads();

let (common, instance_args_vec, config_path) = args::parse_args()?;
```

`log` は Rust の `log` クレートではなく、`shiguredo_webrtc::{log, ...}` が指す webrtc-rs のモジュール (`webrtc-rs/src/rtc_base/logging.rs`) である。`Cargo.toml` に `log` / `env_logger` 依存は無い。

利用可能な API:

| API | 役割 |
|-----|------|
| `log::Severity::{Verbose, Info, Warning, Error, None}` | 閾値列挙子 (`Raw(i32)` は本 issue では設定しない) |
| `log::log_to_debug(Severity)` | C++ の `LogMessage::LogToDebug` 相当 |
| `log::enable_timestamps()` / `log::enable_threads()` | タイムスタンプ・スレッド名表示 (本 issue では現状維持) |

`Severity` は `Debug, Clone, Copy, PartialEq, Eq` のみ。`Display` / `as_str` は無い。

### 引数パース

`src/args.rs` の `CommonArgs` (`:10-24`) に `log_level` フィールドは無い。`is_common_key()` (`:83-97`) にも `"log-level"` は含まれない。`args.rs:2` の import は `VideoCodecType` とログマクロのみで、`log` モジュールは未 import。

`parse_common_args()` / `parse_jsonc_config()` / `split_cli_argv()` はいずれも `is_common_key()` に依存するため、キー未登録のまま CLI / JSONC に `--log-level` / `"log-level"` を書くと InstanceArgs 側へ誤振り分けされる。

`--version` は `parse_args` 内で `rtc_log_info!` の直後に `exit(0)` する短絡経路があり、`CommonArgs` 確定前に終了する (`args.rs:1208-1210` 付近)。

### C++ 版の挙動 (参照)

| 項目 | C++ (`zakuro`) | zakuro-rs 現状 |
|------|----------------|----------------|
| CLI | `--log-level` (`util.cpp:64-67`)。`ignore_case`。README は数値 `0..4` も記載 | 未実装 |
| JSONC | 最上位 `"log-level"` を common_args に載せる (`main.cpp:97-100`) | 未実装 |
| 適用 | パース完了後に `LogToDebug` → timestamps → threads (`main.cpp:178-180`) | パース前に `Info` → timestamps → threads |
| デフォルト | `webrtc::LS_NONE` (`main.cpp:75`) | `Severity::Info` |
| ファイルログ | `FileRotatingLogSink` を常時追加 (`main.cpp:182-190`) | 無し |

### ドキュメント

`docs/ZAKURO.md:175` に CLI 一覧として `--log-level {verbose,info,warning,error,none}` が既に記載されている (隣接オプションと異なり説明文は空)。実装状況チェック (`:277`) は `[ ]` のまま。

## 設計方針

### 1. スコープ

本 issue の対象:

- CLI `--log-level` と JSONC 最上位 `"log-level"` の受付
- `CommonArgs` 経由での値保持
- `log::log_to_debug` への反映
- `common_json` への `"log_level"` 出力 (CommonArgs 追加に伴う付随)
- `docs/ZAKURO.md:277` の実装状況チェック更新

本 issue の対象外:

- `FileRotatingLogSink` 相当のファイルログ (webrtc-rs に同等 API が無い)
- `enable_timestamps` / `enable_threads` の変更・呼び出し順の変更
- ログフォーマット・メッセージ文言の変更
- Rust `log` クレート / `env_logger` の導入
- `docs/ZAKURO.md:175` への説明文追記 (チェック更新のみ。デフォルト値は `--help` の `.doc` で示す)

### 2. 作業ブランチ

`Branch` 欄の `feature/add-log-level` は shiguredo-git / create-issue テンプレート上の論理名である。`CODEBASE.md` によりバージョン `2026.0.0` の間はブランチを切らず `develop` 直で実装する。`CHANGES.md` も同期間は更新しない。

### 3. 文字列 → `Severity` 対応

| CLI / JSONC 値 | `log::Severity` |
|----------------|-----------------|
| `verbose` | `Verbose` |
| `info` | `Info` |
| `warning` | `Warning` |
| `error` | `Error` |
| `none` | `None` |

受け付けは小文字の上記 5 値のみとする。C++ の `ignore_case` と数値 `0..4` は意図的に非対応とする (zakuro-rs の他 enum 系オプションと同様、小文字厳密)。JSONC に数値 (`"log-level": 2`) を書いた場合も `push_kv` 経由で `"2"` になり、同様に拒否する。

パースは `match` で列挙値を拒否する既存パターンに倣い、デフォルト付きで次の形にする。

```rust
// args.rs に `use shiguredo_webrtc::{log, ...}` を追加する
let log_level: log::Severity = noargs::opt("log-level")
    .doc("ログレベル (verbose/info/warning/error/none, デフォルト: info)")
    .take(&mut args)
    .present_and_then(|o| match o.value() {
        "verbose" => Ok(log::Severity::Verbose),
        "info" => Ok(log::Severity::Info),
        "warning" => Ok(log::Severity::Warning),
        "error" => Ok(log::Severity::Error),
        "none" => Ok(log::Severity::None),
        _ => Err("log-level は verbose/info/warning/error/none で指定してください"),
    })?
    .unwrap_or(log::Severity::Info);
```

### 4. デフォルト値

デフォルトは `info` (`Severity::Info`) とする。

- 理由: 現行ハードコードが `Info` であり、未指定時のログ量を変えない (後方互換)
- C++ の未指定デフォルト `none` とは意図的に異なる。本 issue で言う「C++ 互換」は選択肢の集合とオプション名・JSONC キーの互換であり、デフォルト値の一致までは含まない

### 5. `CommonArgs` への追加

`log_level: log::Severity` を `CommonArgs` に追加する (プロセス全体で 1 つ。closed `0021` の CommonArgs 分類に従う)。

同時に更新する箇所:

| 箇所 | 内容 |
|------|------|
| `is_common_key()` | `"log-level"` を追加 |
| `parse_common_args()` | 上記 `noargs::opt`。`CommonArgs { ... }` リテラル 2 箇所 (help_mode 早期 return `:679` 付近、通常成功 `:711` 付近) も更新 |
| `src/main.rs` | パース結果で `log_to_debug` を呼び直す (次節) |
| `src/duckdb_stats/stats_json.rs` の `common_json` | `"log_level"` を CLI と同形の小文字文字列で出力 (次節)。テスト内の `CommonArgs { ... }` リテラルも更新 |

`is_flag()` への追加は不要 (`--log-level` は値付きオプション)。

JSONC:

- 最上位 `"log-level": "warning"` は `is_common_key` 経由で `common_argv` に入る (既存経路)
- `instances[i]` 内の `"log-level"` は既存の common キー禁止エラーになる (追加コード不要、テストで確認)

CLI と JSONC のマージは既存の「JSONC 先・CLI 後勝ち」(`parse_args_from_argv`) に任せる。

### 6. `Severity` → 小文字文字列 (`common_json`)

`Severity` に `Display` は無い。`Debug` (`Info` 等の PascalCase) は使わない。`scenario` の match 先例 (`stats_json.rs:658-661` 付近) に倣い、次のヘルパー (または同等のインライン match) を置く。

```rust
fn severity_as_str(s: log::Severity) -> &'static str {
    match s {
        log::Severity::Verbose => "verbose",
        log::Severity::Info => "info",
        log::Severity::Warning => "warning",
        log::Severity::Error => "error",
        log::Severity::None => "none",
        // CLI / JSONC からは Raw を設定しない
        log::Severity::Raw(_) => unreachable!("log_level must not be Severity::Raw"),
    }
}
```

`common_json` では `f.member("log_level", severity_as_str(c.log_level))?` とする。MaskedJson は不要。

### 7. ログ初期化の順序

現行の呼び出し順 (`log_to_debug` → `enable_timestamps` → `enable_threads`) は維持する。変更するのは「パース成功後の再設定」のみ。

1. `log_to_debug(Severity::Info)` を仮設定 (パース中の `rtc_log_warning!` / `rtc_log_info!` を現行どおり見せるため。仮設定は常に `Info` 固定であり、CLI で `verbose` を指定してもパース完了までは Verbose にはならない)
2. `enable_timestamps()` / `enable_threads()` (現状どおり)
3. `parse_args()` 成功直後、かつ続く起動ログ (`rtc_log_info!("zakuro: instances=...")`、`main.rs:126` 付近) より前に `log_to_debug(common.log_level)` で上書きする

短絡経路:

- パース失敗時: プロセス終了のため、仮設定の Info のままでよい
- `--version`: CommonArgs 確定前に `exit(0)` するため、`--log-level none --version` でも version 行は Info 仮設定で出力される (許容する)

### 8. 後方互換

- `CommonArgs` へのフィールド追加により、構造体リテラルを手書きしている箇所 (`stats_json` のテスト等) はコンパイルエラーになる。それらを同時に直す

## 完了条件

- CLI / JSONC で `--log-level` / `"log-level"` が受理され、設計方針 3 の対応表どおりの `Severity` が入ること (未指定は `Info`)
- 不正値 (例: `debug`、空文字、`INFO`、`0`、JSONC 数値) で起動が拒否され、エラーメッセージが正確に `log-level は verbose/info/warning/error/none で指定してください` であること
- `instances[i]` 内の `"log-level"` が common キー禁止エラーになること
- `parse_args()` 成功直後かつ `rtc_log_info!("zakuro: instances=...")` より前に `log::log_to_debug(common.log_level)` が呼ばれること (自動テスト対象外。コードレビューと手動確認で足りる)
- `common_json` の `"log_level"` が CLI と同形の小文字 (`"info"` / `"warning"` 等) であること。`Debug` 形式 (`"Info"`) は不可
- `docs/ZAKURO.md:277` のログレベル制御チェックを `[x]` に更新すること
- 下記テストがすべて通ること (テストのログメッセージは日本語)
- `CHANGES.md` は更新しない (`CODEBASE.md` に従う)
- 作業は `develop` 直で行い、feature ブランチは切らない (`CODEBASE.md` に従う)

### 追加するテスト

#### `src/args.rs`

- 未指定 → `assert_eq!(common.log_level, log::Severity::Info)`
- 各値について対応表どおり断言する (`verbose` → `Verbose`、`info` → `Info`、`warning` → `Warning`、`error` → `Error`、`none` → `None`)
- 不正値でエラー (`format!("{err}")` が `log-level は verbose/info/warning/error/none で指定してください` を含むこと)
- `is_common_key("log-level") == true`
- `split_cli_argv` が `--log-level` を common に振る
- JSONC 最上位 `"log-level": "warning"` → `Severity::Warning`
- JSONC `"log-level": 2` (数値) が拒否される
- `instances[0]` 内 `"log-level"` が拒否される

#### `src/duckdb_stats/stats_json.rs`

- `common_json` (または `build_config_json`) の出力に `"log_level":"info"` (デフォルト) が含まれること
- `log_level: Severity::Warning` のとき `"log_level":"warning"` であること (`"Warning"` ではないこと)

`log_to_debug` の副作用は libwebrtc 依存のため自動テスト必須としない。手動確認方針: `--log-level none` で起動し、`zakuro: instances=...` を含む Info 起動ログが出ないことを確認する (再設定が当該ログより前にあることの確認を兼ねる)。

## 解決方法

設計方針に従い、変更予定ファイルを更新する。実装完了後、本節を実際の変更内容で書き換えること。

### 変更予定ファイル

- `src/args.rs`
- `src/main.rs`
- `src/duckdb_stats/stats_json.rs`
- `docs/ZAKURO.md`

## 関連

- closed `0021`: CommonArgs / InstanceArgs 分割の確立。本 issue を「log-level → CommonArgs」後続として言及
