# THIRD_PARTY_LICENSES.md を追加する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/add-third-party-licenses
- Polished: {YYYY-MM-DD}

## 目的

zakuro はビルド時に外部のネイティブライブラリバイナリをダウンロードしてリンクする構成を持つ。実行ファイルを配布する場合、第三者ライセンスの表記が必須になる。現在は表記の置き場がどこにも無い。

## 現状

- `THIRD_PARTY_LICENSES.md` は存在しない。同じ組織の公開リポジトリ `shiguredo/sora-rust-sdk` と `shiguredo/webrtc-rs` には存在する
- zakuro がビルド・実行時に外部実体を取り込む経路は複数ある
  - DuckDB: `Cargo.toml` の `duckdb` 依存が `default-features = false` で、`.cargo/config.toml` の `[env] DUCKDB_DOWNLOAD_LIB = "1"` により `libduckdb-sys` の build script が prebuilt の**共有ライブラリ**をダウンロードする
  - libwebrtc: `shiguredo_webrtc` の build script が prebuilt の**静的ライブラリ**をダウンロードしてリンクする
  - Opus: `shiguredo_opus` の build script が prebuilt の静的ライブラリをダウンロードしてリンクする
  - OpenH264 / FDK-AAC: `shiguredo_openh264` と `shiguredo_fdk_aac` はビルド時にリンクせず、実行時にユーザーが指定した共有ライブラリをロードする (`README.md` の `--openh264` に関する注意書きに該当の記述がある)
- 「同梱されるもの」と「同梱されず実行時にユーザー環境が用意するもの」の区別が、リポジトリ内のどこにも文書化されていない
- Cargo.lock 由来の大量の間接依存クレートのライセンスを整理済みでなく、ライセンス整合を機械的に確認する仕組み (cargo-deny の `deny.toml` 等) も無い

## 設計方針

1. `sora-rust-sdk` の `THIRD_PARTY_LICENSES.md` を読み、章立て・粒度・生成方法を踏襲する (手書きか `cargo-about` 等の生成かを先に決める)
2. 次の 3 層を分けて記述する
   - 実行ファイルに静的にリンクされるもの (libwebrtc、Opus)
   - 配布物として同梱される共有ライブラリ (DuckDB。`issues/0065` の配布設計と整合させる)
   - 同梱されず実行時にユーザー環境が用意するもの (OpenH264、FDK-AAC)。 zakuro 側は同梱しないことを明記し、利用側のライセンス責任であることを書く
3. 依存クレート一覧の手書き維持は困難なため、生成物をコミットする運用にする (再生成手順を同じファイル内に書く)
4. ライセンス不整合を検出する仕組み (cargo-deny の licenses job など) を入れるかは別 issue とし、本 issue は表記の存在までとする

## 完了条件

- リポジトリ直下に `THIRD_PARTY_LICENSES.md` が追跡されていること
- 静的リンクされるライブラリ / 同梱される共有ライブラリ / 同梱されない実行時ロードライブラリの 3 層が区別して書かれていること
- 記述が実際の依存構成 (`Cargo.toml` と `.cargo/config.toml` の DuckDB 設定、各 build script の振る舞い) と一致していること
- 第三者ライセンス本文が、対応する配布アーカイブにも同梱される方針が `issues/0065` と食い違っていないこと

## 変更対象

- `THIRD_PARTY_LICENSES.md` (新規)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
