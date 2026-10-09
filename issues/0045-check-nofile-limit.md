# ファイルディスクリプタの上限を確認して注意を促す

- Created: 2026-10-09
- Completed: {YYYY-MM-DD}
- Branch: feature/check-nofile-limit
- Polished: {YYYY-MM-DD}

## 目的

多数の仮想クライアントを動かすときに必要になるファイルディスクリプタの上限を起動時に
確認して警告する。あわせて README に必要な値を記載する。

## 現状

- zakuro-rs はファイルディスクリプタの上限を確認していない
- 1 接続あたり十数個のファイルディスクリプタを使うため、soft の上限が 1024 のままだと
  58 接続程度で DNS 解決が `Too many open files (os error 24)` で失敗する。利用者から
  見ると「すべてのシグナリング URL への接続に失敗しました」として現れ、原因が
  分かりにくい (実測: 1 -> 20 のチャネルを 5 つ、105 接続を起動したときに 45 接続が失敗。
  上限を 262144 に上げると 105 接続すべて成功した)
- systemd のサービスとして動かす場合は `LimitNOFILE` を指定できるが、SSH で入って手動で
  実行する場合は PAM 経由の既定 (1024:524288) がそのまま効く。`limit.conf` に nofile の
  指定が無いと soft は 1024 のままである
- C++ 版の zakuro は起動時に最小 1024 を確認しており、`docs/ZAKURO.md` にも記載がある

## 設計方針

- 起動時に `RLIMIT_NOFILE` の現在値を取得し、`--vcs` の合計から必要なおおよその値を
  求めて警告ログを出す (動作は止めない)
- 1 接続あたりの使用数は環境 (IPv4 / IPv6、TURN の数など) で変わるため、実測値をもとに
  余裕を持った係数を使う
- README の注意点に、多数の仮想クライアントを動かす場合の設定例 (systemd の
  `LimitNOFILE` と `ulimit -n`) を記載する

## 完了条件

- 上限が不足しているときに警告が出ること
- 上限が足りているときは警告が出ないこと
- README に必要な設定が記載されていること
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
