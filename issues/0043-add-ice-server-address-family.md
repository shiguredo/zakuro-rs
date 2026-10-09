# ICE サーバーのアドレスファミリを指定できるようにする

- Created: 2026-10-09
- Completed: {YYYY-MM-DD}
- Branch: feature/add-ice-server-address-family
- Polished: {YYYY-MM-DD}

## 目的

Sora から通知される ICE サーバー (TURN) の URL を、IPv4 と IPv6 のどちらかに固定できる
ようにする。

Sora は TURN の URL をホスト名で通知する。ホスト名が A と AAAA の両方を持つ場合、
libwebrtc はローカルネットワークごとに IPv4 と IPv6 の relay 候補を作る。両方の経路で
メディアが届くと、ICE は「最後にデータが届いた方」へ選択を切り替え続けて発振し、パケット
損失と再送が増えたすえに接続が切断されることがある。実際に受信専用の仮想クライアントを
多数動かしているときに、開始 4 分を過ぎたあたりから経路の切替が毎分数千回規模に発散し、
そのあと接続が切断される事象を確認した (IPv4 の relay と IPv6 の relay の間で
`Switching selected connection due to: data received` を繰り返していた)。

## 現状

- `sora_sdk::SoraConnectionBuilder` には `ice_server_url_configurer` があり、offer で通知
  された URL を取捨選択できるが、`zakuro/src/virtual_client.rs` の `build_client` は
  これを使っておらず、通知された URL をそのまま使っている
- アドレスファミリを指定する引数は無い

## 設計方針

- `--sora-ice-address-family <ipv4|ipv6>` を追加する (JSONC は `sora` オブジェクトの
  `ice-address-family`)。未指定の場合は従来どおり通知された URL をそのまま使う
- 指定した場合は、URL のホスト部を指定したファミリのアドレスに解決してリテラルに
  置き換える。指定したファミリのアドレスを持たない URL は使わない
- `turns:` / `stuns:` (TLS) の URL は使わない。ホスト部をアドレスに置き換えると TLS の
  証明書検証 (ホスト名一致) が通らなくなるため
- URL の解析と取捨選択は、解決結果 (アドレスのリスト) を引数で受け取る純粋な関数にして
  単体テストで検証する。ホスト名の解決だけを別の関数に分ける
- 選んだ URL と使わなかった URL は最初の 1 回だけログに出す (接続ごとに出さない)

## 完了条件

- `--sora-ice-address-family ipv4` で、通知された URL のホスト部が IPv4 のリテラルに
  置き換わること
- `ipv6` で IPv6 のリテラル (ブラケット付き) に置き換わること
- 指定したファミリのアドレスを持たない URL と TLS の URL を使わないこと
- 未指定の場合は通知された URL をそのまま使うこと (従来の動作)
- `cargo fmt --all -- --check` が通ること
- `cargo clippy --locked --workspace --all-targets --features fdk-aac -- -D warnings` が通ること
- `cargo test --locked --workspace --features fdk-aac` が通ること
