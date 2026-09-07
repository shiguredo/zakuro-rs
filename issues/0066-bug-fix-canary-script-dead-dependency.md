# canary.py が存在しない依存を指定して失敗する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-canary-script-dead-dependency
- Polished: {YYYY-MM-DD}

## 目的

リリース補助スクリプトが壊れたままだと、配布手順の再現性が担保できない。他のプロジェクトから引き継がれた存在しない依存を指定しており、実行すると必ず失敗する。

## 現状

- `canary.py` の `run_cargo_update` 関数は `cargo update shiguredo_audio_device` を `subprocess.run(..., check=True)` で実行する
- 依存 `shiguredo_audio_device` は `Cargo.toml` にも `Cargo.lock` にも存在しない。実際の依存は `Cargo.toml` の `[dependencies]` にある `shiguredo_video_device` である
- `check=True` のため、バージョン書き換えと対話確認まで進んだ後に例外となってスクリプトは異常終了する
- 同じ関数の docstring とコメントには「cargo update shiguredo_audio_device を実行」と英語・日本語で書かれており、`AGENTS.md` のコメント規約に対して英語 docstring が残っている
- `update_version` 関数は `input()` による対話確認を前提としており、`issues/0065` で配布をワークフローへ移した場合に使えなくなる
- `git_operations_after_build` 関数は検証なしに `git tag`、`git push`、`git push origin <tag>` を実行する
- 同じ組織の `hisui`、`sora-rust-sdk`、`webrtc-rs` はいずれも `canary.py` を追跡しており、削除は組織慣習から外れる。ただしそれらのリポジトリの `release.yml` がタグ起点で配布しているため、スクリプトの役割は version bump と tag push に限定されている

## 設計方針

1. `run_cargo_update` が更新すべき対象を確定させる。本リポジトリで canary 運用している依存は `shiguredo_webrtc` と `sora_sdk` である。`shiguredo_video_device` を更新対象にする意図が無いなら、更新すべき依存名に直すか、関数ごと削除して `cargo update` の判断をリリース手順から外す
2. `Cargo.toml` に実在しない依存名を呼ぶコードを残さない (Don't live with broken windows)。修正ではなく削除で済むなら、その旨を「## 解決方法」に書く
3. 関数名・docstring・コメントが実際の挙動と一致するように直す。コメントは日本語に統一する
4. `issues/0065` でリリースをワークフローへ移すなら、`git tag` / `git push` をスクリプト側で二重に行わないように役割を切り分ける (スクリプトは version bump のみ、tag push はワークフロー、など)
5. 対話確認 (`input()`) を残すかは 4 と同時に決める。自動化するなら `--dry-run` 既存の挙動とあわせて再設計する

## 完了条件

- `canary.py --dry-run` が正常に完了し、存在しない依存を参照しないこと
- 実際のバージョン bump と更新対象が想定どおりであること (dry-run 出力で確認)
- `Cargo.toml` / `Cargo.lock` に存在しない依存名を呼ぶコードが残っていないこと
- 関数内のコメントと docstring が実際の挙動と一致し、コメントが日本語になっていること
- `issues/0065` の配布設計と役割が重複していないこと

## 変更対象

- `canary.py`

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
