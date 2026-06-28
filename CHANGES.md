# 変更履歴

## develop

- [CHANGE] CLI 引数 `--fake-video-capture` を `--input-y4m` にリネームする
  - @voluntas
- [ADD] DuckDB ファイルへの統計情報出力 (`--duckdb-output-dir` / `--duckdb-interval` / `--no-duckdb-output`) に対応する
  - @voluntas
- [ADD] CLI 引数 `--input-wav` で WAV ファイル (PCM 16bit) を音声入力としてループ再生できるようにする
  - @voluntas
- [ADD] 複数 Zakuro インスタンス起動 (JSONC `instances` 配列と `--instance-hatch-rate`) に対応する
  - @voluntas
- [ADD] DuckDB 統計書き込み層の整合性を改善する (連続エラー停止 / CancellationToken / send エラー検知 / PRIMARY KEY 制約 / インデックス / 型安全化)
  - @voluntas
- [ADD] 引数パースのバリデーションを改善する (空配列検出 / 次トークン検証 / 排他チェック / 環境変数置換エラー)
  - @voluntas
- [FIX] `--input-y4m` で Y4M ファイルを指定しても映像が再生されないバグを修正する
  - @voluntas
- [FIX] FFI 境界越えの Mutex poison による未定義動作リスクを除去する
  - @voluntas
- [FIX] random_range 関数の整数オーバーフローとゼロ除算の潜在バグを修正する
  - @voluntas
- [FIX] Cargo.toml のプロジェクトメタデータと CI 設定の誤りを修正する
  - @voluntas

### misc

- [UPDATE] build.rs の他プロジェクト由来の死にコードを削除する
  - @voluntas
- [UPDATE] duckdb_stats.rs を責務単位でファイル分割する (mod/writer/schema/rows/stats_json)
  - @voluntas
