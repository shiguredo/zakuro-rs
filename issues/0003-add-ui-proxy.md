# UI リバースプロキシ機能を追加する

Created: 2026-03-27
Milestone: 2026.1.0

## 概要

`--ui` オプションを指定すると、HTTP サーバーが Zakuro UI のリバースプロキシとして動作し、ブラウザから `http://<host>:<port>/` にアクセスすると Zakuro UI が表示されるようにする。`--ui-remote-url` でリモート URL を変更可能にする。

## 根拠

zakuro (C++) では PR #75 で UI リバースプロキシ (`HttpProxy`) が追加されている。Zakuro UI をブラウザから利用できるようにすることで、負荷試験の状態をリアルタイムに可視化できる。開発時には `--ui-remote-url` でローカルの開発サーバーを指定できる。C++ 版との機能互換性を維持するために対応する。

なお C++ 版の UI リバースプロキシは zakuro では revert 済みで、zakuro-rs で実装する。C++ 版で見つかった以下の問題を再発させないこと。

- TLS サーバー証明書検証が無効なままだった (`shiguredo/zakuro` の `issues/pending/0009-bug-http-proxy-tls-verify-disabled.md`)
- URL パースが `user:pass@host` / IPv6 リテラル / fragment を扱えず、resolve を含む総合タイムアウト・レスポンスの body_limit・TLS 設定の共有にも問題があった (`shiguredo/zakuro` の `issues/pending/0028-bug-http-proxy-hardening.md`)

## 参考

- https://github.com/shiguredo/zakuro/pull/75
- `shiguredo/zakuro` の `issues/pending/0009-bug-http-proxy-tls-verify-disabled.md` (zakuro では対応せず pending)
- `shiguredo/zakuro` の `issues/pending/0028-bug-http-proxy-hardening.md` (zakuro では対応せず pending)

## 対応内容

### 1. HTTP プロキシモジュール

- リモート URL へのリクエスト転送を実装する
- HTTPS (TLS 1.2/1.3) のリモート URL に対応する
- HTTPS では TLS サーバー証明書検証を必ず有効にする (verify_peer + ホスト名検証)
  - 自己署名・期限切れ・ホスト名不一致の証明書はハンドシェイクを失敗させ 502 を返す
  - システム CA を利用する (macOS は `/etc/ssl/cert.pem`、Ubuntu は `/etc/ssl/certs`)
- TLS 設定はリクエストごとに生成せず共有する
- リクエストヘッダーの転送 (Host ヘッダーの書き換え)
- レスポンスの中継
- プロキシ処理は非同期で行い、イベントループをブロックしない
- タイムアウトは段階ごとにリセットせず、resolve を含む全体の deadline にする (例: 30 秒)
- レスポンスの body limit を明示的に設定する (例: 10MB)
  - 超過時は OOM せず明示的なエラーとして返す
- URL のパースは userinfo (`user:pass@host`)、IPv6 リテラル (`[::1]`)、fragment を安全に扱う
  - 安全に扱えない URL は resolve に流す前に明示的なエラーにする

### 2. コマンドライン引数

- `--ui` フラグを追加する (UI 機能の有効化)
- `--ui-remote-url <URL>` オプションを追加する (デフォルトの Zakuro UI URL を上書き)
  - `--ui-remote-url` は `--ui` と併用必須にする

### 3. HTTP サーバーとの統合

- `--ui` が有効な場合、`/rpc` と `/.ok` 以外のリクエストをリモート URL に転送する
- `--http-host` と `--http-port` が必要 (HTTP サーバーが前提)

## 依存

- 0002-add-http-server
