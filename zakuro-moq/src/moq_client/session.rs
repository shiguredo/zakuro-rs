//! MOQT セッションの駆動 (SETUP 交換・複数トラックの PUBLISH・object 送信)
//!
//! `shiguredo_moqt` の `Session` は Sans I/O の状態機械であり、ワイヤの読み書きは
//! 呼び出し側の責務である。このモジュールは s2n-quic のストリームと `Session` を
//! 1 つのタスクで駆動し、publisher として複数のトラックへ object を送り続ける。
//!
//! トラックごとに bidi request stream を 1 本開くため、その受信方向はストリームごとの
//! タスクで読み、デコードしたメッセージをチャネルでメインタスクへ渡す (`Session` は
//! `&mut self` を要求するため、複数ストリームを select! で直接読むことができない)。
//!
//! 対応する仕様は draft-ietf-moq-transport-21 (MOQT) と draft-ietf-moq-loc-04 (LOC) である。
//! 節番号は将来の draft 改訂で変わる可能性がある。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use s2n_quic::connection::{Connection, Handle};
use s2n_quic::stream::{ReceiveStream, SendStream};
use shiguredo_moqt::decoder::MessageDecoder;
use shiguredo_moqt::loc::{
    LocProperties, LocProperty, LocPropertyValue, PROP_TIMESCALE, PROP_TIMESTAMP,
};
use shiguredo_moqt::message::ControlMessage;
use shiguredo_moqt::message::common::TrackNamespace;
use shiguredo_moqt::message_parameter::MessageParameters;
use shiguredo_moqt::parameter::{
    SETUP_OPTION_AUTHORITY, SETUP_OPTION_MOQT_IMPLEMENTATION, SETUP_OPTION_PATH, SetupOption,
    SetupOptionValue, SetupOptions,
};
use shiguredo_moqt::session::core::Session;
use shiguredo_moqt::session::types::{
    DataStreamId, RequestKind, RequestStreamEnd, SendRequestError, SessionEvent,
    TrackDataAcceptance, Transport,
};
use shiguredo_moqt::stream::decoder::{DecodedSubgroupObject, SubgroupStreamDecoder};
use shiguredo_moqt::stream::encode_control_stream_setup;
use shiguredo_moqt::stream::subgroup::{SubgroupHeader, SubgroupIdMode, SubgroupObject};
use shiguredo_moqt::track_properties::TrackProperties;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::{ErrorMessage, Result};

/// SETUP Option の MOQT_IMPLEMENTATION に載せる実装名
///
/// draft-ietf-moq-transport-21 §9.1.5 (MOQT IMPLEMENTATION) は実装名とバージョンに
/// 限ることを SHOULD としているため、クレートのバージョンを付ける。
const MOQT_IMPLEMENTATION_NAME: &[u8] =
    concat!("zakuro-moq/", env!("CARGO_PKG_VERSION")).as_bytes();

/// 読み込みバッファのサイズ
const READ_BUFFER_SIZE: usize = 16 * 1024;

/// セッションのタイマーを進める間隔
///
/// `Session::tick` は control message のタイムアウトや GOAWAY の猶予時間を評価する。
/// 送信タイミングの粒度には影響しないため 50ms で十分である。
const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// stream type varint を読むための最大試行回数
///
/// varint は最大 8 バイトである。1 回の読み込みで最低 1 バイトは進むため、8 回で足りる。
const MAX_STREAM_TYPE_READS: usize = 8;

/// 送信状況をログに出す間隔
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// SETUP の交換が完了するまでの上限
const ESTABLISH_TIMEOUT: Duration = Duration::from_secs(10);

/// PUBLISH の応答 (REQUEST_OK) を待つ上限 (ms)
///
/// `Session::set_control_message_timeout_ms` に設定する。期限切れは `CloseSession`
/// (CONTROL_MESSAGE_TIMEOUT) として通知されるため、再接続の経路へ戻れる。
const PUBLISH_ACCEPT_TIMEOUT_MS: u64 = 10_000;

/// subgroup stream への書き込みが連続で失敗したときに諦める回数
const MAX_CONSECUTIVE_WRITE_FAILURES: u32 = 5;

/// 遅れがこの object 数分を超えたら、追いつきではなく現在時刻から送信を再開する
///
/// 長時間 Forward State 0 だった場合などに、溜まった分を一気に送らないための上限である。
const MAX_CATCH_UP_OBJECTS: u32 = 10;

/// LOC の Timestamp に使うタイムスケール (1 秒 = 1,000,000)
const LOC_TIMESCALE: u64 = 1_000_000;

/// publish するトラック 1 本の設定
#[derive(Debug, Clone)]
pub(crate) struct TrackConfig {
    /// Track Name (仮想クライアントごとに一意化済み)
    pub(crate) name: String,
    /// 1 秒あたりに送る object 数
    pub(crate) object_rate: f64,
    /// 1 object の payload サイズ (バイト)
    pub(crate) object_size: usize,
}

/// PUBLISH するトラックと object の設定
pub(crate) struct SessionConfig<'a> {
    /// Track Namespace
    pub(crate) namespace: &'a str,
    /// publish するトラック
    pub(crate) tracks: &'a [TrackConfig],
    /// SETUP の AUTHORITY に載せる値
    pub(crate) authority: &'a str,
    /// SETUP の PATH に載せる値
    pub(crate) path: &'a str,
    /// object の payload (最大サイズで 1 つ作り、トラック間で共有する)
    pub(crate) payload: Arc<[u8]>,
    /// subscribe するトラックの Full Track Name (空なら購読しない)
    pub(crate) subscribe_tracks: &'a [String],
    /// 受信 payload が zakuro-moq の publisher のパターンと一致するかを検査する
    pub(crate) verify_payload: bool,
}

/// subscribe 中のトラックの状態
struct SubscriptionState {
    /// Full Track Name
    name: String,
    /// SUBSCRIBE の Request ID (送信後に設定する)
    request_id: Option<u64>,
    /// SUBSCRIBE_OK を受信したか
    accepted: bool,
    /// SUBSCRIBE_OK で確定した Track Alias (受信 object の帰属表示に使う)
    track_alias: Option<u64>,
    /// 受信した object 数
    received_objects: u64,
    /// 受信した payload の合計バイト数
    received_bytes: u64,
    /// payload のパターン検査に失敗した数 (`--verify-payload` 指定時のみ)
    mismatched_payloads: u64,
}

impl SubscriptionState {
    /// 購読状態を作る
    fn new(name: String) -> Self {
        Self {
            name,
            request_id: None,
            accepted: false,
            track_alias: None,
            received_objects: 0,
            received_bytes: 0,
            mismatched_payloads: 0,
        }
    }
}

/// data stream 読みタスクからメインタスクへ渡すメッセージ
enum DataMessage {
    /// uni stream の種別が判明した
    StreamType {
        /// 対象 stream の DataStreamId
        stream_id: DataStreamId,
        /// stream type の varint 値
        stream_type: u64,
    },
    /// SUBGROUP_HEADER をデコードした
    Header {
        /// 対象 stream の DataStreamId
        stream_id: DataStreamId,
        /// デコードしたヘッダ
        header: Box<SubgroupHeader>,
    },
    /// SUBGROUP_OBJECT を 1 つ受け取った
    Object {
        /// 対象 stream の DataStreamId
        stream_id: DataStreamId,
        /// デコードした object
        object: Box<DecodedSubgroupObject>,
        /// payload の長さ (バイト)
        payload_length: u64,
        /// payload が期待するパターンと一致したか (`--verify-payload` 指定時のみ検査)
        payload_ok: bool,
    },
    /// stream が終端した
    Closed {
        /// 対象 stream の DataStreamId
        stream_id: DataStreamId,
    },
    /// 読み込みに失敗した (stream は破棄する)
    Failed {
        /// 対象 stream の DataStreamId
        stream_id: DataStreamId,
        /// 失敗の理由
        reason: String,
    },
}

/// ストリーム読みタスクからメインタスクへ渡すメッセージ
enum RequestMessage {
    /// 受信した制御メッセージ
    Message {
        /// 対象 request の Request ID
        request_id: u64,
        /// 受信したメッセージ
        message: ControlMessage,
    },
    /// peer が送信方向を閉じた (FIN)
    Closed {
        /// 対象 request の Request ID
        request_id: u64,
    },
}

/// セッション中に変化する状態
///
/// `drain_events` が `Session` のイベントを処理しながら更新する。
struct SessionState {
    /// 自側の制御ストリーム (GOAWAY を書く)
    control_send: SendStream,
    /// 接続のハンドル (request stream の open と接続の close に使う)
    connection_handle: Handle,
    /// 自側が開始した bidi request stream の送信方向 (Request ID ごと)
    request_sends: HashMap<u64, SendStream>,
    /// セッションが確立したか (両側の SETUP が完了したか)
    established: bool,
    /// peer からセッション終了を通知された場合の理由
    closed_reason: Option<String>,
    /// 制御ストリームが閉じられたか
    control_closed: bool,
    /// Session が要求した data stream の reset (I/O 層が実行する)
    ///
    /// 同じ `drain_events` の中で複数トラックの reset が届くため、1 件ずつ処理できるよう
    /// 配列で持つ。
    reset_data_streams: Vec<(DataStreamId, u64)>,
    /// セッション確立を通知するチャネル (呼び出し側が待ち合わせに使う)
    established_notify: Option<tokio::sync::oneshot::Sender<()>>,
}

/// 1 本のトラックへ object を送り続ける publisher
struct Publisher {
    /// トラックの設定
    config: TrackConfig,
    /// PUBLISH の Request ID (送信後すぐ設定する)
    request_id: Option<u64>,
    /// PUBLISH に使う Track Alias
    track_alias: u64,
    /// PUBLISH が受理されたか
    accepted: bool,
    /// Forward State (1 のときだけ object を送る)
    ///
    /// draft-ietf-moq-transport-21 §3.1 (Subscriptions): "The publisher does not send Objects
    /// if the Forward State is 0, and does send them if the Forward State is 1."
    /// FORWARD は request (トラック) 単位の値であるため、トラックごとに保持する。
    forward: bool,
    /// 次に object を送る時刻
    next_send: tokio::time::Instant,
    /// object を送る周期
    period: Duration,
    /// 開いている subgroup stream (FIN で閉じた後は `None`)
    stream: Option<SendStream>,
    /// 開いている subgroup stream の DataStreamId
    stream_id: Option<DataStreamId>,
    /// 現在の Group ID
    group_id: u64,
    /// 現在の Group 内で次に送る Object ID
    object_id: u64,
    /// 現在の subgroup stream で直前に送った Object ID
    prev_object_id: Option<u64>,
    /// 1 Group あたりの object 数 (約 1 秒分)
    objects_per_group: u64,
    /// これまでに送った object 数 (Timestamp の算出に使う)
    sent_objects: u64,
    /// object のヘッダと payload を組み立てる再利用バッファ
    scratch: Vec<u8>,
    /// subgroup stream への書き込みが連続で失敗した回数
    consecutive_write_failures: u32,
}

impl Publisher {
    /// publisher を作る
    fn new(config: TrackConfig, track_alias: u64, now: tokio::time::Instant) -> Self {
        let period = Duration::from_secs_f64(1.0 / config.object_rate);
        let objects_per_group = config.object_rate.round().max(1.0) as u64;
        Self {
            config,
            request_id: None,
            track_alias,
            accepted: false,
            forward: true,
            next_send: now,
            period,
            stream: None,
            stream_id: None,
            group_id: 0,
            object_id: 0,
            prev_object_id: None,
            objects_per_group,
            sent_objects: 0,
            scratch: Vec::new(),
            consecutive_write_failures: 0,
        }
    }

    /// 送信すべき時刻かどうか
    fn is_due(&self, now: tokio::time::Instant) -> bool {
        self.accepted && self.forward && now >= self.next_send
    }

    /// 次に送信する時刻を返す (未受理・Forward State 0 の間は `None`)
    fn deadline(&self) -> Option<tokio::time::Instant> {
        if self.accepted && self.forward {
            Some(self.next_send)
        } else {
            None
        }
    }

    /// PUBLISH が受理されたときに呼ぶ
    fn on_accepted(&mut self, request_id: u64) {
        self.request_id = Some(request_id);
        self.accepted = true;
        // 受理直後は待たずに送り始める
        self.next_send = tokio::time::Instant::now();
    }

    /// Forward State を更新し、送信予定を調整する
    ///
    /// Forward State 1 に戻ったときに、止まっていた分を一気に送らないよう現在時刻から
    /// 仕切り直す。
    fn on_forward_changed(&mut self, forward: bool) {
        self.forward = forward;
        if forward {
            let now = tokio::time::Instant::now();
            if self.next_send < now {
                self.next_send = now;
            }
        }
    }

    /// 次の object を 1 つ送る
    ///
    /// # Errors
    ///
    /// ストリームの open に失敗した場合、または `Session` が object の送信を拒否した場合は
    /// エラーになる。書き込みの失敗は stream の終端として扱い、次の Group から送り直す。
    async fn send_next_object(
        &mut self,
        session: &mut Session,
        handle: &mut Handle,
        payload: &Arc<[u8]>,
        objects_sent: &AtomicU64,
    ) -> Result<()> {
        let request_id = self
            .request_id
            .expect("logical invariant: send_next_object is called only after publish is sent");

        // Group の終わりに達していたら FIN で閉じ、次の Group へ進む
        if self.object_id >= self.objects_per_group {
            self.close_stream(session);
            self.group_id += 1;
            self.object_id = 0;
            self.prev_object_id = None;
        }

        // 最初の object を送る前に subgroup stream を開く
        if self.stream.is_none() {
            let mut stream = handle.open_send_stream().await.map_err(|e| {
                ErrorMessage::new(format!("subgroup stream の open に失敗しました: {e}"))
            })?;
            let stream_id = DataStreamId(stream.id());
            let header = SubgroupHeader {
                track_alias: self.track_alias,
                group_id: self.group_id,
                // Subgroup は Group 内で 1 本に固定する (Subgroup ID 0)
                subgroup_id: SubgroupIdMode::Zero,
                // DEFAULT_PRIORITY bit を立て、優先度はトラックの既定値に任せる
                publisher_priority: None,
                has_properties: true,
                end_of_group: false,
                first_object: true,
            };
            session
                .send_subgroup_header(stream_id, request_id, &header)
                .map_err(|e| {
                    ErrorMessage::new(format!("SUBGROUP_HEADER の登録に失敗しました: {e:?}"))
                })?;
            // SUBGROUP_HEADER の先頭 varint がそのまま stream type になる
            stream.write_all(&header.encode()).await.map_err(|e| {
                ErrorMessage::new(format!("SUBGROUP_HEADER の送信に失敗しました: {e}"))
            })?;
            self.stream = Some(stream);
            self.stream_id = Some(stream_id);
            self.prev_object_id = None;
        }

        let stream_id = self
            .stream_id
            .expect("logical invariant: subgroup stream is opened above");
        let object_id = self.object_id;
        let object_id_delta = object_id_delta(self.prev_object_id, object_id);

        let properties = build_loc_properties(self.sent_objects, self.config.object_rate);
        let properties_bytes = properties.encode().map_err(|e| {
            ErrorMessage::new(format!("LOC プロパティのエンコードに失敗しました: {e:?}"))
        })?;

        // Session への登録はワイヤへ書く前に行う。フィルタ不通過の場合はワイヤへ出さない
        // (draft-ietf-moq-transport-21 §3.3.3 (Combining Filters))
        if let Err(e) = session.send_subgroup_object(stream_id, object_id, Some(&properties_bytes))
        {
            if matches!(e, SendRequestError::LocalFilterMismatch) {
                // フィルタ不通過でワイヤへ出さない場合も次の送信時刻を進める。進めないと
                // 直後のループで再び期日到来と判定され、sleep せずに回り続ける
                self.object_id += 1;
                self.advance_next_send();
                return Ok(());
            }
            return Err(
                ErrorMessage::new(format!("SUBGROUP_OBJECT の登録に失敗しました: {e:?}")).into(),
            );
        }

        let payload_bytes = &payload[..self.config.object_size];
        let mut buf = std::mem::take(&mut self.scratch);
        buf.clear();
        SubgroupObject {
            object_id_delta,
            payload_length: payload_bytes.len() as u64,
            status: None,
        }
        .encode(true, Some(&properties_bytes), &mut buf)
        .map_err(|e| {
            ErrorMessage::new(format!("SUBGROUP_OBJECT のエンコードに失敗しました: {e:?}"))
        })?;
        buf.extend_from_slice(payload_bytes);

        let write_result = {
            let stream = self
                .stream
                .as_mut()
                .expect("logical invariant: subgroup stream is opened above");
            stream.write_all(&buf).await
        };
        self.scratch = buf;
        if let Err(e) = write_result {
            return self.on_stream_write_failure(session, stream_id, e);
        }

        self.consecutive_write_failures = 0;
        self.prev_object_id = Some(object_id);
        self.object_id += 1;
        self.sent_objects += 1;
        objects_sent.fetch_add(1, Ordering::Relaxed);

        self.advance_next_send();
        Ok(())
    }

    /// 次の送信時刻を 1 周期進める
    ///
    /// 遅れが `MAX_CATCH_UP_OBJECTS` 周期分を超えた場合は現在時刻から仕切り直す
    /// (Forward State 0 からの復帰時などに、溜まった分を一気に送らないため)。
    fn advance_next_send(&mut self) {
        let now = tokio::time::Instant::now();
        let mut next = self.next_send + self.period;
        let catch_up =
            Duration::from_secs_f64(self.period.as_secs_f64() * f64::from(MAX_CATCH_UP_OBJECTS));
        if next + catch_up < now {
            next = now + self.period;
        }
        self.next_send = next;
    }

    /// 開いている subgroup stream を FIN で閉じる
    fn close_stream(&mut self, session: &mut Session) {
        if let (Some(stream), Some(stream_id)) = (self.stream.as_mut(), self.stream_id) {
            let _ = session.send_data_stream_closed(stream_id, RequestStreamEnd::Fin);
            let _ = stream.finish();
        }
        self.stream = None;
        self.stream_id = None;
    }

    /// Session が要求した data stream の reset を実行する
    fn reset_stream(&mut self, session: &mut Session, stream_id: DataStreamId, error_code: u64) {
        if self.stream_id != Some(stream_id) {
            return;
        }
        if let Some(stream) = self.stream.as_mut() {
            let _ = stream.reset(application_error(error_code));
        }
        self.stream = None;
        self.stream_id = None;
        let _ = session.send_data_stream_closed(
            stream_id,
            RequestStreamEnd::Reset {
                error_code: Some(error_code),
                reliable_size: None,
            },
        );
        // reset された Subgroup を再オープンすると Session が拒否することがある
        // (draft-ietf-moq-transport-21 §11.3.2 (Closing Subgroup Streams))。次の Group へ
        // 進めて、新しい Subgroup として送り直す
        self.group_id += 1;
        self.object_id = 0;
        self.prev_object_id = None;
    }

    /// subgroup stream への書き込みが失敗したときの処理
    ///
    /// peer が STOP_SENDING を送ると s2n-quic は送信側を reset 状態にし、以降の書き込みは
    /// 失敗する。公開 API から STOP_SENDING の受信を直接観測できないため、書き込み失敗を
    /// stream の終端とみなして Session へ通知し (draft-ietf-moq-transport-21 §11.3.2)、
    /// 次の Group を新しい stream で送り直す。
    fn on_stream_write_failure(
        &mut self,
        session: &mut Session,
        stream_id: DataStreamId,
        cause: std::io::Error,
    ) -> Result<()> {
        let _ = session.recv_data_stream_stop_sending(stream_id);
        self.stream = None;
        self.stream_id = None;
        self.group_id += 1;
        self.object_id = 0;
        self.prev_object_id = None;
        self.consecutive_write_failures += 1;
        if self.consecutive_write_failures >= MAX_CONSECUTIVE_WRITE_FAILURES {
            return Err(ErrorMessage::new(format!(
                "subgroup stream への書き込みが連続で失敗しました (track={}): {cause}",
                self.config.name,
            ))
            .into());
        }
        tracing::warn!(
            "MOQT: subgroup stream write failed (track={}): {cause}; retrying with the next group",
            self.config.name,
        );
        Ok(())
    }
}

/// SUBGROUP_OBJECT に載せる Object ID Delta を求める
///
/// draft-ietf-moq-transport-21 §11.3.1 (Subgroup Header): 最初の Object は絶対 ID を送り、
/// 以降は直前の Object ID との差から 1 を引いた Delta を送る。受信側は「直前の Object ID +
/// Delta + 1」で絶対 ID を復元するため、フィルタで送らなかった Object があっても Delta が
/// その分を吸収する (直前 ID は実際に送った Object のものだけを渡すこと)。
///
/// この節番号・規則は draft 由来であり将来の draft 改版で変わる可能性がある。
fn object_id_delta(prev_object_id: Option<u64>, object_id: u64) -> u64 {
    match prev_object_id {
        Some(prev) => object_id - prev - 1,
        None => object_id,
    }
}

/// MOQT セッションを確立し、publisher として object を送り続ける
///
/// `token` がキャンセルされるか、peer がセッションを閉じるか、transport が失敗するまで
/// 戻らない。
///
/// # Errors
///
/// 接続が確立できない場合、MOQT のメッセージが不正な場合、ストリームの読み書きに
/// 失敗した場合はエラーになる。
pub(crate) async fn run(
    connection: Connection,
    config: &SessionConfig<'_>,
    token: &CancellationToken,
    established_notify: Option<tokio::sync::oneshot::Sender<()>>,
    objects_sent: Arc<AtomicU64>,
    receive_counters: ReceiveCounters,
) -> Result<()> {
    let mut connection_handle = connection.handle();
    // Forward State 0 の間は object を送らないため、無通信で idle timeout に達しないよう
    // keep-alive を有効にする
    connection_handle
        .keep_alive(true)
        .map_err(|e| ErrorMessage::new(format!("keep-alive の有効化に失敗しました: {e}")))?;

    let (mut handle, acceptor) = connection.split();
    let (mut bidi_acceptor, mut recv_acceptor) = acceptor.split();

    let mut session = Session::new_client(Transport::Quic, build_setup_options(config))
        .map_err(|e| ErrorMessage::new(format!("MOQT セッションの生成に失敗しました: {e:?}")))?;

    // 自側の制御ストリームを開き、SETUP を送る
    let mut control_send = handle
        .open_send_stream()
        .await
        .map_err(|e| ErrorMessage::new(format!("制御ストリームの open に失敗しました: {e}")))?;
    let setup_message = take_setup_message(&mut session)?;
    control_send
        .write_all(
            &encode_control_stream_setup(&setup_message).map_err(|e| {
                ErrorMessage::new(format!("SETUP のエンコードに失敗しました: {e:?}"))
            })?,
        )
        .await
        .map_err(|e| ErrorMessage::new(format!("SETUP の送信に失敗しました: {e}")))?;

    // peer の制御ストリームを受ける
    let accepted = tokio::time::timeout(
        ESTABLISH_TIMEOUT,
        accept_control_stream(&mut recv_acceptor, token),
    )
    .await
    .map_err(|_| {
        ErrorMessage::new("MOQT の制御ストリームを受信できませんでした (上限時間を超過)")
    })?;
    let (mut peer_control, peer_stream_type, mut control_decoder) = accepted?;

    // request stream の受信方向はストリームごとのタスクで読み、メッセージをチャネルで受ける
    let (request_tx, mut request_rx) = mpsc::channel::<RequestMessage>(256);

    let mut state = SessionState {
        control_send,
        connection_handle: connection_handle.clone(),
        request_sends: HashMap::new(),
        established: false,
        closed_reason: None,
        control_closed: false,
        reset_data_streams: Vec::new(),
        established_notify,
    };
    session
        .recv_control_stream_type(peer_stream_type)
        .map_err(|e| ErrorMessage::new(format!("制御ストリーム種別の通知に失敗しました: {e:?}")))?;
    recv_control_messages(&mut session, &mut control_decoder)?;
    drain_events(&mut session, &mut state, None, None, &request_tx, token).await?;

    // トラックごとの publisher を作る (Track Alias は 1 から順に払い出す)
    let now = tokio::time::Instant::now();
    let mut publishers: Vec<Publisher> = config
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| Publisher::new(track.clone(), i as u64 + 1, now))
        .collect();

    // 購読するトラック (Full Track Name は呼び出し側で解決済み)
    let mut subscriptions: Vec<SubscriptionState> = config
        .subscribe_tracks
        .iter()
        .map(|name| SubscriptionState::new(name.clone()))
        .collect();

    // 受信した uni data stream はストリームごとのタスクで読み、ここで受ける
    let (data_tx, mut data_rx) = mpsc::channel::<DataMessage>(256);
    // 受信中の stream がどの Track Alias のものか (object の帰属判定に使う)
    let mut stream_aliases: HashMap<DataStreamId, u64> = HashMap::new();

    let mut control_buf = vec![0u8; READ_BUFFER_SIZE];
    let mut tick_timer = tokio::time::interval(TICK_INTERVAL);
    tick_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut progress_timer = tokio::time::interval(PROGRESS_INTERVAL);
    progress_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    progress_timer.tick().await;

    let start = tokio::time::Instant::now();
    let establish_deadline = tokio::time::sleep(ESTABLISH_TIMEOUT);
    tokio::pin!(establish_deadline);

    loop {
        // 次に object を送る時刻 (全トラックの最小値) を求める
        let next_deadline = publishers.iter().filter_map(|p| p.deadline()).min();

        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            _ = &mut establish_deadline, if !state.established => {
                return Err(ErrorMessage::new(
                    "MOQT セッションが確立しませんでした (SETUP の交換が完了しない)",
                )
                .into());
            }
            _ = tick_timer.tick() => {
                let now_ms = start.elapsed().as_millis() as u64;
                session.tick(now_ms);
                drain_events(
                    &mut session,
                    &mut state,
                    Some(&mut publishers),
                    Some(&mut subscriptions),
                    &request_tx,
                    token,
                )
                .await?;
            }
            read = peer_control.read(&mut control_buf), if !state.control_closed => {
                let size = read.map_err(|e| ErrorMessage::new(format!("制御ストリームの読み込みに失敗しました: {e}")))?;
                if size == 0 {
                    // draft-ietf-moq-transport-21 §6.4.1: 制御ストリームはセッション中に
                    // 閉じてはならない
                    state.control_closed = true;
                    session
                        .recv_control_stream_closed(RequestStreamEnd::Fin)
                        .map_err(|e| ErrorMessage::new(format!("制御ストリームの終端処理に失敗しました: {e:?}")))?;
                } else {
                    control_decoder.push(&control_buf[..size]);
                    recv_control_messages(&mut session, &mut control_decoder)?;
                }
                drain_events(
                    &mut session,
                    &mut state,
                    Some(&mut publishers),
                    Some(&mut subscriptions),
                    &request_tx,
                    token,
                )
                .await?;
            }
            maybe = request_rx.recv() => {
                match maybe {
                    Some(RequestMessage::Message { request_id, message }) => {
                        session.recv_stream_message(request_id, message).map_err(|e| {
                            ErrorMessage::new(format!("request メッセージの受理に失敗しました: {e:?}"))
                        })?;
                    }
                    Some(RequestMessage::Closed { request_id }) => {
                        if let Err(e) = session.recv_request_stream_closed(request_id, RequestStreamEnd::Fin) {
                            tracing::warn!(
                                "MOQT: request stream closed with an error: request_id={} error={:?}",
                                request_id,
                                e,
                            );
                        }
                    }
                    None => break,
                }
                drain_events(
                    &mut session,
                    &mut state,
                    Some(&mut publishers),
                    Some(&mut subscriptions),
                    &request_tx,
                    token,
                )
                .await?;
            }
            _ = sleep_until_optional(next_deadline) => {
                let now = tokio::time::Instant::now();
                for publisher in publishers.iter_mut() {
                    if publisher.is_due(now) {
                        publisher
                            .send_next_object(&mut session, &mut handle, &config.payload, &objects_sent)
                            .await?;
                    }
                }
            }
            maybe = data_rx.recv() => {
                let Some(message) = maybe else { break };
                match message {
                    DataMessage::StreamType { stream_id, stream_type } => {
                        session
                            .recv_data_stream_type(stream_id, stream_type)
                            .map_err(|e| ErrorMessage::new(format!("data stream 種別の登録に失敗しました: {e:?}")))?;
                    }
                    DataMessage::Header { stream_id, header } => {
                        match session.recv_subgroup_header(stream_id, &header) {
                            Ok(_) => {
                                // 受信 object の帰属 (どのトラックか) を判定するために覚えておく
                                stream_aliases.insert(stream_id, header.track_alias);
                            }
                            // 未知の Track Alias は relay 側の都合 (購読前の残留など) で起こり得る。
                            // セッションは閉じずにこの stream を捨てる
                            Err(e) => {
                                tracing::warn!(
                                    "MOQT: SUBGROUP_HEADER rejected (track_alias={}): {e:?}",
                                    header.track_alias,
                                );
                                let _ = session.recv_data_stream_stop_sending(stream_id);
                            }
                        }
                    }
                    DataMessage::Object { stream_id, object, payload_length, payload_ok } => {
                        match session.recv_subgroup_object(stream_id, &object) {
                            Ok(acceptance) => {
                                if matches!(acceptance, TrackDataAcceptance::Accepted) {
                                    let alias = stream_aliases.get(&stream_id).copied();
                                    record_received_object(
                                        &mut subscriptions,
                                        alias,
                                        payload_length,
                                        payload_ok,
                                        &receive_counters,
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!("MOQT: SUBGROUP_OBJECT rejected: {e:?}");
                            }
                        }
                    }
                    DataMessage::Closed { stream_id } => {
                        stream_aliases.remove(&stream_id);
                        let _ = session.recv_data_stream_closed(stream_id, RequestStreamEnd::Fin);
                    }
                    DataMessage::Failed { stream_id, reason } => {
                        tracing::warn!("MOQT: data stream read failed: stream_id={stream_id:?} {reason}");
                        stream_aliases.remove(&stream_id);
                        let _ = session.recv_data_stream_closed(stream_id, RequestStreamEnd::Fin);
                    }
                }
            }
            accepted = recv_acceptor.accept_receive_stream() => {
                match accepted {
                    Ok(Some(stream)) => {
                        // 購読しているトラックの object は uni data stream で届く
                        let tx = data_tx.clone();
                        let reader_token = token.child_token();
                        let verify_payload = config.verify_payload;
                        tokio::spawn(async move {
                            run_data_reader(stream, tx, reader_token, verify_payload).await;
                        });
                    }
                    Ok(None) => break,
                    Err(e) => return Err(ErrorMessage::new(format!("uni stream の受信に失敗しました: {e}")).into()),
                }
            }
            accepted = bidi_acceptor.accept_bidirectional_stream() => {
                match accepted {
                    Ok(Some(stream)) => {
                        // peer 起点の request (TRACK_STATUS / SUBSCRIBE 等) は扱わない
                        drop(stream);
                    }
                    Ok(None) => break,
                    Err(e) => return Err(ErrorMessage::new(format!("bidi stream の受信に失敗しました: {e}")).into()),
                }
            }
            _ = progress_timer.tick() => {
                let sent: u64 = publishers.iter().map(|p| p.sent_objects).sum();
                let accepted_count = publishers.iter().filter(|p| p.accepted).count();
                let received: u64 = subscriptions.iter().map(|s| s.received_objects).sum();
                let subscribe_accepted = subscriptions.iter().filter(|s| s.accepted).count();
                tracing::info!(
                    "MOQT progress: publish={}/{} sent-objects={} forwarding={} subscribe={}/{} received-objects={}",
                    accepted_count,
                    publishers.len(),
                    sent,
                    publishers.iter().filter(|p| p.forward).count(),
                    subscribe_accepted,
                    subscriptions.len(),
                    received,
                );
            }
        }

        // Session が要求した data stream の reset は publisher が stream を所有しているため
        // ここで実行する (同時に複数届くことがあるため全件処理する)
        for (stream_id, error_code) in state.reset_data_streams.drain(..) {
            for publisher in publishers.iter_mut() {
                publisher.reset_stream(&mut session, stream_id, error_code);
            }
        }

        // セッション確立を観測したら全トラックの PUBLISH を開始する
        if state.established {
            let mut published = false;
            for publisher in publishers.iter_mut() {
                if publisher.request_id.is_some() {
                    continue;
                }
                // PUBLISH の応答が返らないままセッションが Established で残らないよう、
                // 制御メッセージの応答待ちタイムアウトを設定する
                session.set_control_message_timeout_ms(Some(PUBLISH_ACCEPT_TIMEOUT_MS));
                let request_id = session
                    .send_publish(
                        TrackNamespace::new(vec![config.namespace.as_bytes().to_vec()]).map_err(
                            |e| ErrorMessage::new(format!("Track Namespace が不正です: {e:?}")),
                        )?,
                        publisher.config.name.as_bytes().to_vec(),
                        publisher.track_alias,
                        MessageParameters::new(),
                        TrackProperties::new(),
                    )
                    .map_err(|e| {
                        ErrorMessage::new(format!("PUBLISH の送信に失敗しました: {e:?}"))
                    })?;
                publisher.request_id = Some(request_id);
                publisher.accepted = false;
                published = true;
            }
            if published {
                drain_events(
                    &mut session,
                    &mut state,
                    Some(&mut publishers),
                    Some(&mut subscriptions),
                    &request_tx,
                    token,
                )
                .await?;
            }

            // 購読するトラックの SUBSCRIBE を送る
            let mut subscribed = false;
            for subscription in subscriptions.iter_mut() {
                if subscription.request_id.is_some() {
                    continue;
                }
                // SUBSCRIBE_OK が返らないままセッションが残らないよう、応答待ちタイムアウトを
                // 設定する (PUBLISH と同じ扱い)
                session.set_control_message_timeout_ms(Some(PUBLISH_ACCEPT_TIMEOUT_MS));
                let request_id = session
                    .send_subscribe(
                        TrackNamespace::new(vec![config.namespace.as_bytes().to_vec()]).map_err(
                            |e| ErrorMessage::new(format!("Track Namespace が不正です: {e:?}")),
                        )?,
                        subscription.name.as_bytes().to_vec(),
                        MessageParameters::new(),
                    )
                    .map_err(|e| {
                        ErrorMessage::new(format!("SUBSCRIBE の送信に失敗しました: {e:?}"))
                    })?;
                tracing::info!(
                    "MOQT subscribe sent: track={} request_id={}",
                    subscription.name,
                    request_id,
                );
                subscription.request_id = Some(request_id);
                subscription.accepted = false;
                subscribed = true;
            }
            if subscribed {
                drain_events(
                    &mut session,
                    &mut state,
                    Some(&mut publishers),
                    Some(&mut subscriptions),
                    &request_tx,
                    token,
                )
                .await?;
            }
        }

        if let Some(reason) = state.closed_reason.take() {
            return Err(
                ErrorMessage::new(format!("MOQT セッションが閉じられました: {reason}")).into(),
            );
        }
    }

    // 終了処理: 送信結果を先に記録してから、FIN で開いている stream を閉じ GOAWAY を送る
    let sent_total: u64 = publishers.iter().map(|p| p.sent_objects).sum();
    let accepted_count = publishers.iter().filter(|p| p.accepted).count();
    let received_total: u64 = subscriptions.iter().map(|s| s.received_objects).sum();
    let received_bytes: u64 = subscriptions.iter().map(|s| s.received_bytes).sum();
    let mismatched: u64 = subscriptions.iter().map(|s| s.mismatched_payloads).sum();
    tracing::info!(
        "MOQT finished: publish={}/{} sent-objects={} forwarding={} subscribe={}/{} received-objects={} received-bytes={} payload-mismatches={}",
        accepted_count,
        publishers.len(),
        sent_total,
        publishers.iter().filter(|p| p.forward).count(),
        subscriptions.iter().filter(|s| s.accepted).count(),
        subscriptions.len(),
        received_total,
        received_bytes,
        mismatched,
    );
    for publisher in publishers.iter_mut() {
        publisher.close_stream(&mut session);
    }
    let _ = session.send_goaway(Vec::new(), 0);
    let _ = drain_events(
        &mut session,
        &mut state,
        Some(&mut publishers),
        Some(&mut subscriptions),
        &request_tx,
        token,
    )
    .await;
    session.close(0x0, "zakuro-moq client closed");
    let _ = drain_events(
        &mut session,
        &mut state,
        Some(&mut publishers),
        Some(&mut subscriptions),
        &request_tx,
        token,
    )
    .await;
    Ok(())
}

/// request stream の受信方向を読み続けるタスク
///
/// デコードしたメッセージをチャネルでメインタスクへ渡す。チャネルが閉じた場合
/// (セッション終了) は読み込みを止める。
async fn run_request_reader(
    request_id: u64,
    mut stream: ReceiveStream,
    tx: mpsc::Sender<RequestMessage>,
    token: CancellationToken,
) {
    let mut decoder = MessageDecoder::new();
    let mut buf = vec![0u8; READ_BUFFER_SIZE];
    loop {
        let read = tokio::select! {
            biased;
            _ = token.cancelled() => return,
            read = stream.read(&mut buf) => read,
        };
        match read {
            Ok(0) => {
                let _ = tx.send(RequestMessage::Closed { request_id }).await;
                return;
            }
            Ok(size) => {
                decoder.push(&buf[..size]);
                loop {
                    match decoder.try_decode_message() {
                        Ok(Some(message)) => {
                            if tx
                                .send(RequestMessage::Message {
                                    request_id,
                                    message,
                                })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(
                                "MOQT: request message decode failed: request_id={} error={:?}",
                                request_id,
                                e,
                            );
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "MOQT: request stream read failed: request_id={} error={}",
                    request_id,
                    e,
                );
                return;
            }
        }
    }
}

/// 仮想クライアントをまたいで集計する受信カウンタ
#[derive(Clone, Default)]
pub(crate) struct ReceiveCounters {
    /// 受信した object 数
    pub(crate) objects: Arc<AtomicU64>,
    /// 受信した payload の合計バイト数
    pub(crate) bytes: Arc<AtomicU64>,
    /// payload のパターン検査に失敗した数
    pub(crate) mismatched_payloads: Arc<AtomicU64>,
}

impl ReceiveCounters {
    /// カウンタを作る
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 受信した object を記録する
    fn record(&self, payload_length: u64, payload_ok: bool) {
        self.objects.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(payload_length, Ordering::Relaxed);
        if !payload_ok {
            self.mismatched_payloads.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 現在の集計値を返す
    pub(crate) fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.objects.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
            self.mismatched_payloads.load(Ordering::Relaxed),
        )
    }
}

/// 受信した object を購読状態へ記録する
///
/// Track Alias が購読に対応しない場合 (購読を終了した直後など) はグローバルな
/// カウンタだけを進める。
fn record_received_object(
    subscriptions: &mut [SubscriptionState],
    track_alias: Option<u64>,
    payload_length: u64,
    payload_ok: bool,
    counters: &ReceiveCounters,
) {
    if let Some(alias) = track_alias
        && let Some(subscription) = subscriptions
            .iter_mut()
            .find(|s| s.track_alias == Some(alias))
    {
        subscription.received_objects += 1;
        subscription.received_bytes += payload_length;
        if !payload_ok {
            subscription.mismatched_payloads += 1;
        }
    }
    counters.record(payload_length, payload_ok);
}

/// uni data stream を読み、デコードした内容をメインタスクへ送るタスク
///
/// `Session` は `&mut self` を要求するため複数 stream を同時に扱えない。デコードだけを
/// このタスクで行い、`Session` への通知はメインタスクが行う。
async fn run_data_reader(
    mut stream: ReceiveStream,
    tx: mpsc::Sender<DataMessage>,
    token: CancellationToken,
    verify_payload: bool,
) {
    let stream_id = DataStreamId(stream.id());
    let mut buf = vec![0u8; READ_BUFFER_SIZE];

    // 先頭の stream type varint を読む。読み込んだバイトは SubgroupStreamDecoder にも
    // そのまま渡す必要があるため、生バイトを蓄積しておく
    let mut raw: Vec<u8> = Vec::new();
    let mut type_decoder = MessageDecoder::new();
    let stream_type = loop {
        match type_decoder.try_decode_varint() {
            Ok(Some(stream_type)) => break stream_type,
            Ok(None) => {}
            Err(e) => {
                let _ = tx
                    .send(DataMessage::Failed {
                        stream_id,
                        reason: format!("stream type のデコードに失敗しました: {e:?}"),
                    })
                    .await;
                return;
            }
        }
        match read_chunk(&mut stream, &mut buf, &token).await {
            Ok(Some(size)) => {
                raw.extend_from_slice(&buf[..size]);
                type_decoder.push(&buf[..size]);
            }
            Ok(None) => {
                let _ = tx.send(DataMessage::Closed { stream_id }).await;
                return;
            }
            Err(e) => {
                let _ = tx
                    .send(DataMessage::Failed {
                        stream_id,
                        reason: format!("読み込みに失敗しました: {e}"),
                    })
                    .await;
                return;
            }
        }
    };
    if tx
        .send(DataMessage::StreamType {
            stream_id,
            stream_type,
        })
        .await
        .is_err()
    {
        return;
    }

    let mut decoder = SubgroupStreamDecoder::new();
    decoder.push(&raw);
    raw.clear();

    // SUBGROUP_HEADER をデコードする
    let header = loop {
        match decoder.try_decode_header() {
            Ok(Some(header)) => break header,
            Ok(None) => {}
            Err(e) => {
                let _ = tx
                    .send(DataMessage::Failed {
                        stream_id,
                        reason: format!("SUBGROUP_HEADER のデコードに失敗しました: {e:?}"),
                    })
                    .await;
                return;
            }
        }
        match read_chunk(&mut stream, &mut buf, &token).await {
            Ok(Some(size)) => decoder.push(&buf[..size]),
            Ok(None) => {
                let _ = tx.send(DataMessage::Closed { stream_id }).await;
                return;
            }
            Err(e) => {
                let _ = tx
                    .send(DataMessage::Failed {
                        stream_id,
                        reason: format!("読み込みに失敗しました: {e}"),
                    })
                    .await;
                return;
            }
        }
    };
    if tx
        .send(DataMessage::Header {
            stream_id,
            header: Box::new(header),
        })
        .await
        .is_err()
    {
        return;
    }

    // SUBGROUP_OBJECT を順にデコードする
    loop {
        let object = match decoder.try_decode_object() {
            Ok(Some(object)) => object,
            Ok(None) => match read_chunk(&mut stream, &mut buf, &token).await {
                Ok(Some(size)) => {
                    decoder.push(&buf[..size]);
                    continue;
                }
                Ok(None) => {
                    let _ = tx.send(DataMessage::Closed { stream_id }).await;
                    return;
                }
                Err(e) => {
                    let _ = tx
                        .send(DataMessage::Failed {
                            stream_id,
                            reason: format!("読み込みに失敗しました: {e}"),
                        })
                        .await;
                    return;
                }
            },
            Err(e) => {
                let _ = tx
                    .send(DataMessage::Failed {
                        stream_id,
                        reason: format!("SUBGROUP_OBJECT のデコードに失敗しました: {e:?}"),
                    })
                    .await;
                return;
            }
        };

        // payload を読み出す (データが揃うまで読み込みを続ける)
        let payload = loop {
            if let Some(payload) = decoder.try_read_payload() {
                break payload;
            }
            match read_chunk(&mut stream, &mut buf, &token).await {
                Ok(Some(size)) => decoder.push(&buf[..size]),
                Ok(None) => {
                    let _ = tx.send(DataMessage::Closed { stream_id }).await;
                    return;
                }
                Err(e) => {
                    let _ = tx
                        .send(DataMessage::Failed {
                            stream_id,
                            reason: format!("読み込みに失敗しました: {e}"),
                        })
                        .await;
                    return;
                }
            }
        };

        let payload_length = payload.len() as u64;
        // zakuro-moq の publisher は payload を `位置 % 251` のパターンで埋める。
        // 検査するのは `--verify-payload` 指定時だけにする (実メディアを配信する relay に
        // 接続したときに誤検知しないため)
        let payload_ok = !verify_payload || payload_matches_pattern(&payload);
        if tx
            .send(DataMessage::Object {
                stream_id,
                object: Box::new(object),
                payload_length,
                payload_ok,
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

/// 受信 payload が zakuro-moq の publisher のパターン (`位置 % 251`) と一致するか
fn payload_matches_pattern(payload: &[u8]) -> bool {
    payload
        .iter()
        .enumerate()
        .all(|(i, byte)| *byte == (i % 251) as u8)
}

/// stream から 1 チャンク読む (キャンセル時は `Ok(None)`)
async fn read_chunk(
    stream: &mut ReceiveStream,
    buf: &mut [u8],
    token: &CancellationToken,
) -> std::io::Result<Option<usize>> {
    let size = tokio::select! {
        biased;
        _ = token.cancelled() => return Ok(None),
        read = stream.read(buf) => read?,
    };
    if size == 0 {
        return Ok(None);
    }
    Ok(Some(size))
}

/// `Option<Instant>` まで待つ future
///
/// `None` のときは完了しない (`pending`)。tokio の select! の分岐に置くことで、
/// 「送信予定が無いときは発火しない」を分岐の追加なしに表現できる。
async fn sleep_until_optional(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// QUIC の application error code を `s2n_quic` の型へ変換する
fn application_error(error_code: u64) -> s2n_quic::application::Error {
    s2n_quic::application::Error::new(error_code).unwrap_or(s2n_quic::application::Error::UNKNOWN)
}

/// SETUP メッセージを取り出す
fn take_setup_message(session: &mut Session) -> Result<ControlMessage> {
    match session.poll_event() {
        Some(SessionEvent::SendControl(message)) => Ok(message),
        _ => Err(ErrorMessage::new("MOQT セッションが SETUP を生成しませんでした").into()),
    }
}

/// SETUP Options を構築する
///
/// PATH (0x01) と AUTHORITY (0x05) は native QUIC のときだけ載せる
/// (draft-ietf-moq-transport-21 §9.1.1 / §9.1.2: WebTransport では MUST NOT)。
fn build_setup_options(config: &SessionConfig<'_>) -> SetupOptions {
    let mut options = SetupOptions::new();
    options.push(SetupOption {
        option_type: SETUP_OPTION_PATH,
        value: SetupOptionValue::Bytes(config.path.as_bytes().to_vec()),
    });
    options.push(SetupOption {
        option_type: SETUP_OPTION_AUTHORITY,
        value: SetupOptionValue::Bytes(config.authority.as_bytes().to_vec()),
    });
    options.push(SetupOption {
        option_type: SETUP_OPTION_MOQT_IMPLEMENTATION,
        value: SetupOptionValue::Bytes(MOQT_IMPLEMENTATION_NAME.to_vec()),
    });
    options
}

/// object に載せる LOC プロパティを構築する
///
/// draft-ietf-moq-loc-04 §2.3.1 (Timestamp) / §2.3.2 (Timescale)。payload は実メディアでは
/// ないが、media track として扱えるよう時刻情報を付ける。
fn build_loc_properties(sent_objects: u64, object_rate: f64) -> LocProperties {
    let timestamp = (sent_objects as f64 / object_rate * LOC_TIMESCALE as f64) as u64;
    let mut properties = LocProperties::new();
    properties.push(LocProperty {
        prop_id: PROP_TIMESCALE,
        value: LocPropertyValue::VarInt(LOC_TIMESCALE),
    });
    properties.push(LocProperty {
        prop_id: PROP_TIMESTAMP,
        value: LocPropertyValue::VarInt(timestamp),
    });
    properties
}

/// peer の制御ストリームを受け取り、その stream type と読み込み用デコーダを返す
///
/// peer が最初に開く uni stream が制御ストリームであるとは限らないため、制御ストリーム
/// (stream type `SETUP_STREAM_TYPE`) が現れるまで受け取りを続ける。デコーダは stream ごとに
/// 新しく作る (`MessageDecoder` は varint の後ろに読んだバイトを内部に保持するため、
/// stream 間で共有すると前の stream の残骸を次の stream type として誤って解釈する)。
async fn accept_control_stream(
    acceptor: &mut s2n_quic::connection::ReceiveStreamAcceptor,
    token: &CancellationToken,
) -> Result<(ReceiveStream, u64, MessageDecoder)> {
    loop {
        let mut stream = tokio::select! {
            biased;
            _ = token.cancelled() => {
                return Err(ErrorMessage::new("制御ストリームの受信前にキャンセルされました").into());
            }
            accepted = acceptor.accept_receive_stream() => {
                match accepted {
                    Ok(Some(stream)) => stream,
                    Ok(None) => {
                        return Err(ErrorMessage::new("制御ストリームを受信する前に接続が閉じられました").into());
                    }
                    Err(e) => {
                        return Err(ErrorMessage::new(format!("uni stream の受信に失敗しました: {e}")).into());
                    }
                }
            }
        };

        let mut decoder = MessageDecoder::new();
        let stream_type = read_stream_type(&mut stream, &mut decoder).await?;
        tracing::info!("MOQT peer uni stream opened: stream_type={stream_type:#x}");
        if stream_type == shiguredo_moqt::stream::SETUP_STREAM_TYPE {
            return Ok((stream, stream_type, decoder));
        }
        // 制御ストリーム以外の uni stream (padding 等) は publisher 専用クライアントでは
        // 扱わないため閉じる
        drop(stream);
    }
}

/// uni stream の先頭から stream type varint を読む
async fn read_stream_type(stream: &mut ReceiveStream, decoder: &mut MessageDecoder) -> Result<u64> {
    let mut buf = [0u8; 64];
    for _ in 0..MAX_STREAM_TYPE_READS {
        if let Some(stream_type) = decoder.try_decode_varint().map_err(|e| {
            ErrorMessage::new(format!("stream type のデコードに失敗しました: {e:?}"))
        })? {
            return Ok(stream_type);
        }
        let size = stream
            .read(&mut buf)
            .await
            .map_err(|e| ErrorMessage::new(format!("stream type の読み込みに失敗しました: {e}")))?;
        if size == 0 {
            return Err(ErrorMessage::new("stream type を読む前に stream が閉じられました").into());
        }
        decoder.push(&buf[..size]);
    }
    Err(ErrorMessage::new("stream type の varint が 8 バイトを超えました").into())
}

/// 制御ストリームから読めたメッセージをすべて `Session` へ渡す
fn recv_control_messages(session: &mut Session, decoder: &mut MessageDecoder) -> Result<()> {
    loop {
        match decoder.try_decode_message().map_err(|e| {
            ErrorMessage::new(format!("制御メッセージのデコードに失敗しました: {e:?}"))
        })? {
            Some(message) => {
                session.recv_control(message).map_err(|e| {
                    ErrorMessage::new(format!("制御メッセージの受理に失敗しました: {e:?}"))
                })?;
            }
            None => return Ok(()),
        }
    }
}

/// `Session` のイベントを処理し、ワイヤへの書き込みと状態更新を行う
///
/// `publishers` を渡した場合、PUBLISH の受理を該当トラックへ反映する。
/// `SendRequest` で開いた request stream の受信方向は、その場で読みタスクへ渡す。
async fn drain_events(
    session: &mut Session,
    state: &mut SessionState,
    mut publishers: Option<&mut Vec<Publisher>>,
    mut subscriptions: Option<&mut Vec<SubscriptionState>>,
    request_tx: &mpsc::Sender<RequestMessage>,
    token: &CancellationToken,
) -> Result<()> {
    while let Some(event) = session.poll_event() {
        match event {
            SessionEvent::SendControl(message) => {
                // SETUP は初期化時に送り終えているため、ここへ来るのは GOAWAY であり、
                // stream type を前置しない (draft-ietf-moq-transport-21 §6.4.1)
                let bytes = message.encode().map_err(|e| {
                    ErrorMessage::new(format!("制御メッセージのエンコードに失敗しました: {e:?}"))
                })?;
                state.control_send.write_all(&bytes).await.map_err(|e| {
                    ErrorMessage::new(format!("制御メッセージの送信に失敗しました: {e}"))
                })?;
            }
            SessionEvent::SendRequest {
                request_id,
                message,
            } => {
                let stream = state
                    .connection_handle
                    .open_bidirectional_stream()
                    .await
                    .map_err(|e| {
                        ErrorMessage::new(format!("request stream の open に失敗しました: {e}"))
                    })?;
                let (recv, mut send) = stream.split();
                send.write_all(&message.encode().map_err(|e| {
                    ErrorMessage::new(format!(
                        "request メッセージのエンコードに失敗しました: {e:?}"
                    ))
                })?)
                .await
                .map_err(|e| {
                    ErrorMessage::new(format!("request メッセージの送信に失敗しました: {e}"))
                })?;
                state.request_sends.insert(request_id, send);
                // 受信方向はストリームごとのタスクで読む
                let tx = request_tx.clone();
                let reader_token = token.child_token();
                tokio::spawn(async move {
                    run_request_reader(request_id, recv, tx, reader_token).await;
                });
            }
            SessionEvent::SendOnStream {
                request_id,
                message,
                fin,
            } => {
                let Some(send) = state.request_sends.get_mut(&request_id) else {
                    tracing::warn!(
                        "MOQT: no request stream for request_id={request_id} (message is dropped)",
                    );
                    continue;
                };
                send.write_all(&message.encode().map_err(|e| {
                    ErrorMessage::new(format!(
                        "request メッセージのエンコードに失敗しました: {e:?}"
                    ))
                })?)
                .await
                .map_err(|e| {
                    ErrorMessage::new(format!("request メッセージの送信に失敗しました: {e}"))
                })?;
                if fin {
                    let _ = send.finish();
                }
            }
            SessionEvent::Established => {
                tracing::info!("MOQT session established");
                state.established = true;
                if let Some(notify) = state.established_notify.take() {
                    let _ = notify.send(());
                }
            }
            SessionEvent::RequestOkReceived {
                request_id,
                request_kind,
                ..
            } => {
                // SUBSCRIBE_OK は Track Alias が確定するため、受信した object の帰属判定に
                // 使えるよう購読状態へ記録する
                if request_kind == RequestKind::Subscribe {
                    let alias = session
                        .subscription(request_id)
                        .and_then(|subscription| subscription.track_alias);
                    if let Some(subscriptions) = subscriptions.as_deref_mut() {
                        for subscription in subscriptions.iter_mut() {
                            if subscription.request_id == Some(request_id) && !subscription.accepted
                            {
                                subscription.accepted = true;
                                subscription.track_alias = alias;
                                tracing::info!(
                                    "MOQT subscribe accepted: track={} request_id={} track_alias={:?}",
                                    subscription.name,
                                    request_id,
                                    alias,
                                );
                            }
                        }
                    }
                    continue;
                }
                if request_kind != RequestKind::Publish {
                    continue;
                }
                if let Some(publishers) = publishers.as_deref_mut() {
                    for publisher in publishers.iter_mut() {
                        if publisher.request_id == Some(request_id) && !publisher.accepted {
                            // REQUEST_UPDATE への応答でも同じ request_id で届くため、
                            // 初回の受理だけを記録する
                            publisher.on_accepted(request_id);
                            tracing::info!(
                                "MOQT publish accepted: track={} request_id={}",
                                publisher.config.name,
                                request_id,
                            );
                        }
                    }
                }
            }
            SessionEvent::RequestUpdateReceived {
                request_id,
                parameters,
            } => {
                // draft-ietf-moq-transport-21 §9.5 (REQUEST_UPDATE): 受信側は REQUEST_OK または
                // REQUEST_ERROR を 1 通返す MUST。FORWARD の反映は session 層が済ませている
                // FORWARD は request (トラック) 単位の値であるため、該当トラックだけに適用する。
                // セッション全体で 1 つ持つと、購読者のいないトラックへの FORWARD=0 で
                // 他のトラックまで止まってしまう
                if let Some(forward) = parameters.forward() {
                    tracing::info!(
                        "MOQT: REQUEST_UPDATE request_id={} forward={}",
                        request_id,
                        forward,
                    );
                    if let Some(publishers) = publishers.as_deref_mut() {
                        for publisher in publishers.iter_mut() {
                            if publisher.request_id == Some(request_id) {
                                publisher.on_forward_changed(forward != 0);
                            }
                        }
                    }
                }
                session
                    .send_request_ok(request_id, MessageParameters::new(), TrackProperties::new())
                    .map_err(|e| {
                        ErrorMessage::new(format!("REQUEST_UPDATE への応答に失敗しました: {e:?}"))
                    })?;
            }
            SessionEvent::ResetDataStream {
                stream_id,
                error_code,
                ..
            } => {
                state.reset_data_streams.push((stream_id, error_code));
            }
            SessionEvent::ResetRequestStream {
                request_id,
                error_code,
            } => {
                if let Some(send) = state.request_sends.get_mut(&request_id) {
                    let _ = send.reset(application_error(error_code));
                }
                tracing::warn!("MOQT: request stream was reset by peer: request_id={request_id}");
            }
            SessionEvent::FinishRequestStream { request_id } => {
                if let Some(send) = state.request_sends.get_mut(&request_id) {
                    let _ = send.finish();
                }
            }
            SessionEvent::StopSendingRequestStream { request_id, .. } => {
                // 自側が受け取る request stream は読みタスクが保持しているため何もしない
                tracing::warn!("MOQT: STOP_SENDING for request_id={request_id} is ignored");
            }
            SessionEvent::GoawayReceived { timeout, .. } => {
                tracing::warn!("MOQT: GOAWAY received (timeout={timeout}ms)");
            }
            SessionEvent::RequestErrorReceived {
                request_id,
                error_code,
                reason,
                ..
            } => {
                // PUBLISH / SUBSCRIBE のどちらが拒否されたか分かるようにする
                // (relay は購読者のいないトラックへの PUBLISH や、publisher のいないトラックへの
                // SUBSCRIBE を REQUEST_ERROR で拒否する)
                let kind = if subscriptions
                    .as_deref()
                    .is_some_and(|subs| subs.iter().any(|s| s.request_id == Some(request_id)))
                {
                    "subscribe"
                } else {
                    "publish"
                };
                state.closed_reason = Some(format!(
                    "{kind} rejected: request_id={request_id} error_code={error_code:#x} reason={reason:?}"
                ));
            }
            SessionEvent::RequestTerminated {
                request_id, reason, ..
            } => {
                state.closed_reason = Some(format!(
                    "request terminated: request_id={request_id} {reason:?}"
                ));
            }
            SessionEvent::PublishDoneReceived { request_id, .. } => {
                state.closed_reason =
                    Some(format!("PUBLISH_DONE received: request_id={request_id}"));
            }
            SessionEvent::CloseSession(error) => {
                // `CloseSession` は「I/O 層が接続を閉じる」指示である
                state.connection_handle.close(application_error(error.code));
                state.closed_reason = Some(format!("{:#x} {}", error.code, error.reason));
            }
            // publisher 専用クライアントでは扱わないイベント
            SessionEvent::FetchOkReceived { .. }
            | SessionEvent::PublishStateNotifyReceived { .. }
            | SessionEvent::SendPaddingStream { .. }
            | SessionEvent::SendPaddingDatagram { .. }
            | SessionEvent::OpenFillFetchStream { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 受信 payload のパターン検査が publisher の埋め方と一致すること
    #[test]
    fn payload_pattern_check_accepts_publisher_payload() {
        // publisher は `位置 % 251` で埋める
        let payload: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        assert!(payload_matches_pattern(&payload));

        // 251 をまたぐ位置でも検査できること
        let long: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
        assert!(payload_matches_pattern(&long));
    }

    /// 受信 payload が壊れていれば検出できること
    #[test]
    fn payload_pattern_check_detects_mismatch() {
        let mut payload: Vec<u8> = (0..100).map(|i| (i % 251) as u8).collect();
        payload[99] = 0;
        assert!(!payload_matches_pattern(&payload));

        // 空 payload は不一致ではない
        assert!(payload_matches_pattern(&[]));
    }

    /// 最初の object は絶対 ID を Delta として送ること
    #[test]
    fn object_id_delta_uses_absolute_id_for_the_first_object() {
        assert_eq!(object_id_delta(None, 0), 0);
        assert_eq!(object_id_delta(None, 7), 7);
    }

    /// 2 つ目以降は直前の ID との差から 1 を引いた値を送ること
    #[test]
    fn object_id_delta_subtracts_one_for_later_objects() {
        assert_eq!(object_id_delta(Some(0), 1), 0);
        assert_eq!(object_id_delta(Some(4), 5), 0);
    }

    /// 送らなかった object があっても Delta がその分を吸収すること
    #[test]
    fn object_id_delta_absorbs_skipped_objects() {
        assert_eq!(object_id_delta(Some(2), 5), 2);
    }

    /// SETUP Options に PATH と AUTHORITY が載ること
    #[test]
    fn setup_options_include_path_and_authority() {
        let tracks = vec![TrackConfig {
            name: "video-0-0".to_string(),
            object_rate: 30.0,
            object_size: 1000,
        }];
        let config = SessionConfig {
            namespace: "zakuro",
            tracks: &tracks,
            authority: "relay.example.com:4433",
            path: "/",
            payload: vec![0u8; 1000].into(),
            subscribe_tracks: &[],
            verify_payload: false,
        };
        let options = build_setup_options(&config);
        assert_eq!(options.path(), Some(b"/".as_slice()), "PATH が載っていない");
        assert_eq!(
            options.authority(),
            Some(b"relay.example.com:4433".as_slice()),
            "AUTHORITY が載っていない"
        );
    }

    /// LOC プロパティに Timescale と Timestamp が載ること
    #[test]
    fn loc_properties_include_timestamp_and_timescale() {
        let properties = build_loc_properties(3, 30.0);
        assert_eq!(properties.timescale(), Some(LOC_TIMESCALE));
        assert_eq!(properties.timestamp(), Some(100_000));
    }

    /// Timestamp は object 数とともに単調増加すること
    #[test]
    fn loc_timestamp_increases_with_objects() {
        let first = build_loc_properties(0, 30.0)
            .timestamp()
            .expect("Timestamp が載っていること");
        let second = build_loc_properties(1, 30.0)
            .timestamp()
            .expect("Timestamp が載っていること");
        assert!(
            first < second,
            "Timestamp が増加していない: {first} -> {second}"
        );
    }

    /// トラックごとに周期と Group あたりの object 数が決まること
    #[test]
    fn publisher_period_follows_object_rate() {
        let now = tokio::time::Instant::now();
        let publisher = Publisher::new(
            TrackConfig {
                name: "video".to_string(),
                object_rate: 40.0,
                object_size: 100,
            },
            1,
            now,
        );
        assert_eq!(publisher.period, Duration::from_millis(25));
        assert_eq!(publisher.objects_per_group, 40);
    }

    /// Forward State がトラックごとに独立していること
    ///
    /// FORWARD は request 単位の値であり、購読者のいないトラックへの FORWARD=0 が
    /// 他のトラックの送信を止めてはならない。
    #[test]
    fn forward_state_is_per_track() {
        let now = tokio::time::Instant::now();
        let track = |name: &str| TrackConfig {
            name: name.to_string(),
            object_rate: 30.0,
            object_size: 100,
        };
        let mut video = Publisher::new(track("video"), 1, now);
        let mut audio = Publisher::new(track("audio"), 2, now);
        video.on_accepted(0);
        audio.on_accepted(2);

        // audio 宛の REQUEST_UPDATE (FORWARD=0) は audio だけを止める
        audio.on_forward_changed(false);

        assert!(
            !audio.is_due(now),
            "FORWARD=0 のトラックは送信対象にならないこと"
        );
        assert!(
            audio.deadline().is_none(),
            "FORWARD=0 のトラックは送信予定を持たないこと"
        );
        assert!(
            video.deadline().is_some(),
            "他のトラックの送信予定が消えてはならない"
        );
        assert!(
            video.is_due(tokio::time::Instant::now()),
            "他のトラックは送信対象のままであること"
        );

        // FORWARD=1 に戻ると送信対象へ復帰する
        audio.on_forward_changed(true);
        assert!(audio.deadline().is_some(), "FORWARD=1 で送信予定が戻ること");
    }

    /// フィルタ不通過でも次の送信時刻が進むこと
    ///
    /// 進めないと直後のループで再び期日到来となり、sleep せずに回り続ける。
    #[test]
    fn advance_next_send_moves_deadline_forward() {
        let now = tokio::time::Instant::now();
        let mut publisher = Publisher::new(
            TrackConfig {
                name: "video".to_string(),
                object_rate: 30.0,
                object_size: 100,
            },
            1,
            now,
        );
        publisher.on_accepted(0);
        let before = publisher.next_send;
        publisher.advance_next_send();
        assert!(
            publisher.next_send > before,
            "次の送信時刻が進んでいない: {before:?} -> {:?}",
            publisher.next_send
        );
        assert!(!publisher.is_due(before), "進めた分だけ期日が先になること");
    }

    /// 受理前は送信予定が無く、受理後に送信対象になること
    #[test]
    fn publisher_deadline_requires_acceptance() {
        let now = tokio::time::Instant::now();
        let mut publisher = Publisher::new(
            TrackConfig {
                name: "video".to_string(),
                object_rate: 30.0,
                object_size: 100,
            },
            1,
            now,
        );
        assert!(publisher.deadline().is_none(), "受理前は送信予定が無いこと");
        publisher.on_accepted(0);
        assert!(publisher.accepted);
        assert!(publisher.deadline().is_some(), "受理後は送信予定があること");
        publisher.on_forward_changed(false);
        assert!(
            publisher.deadline().is_none(),
            "Forward State 0 では送信予定が無いこと"
        );
    }
}
