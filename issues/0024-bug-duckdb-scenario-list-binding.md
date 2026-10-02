# zakuro_scenario の sora_signaling_urls が List バインド非対応で INSERT に失敗する

- Created: 2026-10-02
- Completed: 2026-10-02
- Branch: feature/fix-duckdb-scenario-list-binding
- Polished: {YYYY-MM-DD}

## 目的

DuckDB 統計出力の `zakuro_scenario` テーブルに 1 行も書き込まれていない。
起動時に記録するはずのインスタンス設定が欠落し、統計ファイルから「どんな設定で走らせたか」を
追えなくなる。負荷試験の結果を解釈するための必須情報であるため修正する。

## 現状

- `src/duckdb_stats/rows.rs` の `insert_zakuro_scenario` は `sora_signaling_urls` を
  `duckdb::types::Value::List` でバインドしている
- duckdb-rs は List パラメータのバインドに対応しておらず、実行時に
  `binding List parameters is not yet supported` で失敗する
- `src/duckdb_stats/writer.rs` は INSERT の失敗をログに出すだけで処理を継続するため、
  起動は成功し、`zakuro_scenario` が空のまま統計ファイルが作られる
- 実測: MOQ モードで起動し、`zakuro` テーブルには行があるが `zakuro_scenario` は 0 行だった
  (writer のログに `[duckdb] write failed: error=binding List parameters is not yet supported` が出る)
- Sora モードでも同じ経路を通るため、モードに関係なく発生する

## 設計方針

- `sora_signaling_urls` を JSON 配列の文字列としてバインドし、SQL 側で
  `CAST(? AS VARCHAR[])` して `VARCHAR[]` 列へ入れる
- JSON の生成は `nojson` を使う (URL に含まれる記号のエスケープを自前で書かないため)
- 空配列も `[]` として CAST でき、`VARCHAR[]` の空リストになる

## 完了条件

- `insert_zakuro_scenario` が成功し、`zakuro_scenario` に設定行が入ること
- 単体テストで `sora_signaling_urls` が複数 URL・空配列の両方で往復できること
- 実機の DuckDB ファイルで `SELECT * FROM zakuro_scenario` が 1 行以上返ること

## 解決方法

### 原因

`insert_zakuro_scenario` が `sora_signaling_urls` を `duckdb::types::Value::List` で
バインドしていた。duckdb-rs は List パラメータのバインドに対応しておらず、
`binding List parameters is not yet supported` で INSERT が失敗していた。
`writer.rs` は失敗をログに出すだけで継続するため、統計ファイルは作られるが
`zakuro_scenario` が空になっていた。

### 修正

- `sora_signaling_urls` を JSON 配列の文字列としてバインドし、INSERT 文で
  `CAST(? AS VARCHAR[])` して `VARCHAR[]` 列へ入れるようにした
- JSON の生成は `nojson` を使い、URL のエスケープを自前で書かないようにした
- 空配列も `[]` として `VARCHAR[]` の空リストになる

### 確認

- 単体テストを追加した
  - `insert_zakuro_scenario_stores_signaling_urls`: 複数 URL と空配列の両方で 1 行入ること
  - `insert_zakuro_scenario_accepts_empty_signaling_urls`: 空配列でも INSERT が成功すること
- 実機の DuckDB ファイルで `SELECT ... FROM zakuro_scenario` が 1 行返ることを確認した
- 分離後 (`zakuro/src/duckdb_stats/rows.rs`) も `make ci` で上記テストを含めて通ることを
  確認した
