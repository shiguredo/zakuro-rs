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
