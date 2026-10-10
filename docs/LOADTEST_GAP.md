# 負荷試験ツールの機能ギャップ分析 (zakuro / zakuro-moq)

## 目的

このリポジトリが提供する 2 つの負荷試験ツールについて、「今あるもの」と「今ないもの」を
洗い出す。WebRTC (Sora) と Media over QUIC (MoQ) の両方の視点で整理し、公開されている
負荷試験ツールが備えている機能を参照して、不足している機能とその優先度を明確にする。

## 対象

| ツール | 対象システム | 実装 | 主な依存 |
| --- | --- | --- | --- |
| `zakuro` (`zakuro/`) | Sora WebRTC SFU | libwebrtc + Sora Rust SDK | DuckDB、Raden、OpenH264 |
| `zakuro-moq` (`zakuro-moq/`) | Sora MoQ (Media over QUIC 実装) のリレー | s2n-quic + moqt-rs | なし (libwebrtc / DuckDB を使わない) |

両者が共有する基盤 (統計集約・HTTP サーバー・JSON-RPC) は `zakuro-core/` にある。

参照するのは、同じ領域で公開されている負荷試験ツールである。

| 領域 | ツール |
| --- | --- |
| WebRTC | srs-bench、webrtcperf、jitsi-meet-torture、livekit-cli (`lk perf load-test`)、pion/webrtc-bench、zakuro (C++ 版) |
| MoQ / QUIC | moq-bench、MoQPerfTestClient (moxygen)、moxygen 適合性試験、moq-interop-runner、moq-stats、h2load、quic-go/perf |

汎用の負荷試験ツールは HTTP / WebSocket / gRPC などが中心で、WebRTC と MoQ は対象外である。
この 2 領域こそが zakuro 系の存在理由であり、比較は「公開ツールのどの層を代替し、どの層を
借りるか」という形で行う。

## 参照した機能モデル

公開ツールが共通して備えている機能を、次の軸で整理する。各軸には実際にその機能を持つ
ツールを添える。

### 負荷生成

| 機能 | 内容 | 実装例 |
| --- | --- | --- |
| 閉じたモデル | 同時実行数を固定し、処理の完了が次の開始を律速する | livekit-cli の `--video-publishers` / `--subscribers`、srs-bench の `-sn` / `-nn` |
| 開いたモデル | 単位時間あたりの開始数を固定し、対象の応答時間に影響されない | livekit-cli の `--num-per-second`、moxygen の `--subscriber_ramp` |
| 起動ランプ | 起動を時間軸でずらして急激な集中を避ける | moq-bench の `--startup`、zakuro の `--vcs-hatch-rate` |
| 送信と受信の分離 | 送信側と受信側の数を独立に指定する | srs-bench の `-sn` と `-nn` の分離 |
| 終了条件 | 秒数 / 件数 / 上限時間で止める | zakuro の `--duration`、moxygen の `--duration` |
| 終了時の猶予 | 実行中の処理を完了させてから止める | 調査したツールでは明示的な指定は一般的ではない |

### 計測値

| 種類 | 内容 | 実装例 |
| --- | --- | --- |
| 累積カウンタと率 | 送受信数 / バイト数 / 欠落数と、その区間レート | moq-bench の `send_mbps` / `recv_fps` / `lost_groups` / `loss` |
| 分布 | 平均 / 標準偏差 / パーセンタイル / 最大 | moq-bench の `latency_p50_ms` / `p90` / `p99` / `max`、webrtcperf の count / sum / mean / stddev / 5p / 95p / min / max |
| フェーズ別の時間 | 接続確立を段階に分解した所要時間 | pion/webrtc-bench の signaling / SDP offer / SDP answer / ICE gathering / ICE connection / DTLS handshake |
| 資源 | 負荷生成側と対象側の CPU / メモリ | pion/webrtc-bench の cpuUsage、webrtcperf の System CPU / Memory |
| 時系列 | 後から集計できる粒度のデータ | moq-bench の JSON Lines、pion/webrtc-bench の CSV |

### 合否判定

| 機能 | 内容 | 実装例 |
| --- | --- | --- |
| しきい値 | 集約値への条件式で合否を決める | webrtcperf の `--alert-rules` (length / sum / min / max / mean / p5 / p95 に条件式と評価期間を付ける) |
| 終了コード | 試験の成否をプロセスの終了コードに反映する | srs-bench のパケット数しきい値、moxygen 適合性試験の exit code |
| 期待値の検証 | 受信内容が期待どおりかを 1 件ずつ確かめる | jitsi-meet-torture の PSNR テスト、zakuro-moq の `--verify-payload` |

### 出力

| 機能 | 内容 | 実装例 |
| --- | --- | --- |
| 終了時のサマリ | 集約結果を 1 つの成果物にする | webrtcperf のアラート JSON、jitsi-meet-torture の PSNR 平均ファイル |
| 粒度データ | 実行中のサンプルを逐次書き出す | moq-bench の JSON Lines、pion/webrtc-bench の CSV |
| メトリクス基盤への送出 | 外部の可視化基盤へ送る | webrtcperf の Prometheus Pushgateway |
| 映像品質の出力 | フレーム単位の品質指標 | webrtcperf の VMAF / PSNR、jitsi-meet-torture のフレーム別 PSNR |

### 分散実行と実行中の制御

| 機能 | 内容 | 実装例 |
| --- | --- | --- |
| 分担と集約 | 複数の負荷生成プロセス / マシンで分担し、結果を集約する | webrtcperf の collector / worker、jitsi-meet-torture-rocket の Terraform / Selenium Grid |
| 実行中制御 | 実行中の状態取得 / 停止 / 負荷変更 | 調査した負荷試験ツールでは一般的ではない |

### 拡張性

| 機能 | 内容 | 実装例 |
| --- | --- | --- |
| ワークロードの定義 | 利用者が手順を書ける | webrtcperf のページスクリプトとアクション、moxygen のパラメータ |
| 計測と出力の追加 | 新しいメトリクスや出力先を足せる | webrtcperf のアラート定義、moq-stats の統計配信 |

### 公開ツールが対象にしない領域

| 領域 | 状況 |
| --- | --- |
| WebRTC のプロトコル統計 | ブラウザ実体 (webrtcperf) を介せば取れるが、専用の負荷生成器ではない |
| MoQ | 専用の負荷生成器 (moq-bench / MoQPerfTestClient) が別に存在する |
| パケット / フレーム単位の品質 | 汎用の負荷試験ツールの計測粒度はリクエスト / ストリーム単位まで |

## 現状の機能

### zakuro (WebRTC / Sora)

#### 負荷生成モデル

| 項目 | 現状 |
| --- | --- |
| 仮想クライアント (VC) | `--vcs` で 1-1000。VC 1 本 = Sora への 1 接続 |
| インスタンス | JSONC `instances` 配列で複数。`--instance-hatch-rate` で段階起動 |
| 起動レート | `--vcs-hatch-rate` / `--instance-hatch-rate`。`DelayQueue` で `interval * i` ずつ線形にずらす (zakuro/src/main.rs:556-562、zakuro/src/main.rs:939-945) |
| 持続時間 | `--duration` (秒)。経過で切断し、`--repeat-interval` があれば再接続 |
| リトライ | `--max-retry` / `--retry-interval`。失敗時に VC 単位で再試行 |
| 停止 | Ctrl+C 1 回目で graceful shutdown、2 回目で `exit(130)` (zakuro/src/main.rs:531-539) |

負荷モデルは閉じたモデルだけである。VC 数は起動時に固定され、到着率を一定に保つ開いた
モデル・ランプダウン・処理回数による終了条件・終了時の猶予は持たない。

#### シナリオ

| 項目 | 現状 |
| --- | --- |
| 組み込みシナリオ | `--scenario reconnect` のみ (zakuro/src/scenario.rs:88-93) |
| reconnect の動作 | 接続直後に切断 → 再接続 → `Sleep(1-5 秒)` × 9 → 先頭へ戻るループ |
| 操作種別 | `Sleep` / `Reconnect` は同梱シナリオから到達する。`Disconnect` / `Exit` / `SendDataChannelMessage` は `#[expect(dead_code)]` が付いており、テストからしか構築されない (zakuro/src/scenario.rs:20-59) |
| 利用者定義 | 不可。シナリオは Rust のコードで構築され、外部ファイルやスクリプトでは定義できない |

`docs/ZAKURO.md` は `Disconnect` / `Exit` / `SendDataChannelMessage` を実装済み `[x]` として
いるが、実際に到達するのは `Sleep` と `Reconnect` のみである。DataChannel の送信は
`--sora-data-channels` の定期送信として別経路で実装されている。

#### メディア入力と送受信

| 項目 | 現状 |
| --- | --- |
| 映像 | フェイク映像 (Raden)・砂嵐・Y4M・実デバイス・MP4 パススルー |
| 音声 | フェイク音声 (BIP / BOP / HUM / ノイズ)・WAV・MP4 内の Opus / AAC |
| コーデック | VP8 / VP9 / AV1 / H264 / H265、音声は Opus。エンコーダー実装の個別指定可 |
| ロール | `sendonly` / `recvonly` / `sendrecv` |
| 高度な機能 | サイマルキャスト・スポットライト・DataChannel メッセージング・mTLS |
| DataChannel | `--sora-data-channels` で送信のみ。受信メッセージの処理 (RTT 計測など) は無い (zakuro/src/data_channel.rs:88-125) |

#### メトリクス

| 項目 | 現状 |
| --- | --- |
| 収集対象 | Sora SDK の `get_stats()` が返す RTCStats 一式を DuckDB へ記録 |
| テーブル | RTCStats 由来の 7 テーブル (codec / inbound-rtp / outbound-rtp / media-source / remote-inbound-rtp / remote-outbound-rtp / data-channel) と `connection` / `zakuro` / `zakuro_scenario` |
| 品質系フィールド | `jitter` / `packets_lost` / `nack_count` / `pli_count` / `fir_count` / `freeze_count` / `quality_limitation_*` / `round_trip_time` / 再送系 (zakuro/src/duckdb_schema.sql:101-296) |
| サンプリング | `--duckdb-interval` (既定 1.0 秒) ごとに全 VC 分を書き込み |
| リアルタイム性 | 接続状態 (connected / retrying / stopped) を 5 秒ごとにログ出力 (zakuro-core/src/stats.rs:173-194)。メトリクスの外部公開 API は無い |
| 集計 | 生サンプルの記録のみ。平均・パーセンタイル・最大などの集計は無い |

#### 合否判定と出力

| 項目 | 現状 |
| --- | --- |
| しきい値判定 | 無し。しきい値・期待値検証・中断に相当する実装は存在しない (`rg -i "threshold" zakuro/src` がテストの `assert` 以外に一致しない) |
| 終了コード | 0 = 正常終了、1 = 起動 / 設定エラー、130 = 2 回目 Ctrl+C。負荷試験の合否とは無関係 |
| 構造化出力 | DuckDB ファイルのみ。JSON サマリ / CSV / Prometheus / OTLP / StatsD / InfluxDB は無い |
| サマリ | ログのみ。試験全体のサマリレポートは無い |

#### 制御 API

| 項目 | 現状 |
| --- | --- |
| HTTP API | `--http-host` / `--http-port` 指定時に `GET /.ok` と `POST /rpc` (zakuro-core/src/http_server.rs:53-69) |
| JSON-RPC | `GetVersion` のみ (zakuro-core/src/json_rpc.rs:70-77)。DuckDB への Query メソッドは open issue 0001 |
| 認証 | 無し (zakuro-core/src/http_server.rs:53-69) |
| 実行中制御 | 無し。VC の追加・停止・レート変更・統計取得はできない |

#### 設定と CLI

| 項目 | 現状 |
| --- | --- |
| 設定ファイル | JSONC (`--config`)。`instances` 配列でインスタンスごとの設定が可能 |
| 環境変数の展開 | 未対応。`${...}` を含む値を書くとエラーで起動を拒否する (zakuro/src/args.rs:489-494) |
| 設定の記録 | 全設定を DuckDB の `zakuro.config_json` に保存する (docs/DUCKDB.md:96)。試験条件の再現に使える |
| 検証 / 整形 | `zakuro lint` / `zakuro fmt` (`--check` あり) |
| ログ制御 | `--log-level`、`--log-suppress` (部分一致で抑制) |
| 拡張機構 | 無し。プラグインやスクリプトによるシナリオ / メトリクス拡張はできない |

### zakuro-moq (MoQ)

#### 負荷生成モデル

| 項目 | 現状 |
| --- | --- |
| 仮想クライアント | `--vcs` で 1-1000。VC 1 本 = 1 QUIC 接続 = 1 MOQT セッション |
| インスタンス | JSONC `instances` 配列で 1-64。`--instance-hatch-rate` で段階起動 |
| 起動レート | `--vcs-hatch-rate` / `--instance-hatch-rate`。VC 起動後は本数が増えない |
| 持続時間 | `--duration`。経過でセッションを閉じ、`--repeat-interval` で再接続 |
| リトライ | `--max-retry` (既定 0) / `--retry-interval` (既定 60 秒) |
| 停止 | Ctrl+C 1 回目で FIN / GOAWAY を最大 3 秒待つ graceful shutdown、2 回目で強制終了 |

閉じたモデルのみで、到着率一定の開いたモデル・ランプダウン・終了時の猶予は無い。

#### publish / subscribe

| 項目 | 現状 |
| --- | --- |
| publish | `--tracks 名前[:レート[:サイズ]]`。VC 1 本が全トラックを publish |
| Track Name | `<指定名>-<インスタンス>-<VC>` で一意化 (zakuro-moq/src/moq_client.rs:93-104) |
| Track Alias | セッション内で 1 からトラック順に払い出し |
| Group / Subgroup | Group は約 1 秒分の object で FIN して次へ。Subgroup は Group 内 1 本固定 (`SubgroupIdMode::Zero`) |
| 優先度 | `publisher_priority: None` 固定。CLI / JSONC からの指定は無い (zakuro-moq/src/moq_client/session.rs:372-373) |
| ペーシング | トラックごとに `period = 1.0 / object_rate` で送信。10 object 分を超える遅れは切り捨てて現在時刻から再開 |
| LOC プロパティ | Timescale と Timestamp (送信数 / レートから算出した合成値) を付与 (zakuro-moq/src/moq_client/session.rs:1366-1382) |
| REQUEST_UPDATE | FORWARD=0 で該当トラックの送信を停止、FORWARD=1 で再開 (zakuro-moq/src/moq_client/session.rs:322-334) |
| subscribe | `--subscribe-tracks`。`{instance}` / `{vc}` のプレースホルダで 1 publish に対して N subscribe を構成できる |
| 受信検証 | `--verify-payload` で `位置 % 251` パターンとの一致のみ検査 (既定は無効) |
| 欠落 / 順序 / 遅延 | 計測しない。受信側で Object ID / Group ID を記録していない (zakuro-moq/src/moq_client/session.rs:1049-1083) |
| DATAGRAM | endpoint は有効化しているが、object は subgroup stream のみで送る (zakuro-moq/src/moq_client/transport.rs:298-311) |

#### メトリクス

| 項目 | 現状 |
| --- | --- |
| 計測項目 | `objects-sent` / `send-rate` / `objects-received` / `recv-rate` / `bytes-received` / `payload-mismatches` の累積値と区間レート |
| 出力 | 5 秒ごとのログ (`[stats]` 行) と終了時のログのみ (zakuro-moq/src/main.rs:276-312) |
| トラック別内訳 | 無し。全 publisher / subscription の合計値のみ |
| セッション進捗 | `publish=受理/全, sent-objects=, forwarding=, subscribe=受理/全, received-objects=` を 5 秒ごとにログ出力 |
| レイテンシ | publish から subscribe までの遅延、RTT、jitter の計測は無い |
| QUIC 統計 | s2n-quic の接続メトリクス (RTT / 損失 / cwnd) を読む API 呼び出しは無い |
| 集計 | 累積値と区間レートのみ。平均・パーセンタイル・最大は無い |

#### 合否判定と出力

| 項目 | 現状 |
| --- | --- |
| しきい値判定 | 無し。VC / インスタンスの失敗は warn ログを出して継続する (zakuro-moq/src/main.rs:151-157) |
| 終了コード | 0 = 正常終了、1 = 起動 / 設定エラー、130 = 2 回目 Ctrl+C |
| 構造化出力 | 無し。DuckDB / JSON / CSV / Prometheus / OTLP への出力は無い |
| CI での判定 | `.github/workflows/e2e-test.yml` がログの最終行を grep して往復を確認する (しきい値判定ではない) |

#### 制御 API と設定

| 項目 | 現状 |
| --- | --- |
| HTTP API | `zakuro` と同じ `zakuro-core` の `GET /.ok` / `POST /rpc` (GetVersion のみ) |
| 実行中制御 | 無し |
| 設定ファイル | JSONC (`--config`)。`instances` は最上位をテンプレートとして継承 |
| 環境変数の展開 | 未対応。`${...}` を含む値を書くとエラーになる (zakuro-moq/src/args.rs:888-893) |
| 検証 / 整形 | `zakuro-moq lint` / `zakuro-moq fmt` は無い (README.md:324) |
| TLS | WebPKI / `--ca-cert` / `--insecure`。クライアント証明書 (mTLS) は無い |

### 共通で欠けているもの (両ツール)

| 項目 | 現状 |
| --- | --- |
| 合否判定 | しきい値と期待値検証が無く、試験結果の pass / fail を機械判定できない |
| サマリレポート | 実行結果を 1 つの成果物 (JSON / HTML) として出力できない |
| リアルタイムメトリクス | Prometheus / OTLP などで外部から観測できない |
| 実行中制御 | 実行中の開始 / 停止 / スケール / 状態取得ができない |
| 分散実行 | 複数プロセス / 複数マシンでの負荷配分と結果集約ができない |
| ネットワーク条件 | 帯域制限・パケットロス・遅延 / ジッタ注入ができない |
| 段階的な停止 | 終了時に実行中の処理を完了させる猶予が無く、一斉切断になる |
| 開いたモデル | 毎秒 N 接続を維持する負荷生成ができない |
| レイテンシ計測 | アプリケーション層の end-to-end 遅延を測っていない |
| 拡張機構 | シナリオ / メトリクス / 出力先を利用者が拡張できない |

## 計測項目のチェックリスト

「何が測れて、何が測れていないか」を項目単位で整理する。WebRTC の項目は W3C の
WebRTC Statistics 仕様と既存ツールの実装、MoQ の項目は draft-ietf-moq-transport-22 と
既存の負荷生成器を基準にした。

### WebRTC (zakuro)

| 計測項目 | 状態 | 備考 |
| --- | --- | --- |
| 接続成功率 | 一部 | connected / retrying / stopped の現在値をログに出すのみ。成功率や失敗理由の集計は無い |
| 接続確立時間 (シグナリング / SDP / ICE / DTLS) | 無し | `connection` テーブルは 1 接続 1 行で所要時間を持たない。フェーズ分解は pion/webrtc-bench が参考になる |
| RTT | あり | `rtc_stats_remote_inbound_rtp.round_trip_time` など |
| ICE candidate-pair (選択経路 / availableOutgoingBitrate) | 無し | `candidate-pair` は未対応 type |
| パケットロス / ジッタ | あり | `packets_lost` / `jitter` / `fraction_lost` |
| NACK / PLI / FIR / 再送 | あり | `nack_count` / `pli_count` / `fir_count` / 再送系 |
| freeze / concealment / jitter buffer | あり | `freeze_count` / `concealed_samples` / `jitter_buffer_delay` など |
| quality limitation (CPU / bandwidth) | あり | `quality_limitation_reason` / `quality_limitation_duration_*` |
| 目標ビットレート / エンコード結果 | あり | `target_bitrate` / `frames_encoded` / `qp_sum` など |
| 解像度 / fps / 音声レベル | あり | `rtc_stats_media_source` |
| end-to-end 遅延 | 無し | アプリ層の遅延計測が無い。webrtcperf のウォーターマーク方式が参考になる |
| VMAF / PSNR | 無し | `psnrSum` / `psnrMeasurements` は型非対応。受信映像はデコードせず破棄する |
| 音声の知覚品質 (MOS 相当) | 無し | 音声レベルと concealment 統計のみ |
| DataChannel の送受信量 | 一部 | 送信は `rtc_stats_data_channel` に記録。受信処理と RTT 計測はしない |
| プロセス / クライアント資源 (CPU / メモリ) | 無し | SFU 側の資源は Sora 側の統計と突き合わせる |
| サンプル欠落の可視化 | あり | DuckDB の drop 件数を 5 秒ごとに warn 出力 |

### MoQ (zakuro-moq)

| 計測項目 | 状態 | 備考 |
| --- | --- | --- |
| object の送信数 / レート / サイズ | あり | ログのみ。トラック別の内訳は無い |
| object の受信数 / レート / バイト数 | あり | ログのみ |
| publish から subscribe までの遅延 | 無し | LOC Timestamp は合成値。moq-bench は wall-clock を埋めて p50 / p90 / p99 / max を出している |
| object の欠落 (group 単位) | 無し | Object ID / Group ID を記録していない。moq-bench は sequence span で欠落を数える |
| object の順序 / 重複 | 無し | 同上 |
| payload の一致 | あり | `--verify-payload` (既定は無効) |
| PUBLISH / SUBSCRIBE の受理状況 | あり | 5 秒ごとの進捗ログ |
| REQUEST_UPDATE (FORWARD) への追従 | あり | 動作はする。計測は forwarding 数のみ |
| QUIC の RTT / 損失 / cwnd / stream 数 | 無し | s2n-quic の接続メトリクスを読んでいない |
| ハンドシェイク / セッション確立時間 | 無し | 確立の成否のみログに出る |
| リレー段数 / キャッシュ | 無し | リレー側の機能。負荷試験側からは観測していない |
| 資源 (CPU / メモリ) | 無し | プロセス外の計測が必要 |

## 機能マトリクス

機能軸ごとに、公開ツールに一般に期待される機能と zakuro / zakuro-moq の現状を並べる。

| 機能軸 | 一般に期待される機能 | zakuro (WebRTC) | zakuro-moq (MoQ) |
| --- | --- | --- | --- |
| 負荷生成モデル | 閉じたモデルと開いたモデル (単位時間あたりの開始数) を選べる | 同時接続数を固定 (閉じたモデルのみ) | 同時セッション数を固定 (閉じたモデルのみ) |
| 起動ランプ | 増加カーブと、終了時の段階的な停止 | 起動レートのみ。ランプダウン無し | 起動レートのみ。ランプダウン無し |
| 終了条件 | 秒数 / 件数 / 上限時間 | `--duration` と `--repeat-interval` | `--duration` と `--repeat-interval` |
| ワークロード定義 | 利用者が手順を書ける。複数のワークロードを並列実行できる | 組み込みの `reconnect` のみ | 無し (publish / subscribe の定常動作のみ) |
| 負荷の単位 | 同時実行数と処理 1 回の単位 | VC = Sora への 1 接続 | VC = 1 QUIC 接続 + N トラック |
| 計測値の種類 | 累積カウンタ / 現在値 / 率 / 分布 (パーセンタイル) | RTCStats の生値 (分布の集計なし) | 累積カウンタと区間レート |
| プロトコル統計 | 対象プロトコルの統計 | RTP 統計一式 (最大 76 列) | object 数とバイト数のみ |
| 集計 | 平均 / 標準偏差 / パーセンタイル / 最大 | 無し (生サンプルのみ) | 無し |
| 結果の切り分け | 接続別 / トラック別 / シナリオ別に分ける | DuckDB の `instance_id` / `vc_id` / `connection_id` で切り分け可 | 無し (ログに instance / vc が出るのみ) |
| 合否判定 | 集約値への条件式と終了コード | 無し | 無し |
| サマリ | 終了時の集約レポート | 無し | 無し |
| 出力先 | ファイル / メトリクス基盤 / 標準出力 | DuckDB のみ | ログのみ |
| リアルタイム可視化 | 実行中の進捗とメトリクスの観測 | 5 秒ごとのログ | 5 秒ごとのログ |
| 実行中制御 | 状態取得 / 停止 / 負荷変更 | `GetVersion` のみ | `GetVersion` のみ |
| 分散実行 | 複数プロセス / 複数マシンでの分担と集約 | 1 プロセス内の `instances` のみ | 同左 |
| 拡張機構 | シナリオ / 計測 / 出力の追加 | 無し | 無し |
| 設定 | 設定ファイルとその検証・整形 | JSONC + `lint` / `fmt` | JSONC のみ |
| ネットワーク劣化注入 | 帯域 / 遅延 / 損失の注入 | 無し | 無し |

## WebRTC 視点の不足

WebRTC 負荷試験として見たときに、zakuro に足りていないものを挙げる。

### 接続の品質と失敗の可視化

| 不足 | 内容 | 参考になる機能 |
| --- | --- | --- |
| 接続確立時間 | シグナリング成立と WebRTC 確立は `connection_lifecycle` に分かれて記録される。経路の RTT と可用帯域は `rtc_stats_candidate_pair`、選択中ペアは `rtc_stats_transport` に毎サンプル残る。フェーズごとの所要時間を 1 行にまとめたメトリクスは無い | pion/webrtc-bench のフェーズ分解 (signaling / SDP / ICE gathering / ICE connection / DTLS) |
| 成功 / 失敗の集計 | `StatsCollector` は connected / retrying / stopped の現在値のみを持ち、失敗理由別の累計や接続成功率を出さない (zakuro-core/src/stats.rs:49-62) | srs-bench のパケット数しきい値と終了コード、webrtcperf のアラート出力 |
| 接続チャーン耐性 | 到着率一定で接続と切断を繰り返すモデルが無い。`--repeat-interval` は全 VC が同じタイミングで動く | livekit-cli の `--num-per-second`、moxygen の `--subscriber_ramp` のような開いたモデル |
| 時間軸での集計 | `--duckdb-interval` ごとの生サンプルはあるが、任意区間の p95 などを試験中に出す仕組みが無い | webrtcperf の集約表 (count / sum / mean / stddev / 5p / 95p / min / max) |
| サンプリング欠落 | 統計サンプルのチャネル容量は 8192 tick で、超えた分は接続単位で捨てる。遅延中の同一接続は最新 tick だけを残す。欠落件数は `[duckdb] dropped stats samples` に出る。接続行、ライフサイクル、codec は欠落しない | 欠落件数を結果の指標として扱う (moq-bench は無効サンプルを失敗として扱う) |

### メディア品質の評価

| 不足 | 内容 | 参考になる機能 |
| --- | --- | --- |
| end-to-end 遅延 | 送信フレームに識別情報を埋めて受信側で測る仕組みが無い。受信は `NopVideoDecoder` で捨てるため実デコード時間も測れない | webrtcperf のウォーターマーク方式 / abs-capture-time 方式 |
| 品質指標の導出 | jitter / loss / NACK / PLI / freeze / quality limitation は取れているが、「品質が基準を満たしたか」を判定する層が無い | webrtcperf の `--alert-rules` (p5 / p95 などの集約に条件式) |
| 音声品質 | 音声の品質指標 (MOS 相当、音声の途切れ) を測っていない。concealment 統計はある | webrtcperf の ggwave による e2e 遅延、専用の音声品質計測 |
| 実クライアントとの差 | libwebrtc の統計は取れるが、ブラウザ実体の描画・デコード・jitter buffer の負荷を再現しない | webrtcperf の Chromium 実クライアント方式 |

### 試験の制御と再現性

| 不足 | 内容 | 参考になる機能 |
| --- | --- | --- |
| シナリオの拡張 | `reconnect` 以外のワークロード (解像度変更、mute / unmute、コーデック切替、帯域逼迫時の挙動) を定義できない | webrtcperf のページスクリプトとアクション |
| 帯域・ロス注入 | 帯域制限・パケットロス・遅延 / ジッタを注入できない。libwebrtc の BWE と SFU の劣化挙動を試せない | webrtcperf が内蔵する tc + NetEm |
| 実行中制御 | 試験中に VC を追加・切断・レート変更できない | 実行中制御 API (調査した負荷試験ツールでも一般的ではない) |
| 分散実行 | 1 プロセスの `--vcs` 上限 1000 とエンコード負荷のため、大規模試験は複数プロセス / 複数マシンに分ける必要があるが、分担と集約の仕組みが無い | webrtcperf の collector / worker、jitsi-meet-torture-rocket の Terraform |
| サーバー資源 | 負荷試験中の SFU と負荷生成器の CPU / メモリを記録していない | livekit-cli のベンチマーク、pion/webrtc-bench の cpuUsage |
| 結果の合否 | 試験結果を機械判定できないため、CI で性能回帰を検出できない | webrtcperf の `--alert-rules`、srs-bench の終了コード |
| 設定の外部化 | 環境変数の展開ができず、チャネル ID や資格情報を環境ごとに差し替える手段が CLI 引数と設定ファイルの書き換えしかない | 環境変数と秘匿情報の外部注入 |

## MoQ 視点の不足

MoQ 負荷試験として見たときに、zakuro-moq に足りていないものを挙げる。

### object レベルの計測

| 不足 | 内容 | 参考になる機能 |
| --- | --- | --- |
| publish から subscribe までの遅延 | LOC の Timestamp は「送信数 / レート」から作る合成値で、受信側でも decode していない (zakuro-moq/src/moq_client/session.rs:1366-1382) | moq-bench の片道遅延 (p50 / p90 / p99 / max、clock skew の除外付き) |
| 欠落 / 順序 / 重複 | 受信側で Object ID / Group ID を記録していないため、object の欠落・順序逆転・重複を検出できない (zakuro-moq/src/moq_client/session.rs:1049-1083) | moq-bench の group sequence span による欠落検出 (live frontier を除外) |
| ペイロード長の検証 | 受信 payload 長を送信側の `object-size` と比較していない | 受信内容の期待値検証 |
| トラック別の内訳 | メトリクスは全トラックの合計値のみ。トラック別の送信数 / レート / 遅延が出ない | moq-bench の broadcast 単位の統計 |

### トランスポートとリレーの計測

| 不足 | 内容 | 参考になる機能 |
| --- | --- | --- |
| QUIC 統計 | RTT / 損失 / 輻輳ウィンドウ / bytes in flight / stream 数を読んでいない (zakuro-moq/src/moq_client/transport.rs:222-258 は接続のみ) | h2load の最小 RTT / 平滑化 RTT / 損失宣言数、s2n-quic の接続メトリクス |
| 輻輳制御と 0-RTT | 輻輳制御アルゴリズムの指定、0-RTT の利用、idle timeout の指定ができない | quic-go/perf のハンドシェイク計測、QUIC 実装のパラメータ指定 |
| リレーの経路 | 複数ホップのリレー、キャッシュヒット、同一 object を複数 subscriber が受ける場合の挙動を検証する仕組みが無い | moq-relay の hop リストとデバッグ用 HTTP API、moq-stats |
| プロトコル機能の網羅 | 優先度 (`publisher_priority`) は固定、DATAGRAM は未使用、Subgroup は Group 内 1 本固定、SUBSCRIBE_UPDATE / TRACK_STATUS などは未使用。`--tracks` は固定値のみ | moxygen の転送プリファレンス 4 種、moq-bench の `[min, max]` のレンジ指定 |
| 異常系 | GOAWAY 受信、PUBLISH / SUBSCRIBE の拒否、REQUEST_ERROR などを意図的に起こせない。peer 起点の request も扱わない (zakuro-moq/src/moq_client/session.rs:818-827) | moxygen 適合性試験のケース設計 |

### 試験の制御と出力

| 不足 | 内容 | 参考になる機能 |
| --- | --- | --- |
| 構造化出力 | DuckDB も JSON サマリも無く、5 秒ごとのログのみ | moq-bench の JSON Lines 出力 (`timestamp_ms` + 累積カウンタ)、pion/webrtc-bench の CSV |
| リアルタイムメトリクス | Prometheus / OTLP で外部から観測できない | webrtcperf の Prometheus Pushgateway 送出 |
| 合否判定 | `--verify-payload` の不一致数は出るが、しきい値判定と終了コードが無い。CI はログを grep している | webrtcperf の `--alert-rules`、moxygen 適合性試験の exit code |
| 分散実行 | 1 プロセス内の `instances` のみ | webrtcperf の collector / worker |
| シナリオ | publish / subscribe の定常動作以外を定義できない | moxygen の購読者ランプと配信タイムアウト、moq-bench の負荷形状 |
| 設定の検証 | `zakuro-moq lint` / `fmt` が無い | `zakuro lint` / `fmt` |
| 購読側の負荷モデル | subscribe 専用起動はできるが、購読開始レート・購読数・購読解除の制御は publish と同じ扱い | moxygen の `--subscriber_ramp` / `--subscriber_max` |
| 設定の外部化 | 環境変数の展開ができない (zakuro-moq/src/args.rs:888-893) | 環境変数と秘匿情報の外部注入 |

## 参考ツール

同じ領域のツールが何を提供しているかを整理する。2026-10-07 時点で公開情報を確認できた
ものだけを挙げる。

### WebRTC

| ツール | 提供元 | 何をするか | 参考にできる点 |
| --- | --- | --- | --- |
| zakuro (C++ 版) | 時雨堂 | Sora 向けの負荷試験。zakuro-rs の元実装 | 機能互換の基準。`--output-file-connection-id` による接続単位の結果出力 |
| srs-bench | SRS | Go + pion で publish / play を負荷生成 | 送信数と受信数を別々に指定する設計、パケット数のしきい値と終了コードによる回帰判定、再接続ループでのリーク検査、受信の録画を次の素材にする DVR |
| webrtcperf | Vittorio Palmisano | Chromium を実クライアントとして起動し、RTCPeerConnection 統計を集約 | `--alert-rules` によるしきい値判定 (length / sum / min / max / mean / p5 / p95 に条件式と評価期間を付ける)、Prometheus Pushgateway への送出、ウォーターマークと abs-capture-time による e2e 遅延の実測、VMAF / PSNR、tc + NetEm による劣化注入、collector / worker による分散 |
| jitsi-meet-torture | Jitsi | Selenium による大規模会議テスト | PSNR のフレーム別出力と平均、参加者 / 送信者の個別指定、assert による合否 |
| livekit-cli (`lk perf load-test`) | LiveKit | Go SDK で publisher / subscriber を模擬 | 毎秒起動 (`--num-per-second`)、layout と simulcast の指定、bytes/s / packets/s と CPU 使用率の併記 |
| pion/webrtc-bench | Pion | WebRTC サーバーのベンチ | 接続確立フェーズの分解計測 (signaling / SDP offer / SDP answer / ICE gathering / ICE connection / DTLS handshake)、プロセス CPU 使用率、測定不能サンプルの記録 |

### MoQ

| ツール | 提供元 | 何をするか | 参考にできる点 |
| --- | --- | --- | --- |
| moq-bench | moq-dev | MoQ relay 向けの負荷生成 | group の sequence span による欠落検出 (live frontier を除外し、QUIC のパケットロスと区別する)、片道遅延の p50 / p90 / p99 / max と clock skew の除外、接続ごとに `[min, max]` でロールさせる負荷形状、`--fanout` による 1:N、JSON Lines 出力 |
| MoQPerfTestClient (moxygen) | Meta | MoQT relay の性能試験クライアント | 購読者のランプ (`--subscriber_ramp` / `--subscriber_max`)、配信タイムアウト、転送モード (QUIC / WebTransport / QMUX) の切替、I フレーム / P フレーム相当のサイズ指定 |
| moxygen conformance suite | Meta | MoQT の適合性試験 | 転送プリファレンス 4 種 (subgroup / datagram の構成)、object サイズ、group / object 構成の網羅 |
| moq-interop-runner | Mike English | 実装間の相互接続試験 | テストケースの設計 |
| moq-stats | moq-dev | 統計を MoQ のトラックとして配信する | 統計を別プロトコルではなく MoQ で運ぶ設計 |
| h2load / quic-go perf | nghttp2 / quic-go | HTTP/3 / QUIC の性能測定 | QUIC 層の最小 RTT / 平滑化 RTT / 損失宣言数、ハンドシェイク性能 |

MoQ の負荷生成は moq-bench と MoQPerfTestClient が既に存在し、group 単位の欠落と片道遅延の
パーセンタイルを出している。zakuro-moq の差別化は「Sora MoQ のリレーに対する、1 プロセス
多接続 + 多トラック + publish / subscribe 同時実行」と「zakuro と共通の設定・運用手順」に
ある。計測の設計はこの 2 ツールから借りるのが早い。

### 調査の限界

- 汎用の負荷試験ツールに WebRTC / MoQ の専用拡張は確認できなかった
- mediasoup 公式の負荷試験ツールは確認できなかった
- srs-bench の統計集約レポート形式、zakuro (C++ 版) の合否判定機能と統計レポート形式は
  確認できなかった
- MoQ リレーのキャッシュヒット率を出すメトリクス、QUIC の cwnd を出力する負荷試験ツールは
  確認できなかった

## 優先度の提案

「今ないもの」を、効果と実装コストの観点で並べる。あくまで提案であり、採用可否は別途判断する。

| 優先度 | 項目 | 理由 |
| --- | --- | --- |
| 高 | 合否判定 (しきい値と終了コード) | CI で性能回帰を検出できないことが最大の制約。MoQ の E2E は既にログ grep で判定しており、置き換え先になる。webrtcperf の `--alert-rules` (集約値 + 評価期間 + JSON 出力) が設計の参考になる |
| 高 | サマリ出力 (JSON) と集計 (avg / p95 / max) | DuckDB やログの後処理を毎回書かずに済む。しきい値判定の前提にもなる。moq-bench の JSON Lines 出力が粒度の参考になる |
| 高 | MoQ の object 遅延と欠落の計測 | MoQ 負荷試験ツールとしての価値の中心。moq-bench が group 単位の欠落と片道遅延のパーセンタイルを実装済みで、設計をそのまま借りられる |
| 中 | 開いたモデルとランプ | 接続チャーン耐性の試験に必要。既存の `DelayQueue` の延長で実装できる。moxygen の `--subscriber_ramp` が参考になる |
| 中 | メトリクスのリアルタイム出力 (Prometheus / OTLP のいずれか) | 長時間試験の観測に必要。まずは HTTP のメトリクスエンドポイントでもよい。webrtcperf は Pushgateway を使う |
| 中 | 実行中制御 API (状態取得 / 停止 / VC 変更) | 長時間試験の運用で必要。既存の JSON-RPC 基盤にメソッドを足せる |
| 中 | シナリオの外部定義 | `reconnect` 以外の需要に応える。JSONC でステップを並べる形式が既存の設定と整合する |
| 中 | 分散実行 (負荷の分担と結果集約) | 大規模試験の前提。まずは負荷の分担から。webrtcperf の collector / worker 構成が参考になる |
| 中 | WebRTC の接続確立時間と失敗理由の集計 | 接続成功率は SFU 評価の基本指標。pion/webrtc-bench のようにフェーズ別に記録すると切り分けやすい |
| 中 | 負荷生成側の資源記録 (CPU / メモリ) | 負荷生成器自身が限界に達していないかを判断できない。livekit-cli と pion/webrtc-bench は CPU 使用率を併記している |
| 低 | ネットワーク劣化注入 | 外部の tc / netem で代替できる。webrtcperf は tc + NetEm を内蔵している |
| 低 | Sora 側統計との突き合わせ | `ListRtcStats` と DuckDB のサンプル数を比較すると、統計の欠落を検出できる |
| 低 | `zakuro-moq lint` / `fmt` | 保守性の向上。設定の誤りは起動時エラーで検出できる |
| 低 | 実ブラウザ (Chromium) での品質計測 | webrtcperf の領域。zakuro の設計 (libwebrtc 直結) と方向性が異なる |

## 既存 issue との関係

この分析で挙げた不足のうち、既に issue として積まれているものは少ない。

| 既存 issue | 関連する不足 |
| --- | --- |
| 0001 RPC に Query メソッドを追加する | メトリクスのリアルタイム参照 (DuckDB を RPC 経由で引く) |
| 0003 UI リバースプロキシ | 可視化 (ただし負荷試験のメトリクス表示ではない) |
| 0004 解像度固定モード | 試験の再現性 |
| 0005 degradation-preference | メディア品質の制御 |
| 0009 connection ID ファイル出力 | 結果の出力 (DuckDB で代替済み) |

上記以外 (合否判定、サマリ出力、MoQ の遅延 / 欠落計測、開いたモデル、分散実行、
実行中制御、リアルタイムメトリクス出力) は issue 化されていない。

issue 化する場合の候補名の例を挙げる。

- `add-threshold-and-exit-code`
- `add-summary-report-json`
- `add-moq-object-latency-and-loss-metrics`
- `add-arrival-rate-load-model`
- `add-metrics-http-endpoint`
- `add-runtime-control-rpc`
- `add-scenario-file`
- `add-load-distribution`
