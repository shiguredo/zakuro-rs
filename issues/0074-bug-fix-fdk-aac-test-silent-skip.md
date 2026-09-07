# AAC デコードテストが共有ライブラリ不在時に成功扱いになる

- Created: 2026-09-07
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-fdk-aac-test-silent-skip
- Polished: {YYYY-MM-DD}

## 目的

実行時に外部共有ライブラリをロードする経路のテストが、ライブラリが無い環境では「何も検証しないまま ok」で終わっている。AAC デコードという zakuro の機能 1 つが、環境によっては検証されていないのに緑に見える状態を解消する。

## 現状

`src/mp4_audio.rs` のテストモジュールに次の構成がある。

- `fdk_aac_lib_available()` が `shiguredo_fdk_aac::FdkAacLibrary::load("libfdk-aac.so.2")` の成否でライブラリの有無を判定する
- `aac_decodes_and_resamples_to_48khz_mono` と `aac_loop_replays_identical_pcm` の 2 テストが、冒頭で `if !fdk_aac_lib_available() { return; }` として早期 return する
- 早期 return は成功扱いなので、`cargo test` の出力は「2 passed」になり、実際には 1 行も検証されていない
- cfg 条件は `all(target_os = "linux", feature = "fdk-aac")` であり、macOS / Windows ではコンパイルすらされない (これは設計上仕方ない)
- Linux で `--features fdk-aac` を付けても、`libfdk-aac.so.2` が無ければ同じく空振りする
- CI は ubuntu ランナーで apt により `libfdk-aac-dev` を導入しているため CI では常に検証される (関数の doc コメントにもその旨が書かれている)。ただし**空振りしたことが CI のログにもテスト結果にも残らない**
- `shiguredo-rust` は「`#[ignore]` を使わないこと」を定める。早期 return は `#[ignore]` を使わないスキップの実装であり、規約の意図 (意図的に無効化することを明示する) と緊張関係にある
- 同じファイル内の他の AAC 系テスト (ロード失敗時のエラー経路を検証する `mp4_audio.rs` のテスト) はライブラリ非存在でも成立するため、影響するのは上記 2 テストのみである

## 設計方針

1. 「検証された」と「検証されなかった」を出力から区別できるようにする。具体的には、ライブラリ非存在時に早期 return する代わりに、スキップである旨をテスト出力へ明示する手段を選ぶ。候補は次の 3 つであり、実装前にどれを採るか決める
   - パニックではなく明示的なメッセージを出力して早期 return する (スキップを可視化するが、依然「ok」に見える)
   - ライブラリ要求を前提条件として `expect` で失敗させる (CI 以外のローカル Linux で赤くなる。開発環境要件を README に書く必要がある)
   - テスト関数の cfg 条件を強め、ライブラリ存在を確かめる helper ごと整理する
2. CI 側で「実際に走ったか」を確かめる手立てを残す。`cargo test --features fdk-aac -- --nocapture` を CI の test job で使うか、`libfdk-aac.so.2` の存在を別ステップで検証してログに残すなどの方針を決める。CI が緑なら AAC が検証されている、と読み取れる状態が目標である
3. 関数の doc コメント (「未導入のローカル Linux 環境では検証をスキップする (CI には導入済みなので CI では常に検証される)」) は、採った方針の挙動と一致させる。コメントと実態のズレを残さない
4. `README.md` の必要環境または動作確認済み環境の記述に、AAC 検証に必要なライブラリ要件を書く (`issues/0068` と表現を揃える)

## 完了条件

- `cargo test --features fdk-aac` を `libfdk-aac.so.2` が無い Linux 環境で実行したときに、スキップであることが出力から読み取れること (手元で実測して出力を本 issue に記録する)
- CI のログから、AAC デコードテストが実際に検証されたかどうかを判断できること
- `fdk_aac_lib_available()` の doc コメントと、実際のスキップ挙動が一致していること
- AAC 検証に必要なライブラリ要件が README から読み取れること
- `#[ignore]` を導入していないこと (`shiguredo-rust` の規約)

## 変更対象

- `src/mp4_audio.rs` のテストモジュール (`fdk_aac_lib_available` と該当 2 テスト)
- `.github/workflows/ci.yml` (CI での確認方法を決めた場合)
- `README.md` (要件を明記する場合)

`CODEBASE.md` の規約 (バージョン 2026.0.0 の間は develop で開発しブランチを切らない) により develop へ直接コミットする。`Branch:` は名目上の名前であり実ブランチは切らない (`issues/0057` と同じ運用)。
