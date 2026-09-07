# README を OSS 公開向けに整える

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/update-readme-publication
- Polished: {YYYY-MM-DD}

## 目的

README は外部ユーザーが最初に読む唯一の入口である。現状は開発者向けの機能一覧と実行例が中心で、公開リポジトリとして必要な「入手方法」「動く環境」「ライセンス」「サポート範囲」が欠けている。

## 現状

- README は日本語のみ。組織の公開リポジトリ `shiguredo/hisui` は英日の両方を含む構成になっており、冒頭に次の要素がある (zakuro-rs には無い)
  - CI / crates.io / License の badge
  - 「About Shiguredo's open source software」として、Discord での議論を経た issue / PR 以外には応答しない旨と、時雨堂 OSS の案内先へのリンク
- **インストール手順が存在しない**。`## 必要環境` と `## ビルド` があり `cargo build` しか案内していない。バイナリ配布が無い現状 (前提: `issues/0065`) では、利用者はビルド環境を自分で用意する必要がある
- ビルドの前提が局所的すぎる
  - DuckDB は `.cargo/config.toml` の `DUCKDB_DOWNLOAD_LIB = "1"` に依存しており、リポジトリを clone せずにビルドする場合に効かないことを説明していない
  - `duckdb = "1.10505"` という版指定が DuckDB 本体のバージョン (1.5.5 相当) をエンコードしたものであることが読み取れない
  - ビルド時に GitHub Releases へ HTTP ダウンロードしに行く依存 (libwebrtc、Opus、libduckdb) があり、オフラインビルドや企業内プロキシの話が記載されていない
  - 非 Ubuntu の Linux ディストリビューションでは `WEBRTC_C_TARGET` の明示設定が必要になるが記載が無い (上流 build script が ubuntu / raspberry-pi-os 以外で失敗する)
  - Intel Mac は上流の prebuilt アセットが存在せずビルド対象外になる見込みで、対応環境の下限が書かれていない
- OpenH264 / FDK-AAC は実行時にユーザーが共有ライブラリを用意する設計だが、`README.md` の `## 注意点` に「パスを指定します」とあるだけで、入手元とバージョン要件が書かれていない
- ライセンスを説明するセクションが無い (`## 注意点` で終わっている)
- CI badge が無く、CI が公開されていることが伝わらない
- `docs/ZAKURO.md` と `docs/DUCKDB.md` への導線はある (README 冒頭)

## 設計方針

1. 冒頭を組織の公開リポジトリ (`hisui`) の構成に寄せる。badge (リリース / License) と「About Shiguredo's open source software」相当のサポート方針の节を、表現を合わせて追加する
2. `## インストール` を追加し、(a) GitHub Releases からのバイナリ入手、(b) clone してのソースビルド、の 2 経路を書く。(a) は `issues/0065` の完了が前提なので、先に (b) の前提条件を明確にしたうえで、(a) は配布が形になったら追記する
3. `## 動作確認済み環境` の表を追加する。CI matrix で実際に通っている os とアーキテクチャだけを「確認済み」として書き、コメントアウトされている arm / macOS は「未確認」として区別する (`shiguredo_webrtc` が対応する ubuntu バージョンの範囲も明記する)
4. 実行時にユーザー側で用意するライブラリ (OpenH264、FDK-AAC) と、同梱で解決するもの (libduckdb) の区別を表にして、それぞれ必要な操作を書く。この区別は `issues/0060` の第三者ライセンス表記と表現を揃える
5. MSRV と `rust-version` の関係を「必要環境」に書く (`issues/0064` と連携)
6. 日本語本文は変更せず、英語版 README を別に置くかは本 issue では判断を保留する (組織の他リポジトリが英日どちらを主体にしているかを確認してから別 issue で扱う)
7. README に issue 番号や内部プロセスへの言及を書かない (`shiguredo-issues` の「issue 番号・issue への言及をソースコードに持ち込まないこと」は README / docs も対象)。配布物の入手先として GitHub Releases の URL を書くのは可

## 完了条件

- README にインストール手順 (バイナリとソースの 2 経路) が存在し、書かれているとおりに実行できること
- 動作確認済み環境と未確認の区別が、CI の実際のマトリクスと一致していること
- 実行時にユーザーが用意するライブラリの要件 (名前・入手方法・指定方法) が読み取れること
- ライセンスを説明するセクションと、License / CI の badge があること
- 冒頭のサポート方針セクションが、組織の既存公開リポジトリと食い違った約束になっていないこと
- README から GitHub の私有リポジトリ名や開発環境の絶対パスが読み取れないこと

## 変更対象

- `README.md`

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
