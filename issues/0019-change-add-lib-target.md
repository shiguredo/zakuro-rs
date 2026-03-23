# lib ターゲットを追加して公開 API を定義する

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/change-add-lib-target
- Polished: {YYYY-MM-DD}

## 目的

 zakuro は bin クレートのみの構成であり、外部から参照できる API が 1 つも存在しない。このため `shiguredo-rust` の規約が要求する PBT と Fuzzing を外部クレートとして用意できず、テストは `src/` 内の `#[cfg(test)]` に閉じている。テスト基盤を増やす前提として、公開範囲を意図的に決める構成変更が必要である。

## 現状

- `Cargo.toml` のパッケージ節は `[[bin]] name = "zakuro" path = "src/main.rs"` のみを宣言する。`[lib]` は無く `src/lib.rs` も存在しない
- 22 個のモジュールはすべて `src/main.rs` 冒頭の `mod` 宣言でぶら下がっている (`args`、`cmd_fmt`、`cmd_lint`、`data_channel`、`diagnostic`、`duckdb_stats`、`error`、`fake_audio_capturer`、`fake_video_capturer`、`http_server`、`json_rpc`、`jsonc_fmt`、`mp4_audio`、`nop_video_decoder`、`openh264_video_codec`、`scenario`、`stats`、`video_codec_capability`、`video_device_capturer`、`virtual_client`、`wav_reader`、`y4m_reader`)
- 実測すると `src/` 配下に `pub` で始まる項目は 1 つも無い。`pub` の出現 199 件はすべて `pub(crate)` であり、**crate の外から見えるシンボルはゼロ**である
- `shiguredo-rust` は「`tests/`・`pbt/`・`fuzz/` のテストは公開 API に対してだけ書くこと」「PBT と Fuzzing は `src/` に書いてはいけない」と定めるが、公開 API が無いので外部テストクレートが物理的に成立しない
- 同規約は「テストのために private な API を feature フラグや `#[doc(hidden)]` 等で無理矢理公開しないこと」も定めるため、公開範囲を設計せず `pub` を撒くことも許されない
- 組織の公開リポジトリでは `shiguredo/hisui` が `pbt/` と `fuzz/` と `tests/` を、`shiguredo/sora-rust-sdk` が `pbt/` と `tests/` と `examples/` を持つ。単一 bin クレートの zakuro だけがこの構成から外れている

## 設計方針

1. `src/lib.rs` を追加し、`src/main.rs` の `mod` 宣言を lib 側へ移す。`main.rs` は lib の公開 API を呼ぶ薄い起動処理に徹する (どの関数を起動側から呼ぶかを先に洗い出す)
2. 公開する範囲は「外部から意味のある契約で提供できるもの」に限定する。候補として妥当性が高いのは純粋な変換・解析の部品であり、例えば次の系統である
   - Y4M と WAV の読み取り (`src/y4m_reader.rs` と `src/wav_reader.rs`)
   - MP4 からの音声トラック抽出とデコード (`src/mp4_audio.rs`)
   - JSONC の検証・整形 (`src/jsonc_fmt.rs`、`src/cmd_lint.rs`、`src/cmd_fmt.rs`)
   - 引数解析とその妥当性 (`src/args.rs`)
   - 映像コーデック能力の構築 (`src/video_codec_capability.rs`)
   - DuckDB 統計のスキーマと行変換 (`src/duckdb_stats/`)
3. 公開しないもの (デバイスキャプチャ、Webrtc 接続、HTTP サーバーの起動など、副作用と環境依存が強いもの) は `pub(crate)` のまま据え置く。公開するか非公開にするかをモジュール単位で決めた表を本 issue に追記する
4. 公開項目には `///` で利用者視点の doc を書く (`shiguredo-rust` のコメント規約)。`cargo doc` で見たときに意味が通る公開面だけを残す判断をする
5. エラー型は公開 API の顔になる。`src/error.rs` の型を公開するか、モジュール固有のエラー型を公開するかを決めてから実装する (後付けで破壊的変更にならないよう、ここは特に慎重に判断する)
6. bin の名前と crate 名が同じ (`zakuro`) なので、lib 参照時の命名衝突を確認する。`use zakuro::...` が書けることを実測で確かめる
7. MSRV と edition は変更しない (`issues/0017` で扱う)。今回の構成変更で `Cargo.toml` に `[lib]` 宣言を足すか暗黙の `src/lib.rs` 検出に任せるかは、明示性を重視して決める

## 完了条件

- `cargo build` と `cargo test` が通過し、既存の CLI 挙動が 1 つも変わらないこと (snapshot テストを含む)
- `src/` 配下に `pub` の公開項目が意図的に存在し、その一覧が docs から確認できること (`cargo doc` を生成して目視確認する)
- 外部クレートから `use zakuro::<公開項目>` が書けること (空のテストクレートを一時作って実測し、成果物はコミットしない)
- 副作用の大きいモジュールが安易に公開されていないこと (方針 3 の判断記録が本 issue に残っていること)
- `cargo clippy --workspace --all-targets -- -D warnings` が通過すること

## 変更対象

- `src/lib.rs` (新規)
- `src/main.rs` (`mod` 宣言と起動処理)
- 各モジュールの可視指定 (`pub(crate)` の見直し)
- `Cargo.toml` (必要になった場合の `[lib]` 宣言)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない。
