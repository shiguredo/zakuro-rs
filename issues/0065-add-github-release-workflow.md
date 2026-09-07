# GitHub Releases によるバイナリ配布ワークフローを追加する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-github-release-workflow
- Polished: {YYYY-MM-DD}

## 目的

zakuro は負荷試験ツールであり、利用者は Rust のビルド環境を持たずにバイナリを入手したい。現状はビルド方法しかしらず、タグもリリースも 1 つも存在しない。配布の形を作ることが OSS 公開の実用的な出発点になる。

## 現状

### 配布経路がゼロ

- `.github/workflows/` は `ci.yml` の 1 本のみ。タグを push しても何も走らない
- `git tag` の件数は 0、GitHub Releases も 0 件
- `origin/master` は `origin/develop` より 168 コミット遅延で、リリースのためのマージが一度も起きていない
- 同じ組織の公開リポジトリ `shiguredo/hisui`、`shiguredo/sora-rust-sdk`、`shiguredo/webrtc-rs` はいずれも `release.yml` を持ち、canary タグを含む GitHub Releases を発行している (`sora-rust-sdk` は `2026.2.0-canary.3` までリリース済み)

### `canary.py` による手動リリースに紐づいている

- `canary.py` が対話確認のうえで `git tag` と `git push` を行う設計だが、このスクリプト自体が壊れている (`issues/0066`)。tag push の先である配布ワークフローが存在しないため、直しても配布は起きない

### ネイティブライブラリのリンク構成が配布の障害になる

事実は `~/.cargo/registry` 内の各 build script を読んで確認した。

| 依存 | リンク方法 | 配布時の影響 |
|---|---|---|
| DuckDB (`Cargo.toml` の `duckdb` を `default-features = false` で使用) | `libduckdb-sys` の build script が `duckdb/duckdb` のリリース (v1.5.5 相当) から **共有ライブラリ** をダウンロードし、`dylib=duckdb` でリンク。rpath にはビルドツリー内の絶対パス (`target/duckdb-download/<target>/<version>`) が出る | 素の実行ファイルでは実行時に `libduckdb` 解決に失敗する。同梱と `$ORIGIN` / `@executable_path` 系 rpath への組み替え、または `DUCKDB_LIB_DIR` を使った配置設計が必要 |
| libwebrtc (`shiguredo_webrtc`) | build script が prebuilt の**静的ライブラリ**を SHA256 検証付きでダウンロードして静的リンク | 実行時依存なし。ただし上流リリースアセットの公開継続に依存 |
| Opus (`shiguredo_opus`) | prebuilt 静的ライブラリをダウンロードして静的リンク | 実行時依存なし |
| OpenH264 / FDK-AAC (`shiguredo_openh264` / `shiguredo_fdk_aac`) | ビルド時はリンクせず、実行時にユーザー指定パスの共有ライブラリをロード | 同梱しない。配布物と README で「ユーザーが用意する」ことを明記する (`issues/0060`) |

### `cargo install` が成立しない見込み

- `[env] DUCKDB_DOWNLOAD_LIB = "1"` は `.cargo/config.toml` に書かれており、cargo はリポジトリ内の config を CWD 起点で探索する。パッケージ内の config は読まれないため、リポジトリ外から `cargo install --git` すると環境変数が渡らず DuckDB 探索に落ちる
- README は `cargo build` によるソースビルドしか案内していない (`## ビルド` セクション)

### サポートターゲットは上流依存が打ち返してこない

- `shiguredo_webrtc` の build script は Linux のディストリビューションを `/etc/os-release` から検出し、ubuntu 22.04 / 24.04 / 26.04 と raspberry-pi-os 以外では panic する (`WEBRTC_C_TARGET` で明示回避できるが README に記載が無い)
- macOS は arm64 のみで、リリースアセット一覧に `macos_x86_64` は存在しない。**Intel Mac 向け配布は上流アセットが出るまで不可**
- DuckDB の prebuilt は osx-universal / linux-amd64 / linux-arm64 / windows-amd64 / windows-arm64 が揃っている
- `rust-toolchain.toml` の `targets` は Linux 2 種のみのため、配布マトリクスの拡張にはターゲット追加が要る (`issues/0064`)

### その他の不足

- `Cargo.toml` に `[profile.release]` が無く、配布物サイズの最適化 (lto / strip / codegen-units) が未設計
- `Cargo.toml` の `[package]` は `publish = false`。crates.io 公開を併用するかの方針決定が要る (`sora-rust-sdk` の `release.yml` には publish job がある。 zakuro は bin のみで配布価値が薄く、`issues/0071` まで進まないと再利用されにくい)
- canary 依存 (`shiguredo_webrtc 0.152.1-canary.1`、`sora_sdk 2026.2.0-canary.1`) のままでも canary タグのリリースは発行できる (上流が同形式でリリース済み)。GA 依存の待ち合わせを配布開始の前提にしない

## 設計方針

1. まず配布方針を 1 文で決めて README と Issue に残す。推奨は「GitHub Releases で、実行時共有ライブラリを同梱した tar.gz / zip を配る。`cargo install` と crates.io は当面やらない」
2. 配布マトリクスを現実的な範囲で確定する。上流アセットの実在から少なくとも次の 3 つは発行可能
   - `x86_64-unknown-linux-gnu` (ubuntu ランナー)
   - `aarch64-unknown-linux-gnu` (`ubuntu-24.04-arm` 系ランナー、または `WEBRTC_C_TARGET` 明示)
   - `aarch64-apple-darwin` (`macos-15` / `macos-26` ランナー。CI matrix でコメントアウトされている `macos-26` を復活する形で検討、`issues/0063` 参照)
   - Windows (`x86_64-pc-windows-msvc`) は上流アセットはあるが CI に Windows job が一切無い。検証 job を先に作ってから配布へ入れる
3. `release.yml` は `sora-rust-sdk` の構成を踏襲する。`on: push: tags:` を起点に、`github-release` job でリリースを作成し (`permissions: contents: write`)、ビルド job の matrix でアーカイブを `gh release upload` する。最後に `slack_notify` を続ける
4. DuckDB の実行時リンクは、アーカイブへ `libduckdb` 共有ライブラリを同梱し、リンク時に `$ORIGIN` (macOS は `@executable_path/../lib`) を rpath として埋め込む方向で設計する。`DUCKDB_LIB_DIR` を使う方が再現しやすいならそちらを選ぶ。いずれにせよビルド後 `otool -L` / `ldd` で解決先を実測し、別マシンで動くことを確認して記録する
5. アーカイブは zakuro 実行ファイル単体ではなく、README のインストール手順が成立する形にする (例: `bin/zakuro` と同梱ライブラリ、`LICENSE`、`THIRD_PARTY_LICENSES.md`)
6. SHA256 チェックサムファイルをリリースに含める (`shiguredo_webrtc` のビルドが SHA256 検証を使うのと同じ水準の利用者向け検証を可能にする)
7. `Cargo.toml` に `[profile.release]` を追加する (`lto`、`strip`、`codegen-units`)。サイズとビルド時間を実測して決める
8. リリース手順を `README.md` もしくは `docs/` に書き、`canary.py` の役割を決める (ワークフローへ移すなら削除、`issues/0066` と連携)

## 完了条件

- タグ push を起点に GitHub Releases が発行され、実行ファイルが入手できること
- ダウンロードした利用者が、追加のビルド作業なしに zakuro を起動できること (`--help` と DuckDB 出力を伴う最小動作を実機で確認する)
- DuckDB の共有ライブラリ依存が、ビルドツリーの絶対パスに依存していないこと (`ldd` / `otool -L` の出力で確認)
- 配布アーカイブに SHA256 チェックサムが付いていること
- README のインストール手順が、実際に配布されたアーカイリで成立していること
- 上流リリースアセットの実在で裏付けられていないターゲット (Intel Mac など) を配布対象に書いていないこと

## 変更対象

- `.github/workflows/release.yml` (新規)
- `Cargo.toml` の `[profile.release]`
- ビルド・リンク設定 (rpath 組み替えに必要な部分)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
