# しきい値判定の違反を起動エラーと別の終了コードにする

- Created: 2026-10-09
- Completed: {YYYY-MM-DD}
- Branch: feature/change-threshold-exit-code
- Polished: {YYYY-MM-DD}

## 目的

「しきい値を満たさなかった」という試験の結果と、「起動・設定に失敗した」というエラーを、
終了コードで区別できるようにする。

いまは両方とも終了コード 1 になるため、CI や systemd から見ると「試験を実施して不合格
だった」のか「試験を実施できなかった」のかが区別できない。systemd で定期実行している
ときに、しきい値違反で終わった実行がユニットの failed として現れ、起動に失敗していると
誤認した。ユニット側で `SuccessExitStatus=1` を指定して回避することもできない。設定や
起動のエラーも同じ 1 のため、本当のエラーまで成功として扱われてしまう。

## 現状

- `zakuro/src/main.rs` の `main` は `run_load_test` の `Err` を一律 `ExitCode::from(1)` に
  変換する。起動・設定のエラーとしきい値の違反がここに合流する
- しきい値の判定は `zakuro/src/threshold.rs` の `evaluate` が行い、`zakuro/src/main.rs` が
  違反を `Err` に変換して返す
- `zakuro lint` / `zakuro fmt` も失敗を終了コード 1 で返す
- 2 回目の Ctrl+C は `std::process::exit(130)`、正常終了は 0
- README の「しきい値による合否判定」の節も「終了コード 1 で終わります」とだけ書いており、
  起動エラーとの区別には触れていない

## 設計方針

- 負荷試験の終了コードを次のように分ける
  - 0: 正常終了
  - 1: 起動・設定のエラー
  - 2: しきい値の違反
  - 130: 2 回目の Ctrl+C
- しきい値の違反は、負荷試験の起動と実行が完了したうえで出る結果なので、起動・設定の
  エラーとは別の経路として扱う。`main` の戻り値を、しきい値の違反だけを表す値に変換する
- `zakuro lint` / `zakuro fmt` の終了コードは変えない
- README の「しきい値による合否判定」の節と、終了コードに触れている箇所を更新し、
  systemd や CI から見た意味を書く

## 完了条件

- しきい値の違反で終わった場合の終了コードが、起動・設定エラーのそれと異なること
- 起動・設定エラーの終了コードが変わらないこと
- 終了コードの意味が README に記載されていること
- 違反の有無と終了コードの対応が単体テストで検証されること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
