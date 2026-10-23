//! ngtcp2 の QUIC 接続と HTTP/3 / WebTransport を結線する
//!
//! HTTP/3 と Capsule Protocol は http3-rs が処理する。受信ヘッダーの消費と
//! アプリケーションによる本文の消費を分け、QUIC と WT の両方へ受信枠を返す。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use super::{ALPN, Connection, Driver, MoqTlsOptions, Received, StreamReset, closed, io_error};
use crate::error::Result;
use shiguredo_http3::WebTransportEvent as WtEvent;
use shiguredo_http3::webtransport::{
    ApplicationErrorCode, Capsule, ConnectRequest, FlowControlLimits, Session, StreamHeader,
};
use shiguredo_ngtcp2_tokio::{Client, ClientConfig, ClientConnection, ConnectionEvent};

use shiguredo_http3::{ClientConnection as H3Client, Event, Settings as H3Settings, VarInt};

#[cfg(test)]
mod tests;

/// HTTP/3 の ALPN
const H3_ALPN: &[u8] = b"h3";

/// HTTP/3 用の証明書検証と QUIC transport parameter を設定する
pub(crate) fn build_config(options: &MoqTlsOptions) -> Result<ClientConfig> {
    super::build_config_with_alpn(options, H3_ALPN)
}

/// TLS と WebTransport の CONNECT 交渉を完了してから MOQT へ接続を渡す
pub(crate) async fn connect(
    remote: SocketAddr,
    local: SocketAddr,
    server_name: &str,
    config: &ClientConfig,
    authority: &str,
    path: &str,
) -> Result<Connection> {
    let mut connection = Client::connect_with_config(remote, local, server_name, config)
        .await
        .map_err(io_error)?;
    if connection.selected_alpn_protocol().as_deref() != Some(H3_ALPN) {
        return Err(io_error("peer did not select h3 ALPN").into());
    }
    let mut state = State::new(&mut connection)?;
    let mut requested = false;
    let mut established = false;
    // 上位の接続タイムアウトが SETTINGS と CONNECT の待ち時間も制限する。
    loop {
        if !requested && let Some(settings) = state.h3.peer_settings() {
            let draft = settings
                .webtransport_draft_pattern()
                .ok_or_else(|| io_error("peer did not enable WebTransport"))?;
            let wt = settings
                .wt_settings
                .as_ref()
                .ok_or_else(|| io_error("peer did not advertise WebTransport SETTINGS"))?;
            *state.session.remote_limits_mut() = FlowControlLimits {
                max_streams_uni: wt.wt_initial_max_streams_uni.get(),
                max_streams_bidi: wt.wt_initial_max_streams_bidi.get(),
                max_data: wt.wt_initial_max_data.get(),
            };
            let headers = ConnectRequest::new("https", authority, path)
                .draft_version(draft)
                .available_protocols(vec![String::from_utf8_lossy(ALPN).into_owned()])
                .to_headers()
                .map_err(io_error)?;
            let id = connection.open_bidi_stream().map_err(io_error)?;
            let session_id = state.h3.send_request(&headers, false).map_err(h3_error)?;
            if id as u64 != session_id {
                return Err(io_error("HTTP/3 and QUIC request stream IDs differ").into());
            }
            state.session = {
                let limits = *state.session.remote_limits();
                let mut session = Session::new(session_id);
                *session.remote_limits_mut() = limits;
                session
            };
            state.protocol_streams.insert(id);
            requested = true;
        }
        while let Some(event) = state.h3.poll_event().map_err(h3_error)? {
            match event {
                Event::Header { name, value, .. } if name == b":status" => {
                    let status = std::str::from_utf8(&value)
                        .ok()
                        .and_then(|s| s.parse::<u16>().ok());
                    if status.is_some_and(|s| s >= 200)
                        && !status.is_some_and(|s| (200..300).contains(&s))
                    {
                        return Err(io_error(format!(
                            "WebTransport CONNECT rejected: {}",
                            String::from_utf8_lossy(&value)
                        ))
                        .into());
                    }
                }
                Event::WebTransport(WtEvent::SessionEstablished {
                    flow_control_enabled,
                    ..
                }) => {
                    state.session.set_flow_control_enabled(flow_control_enabled);
                    state.flow_control_enabled = flow_control_enabled;
                    state.session.set_established();
                    established = true;
                }
                Event::WebTransport(WtEvent::SessionClosed {
                    error_code,
                    close_error_code,
                    close_message,
                    ..
                }) => {
                    return Err(io_error(format!("WebTransport CONNECT closed: {error_code:#x}, {close_error_code}: {close_message}")).into());
                }
                Event::StreamEnd { .. } | Event::StreamReset { .. } | Event::StopSending { .. } => {
                    return Err(io_error("WebTransport CONNECT ended before establishment").into());
                }
                event => state.pending.push_back(event),
            }
        }
        state.flush(&mut connection)?;
        connection.flush().await.map_err(io_error)?;
        if established {
            return Ok(Connection::with_webtransport(connection, Some(state)));
        }
        let event = connection.recv_event().await.map_err(io_error)?;
        if !state.input(&mut connection, &event)? {
            return Err(closed().into());
        }
    }
}

/// 受信先頭の分類が終わるまでは最大 16 バイトだけ保持する
struct Prefix {
    data: Vec<u8>,
    complete: bool,
}

/// 1 QUIC 接続に 1 WebTransport セッションを載せる
pub(super) struct State {
    pub(super) h3: H3Client,
    /// 送信側の WT フロー制御。受信側は h3 が管理する
    pub(super) session: Session,
    pub(super) draining: bool,
    protocol_streams: HashSet<i64>,
    prefixes: HashMap<i64, Prefix>,
    pending: VecDeque<Event>,
    /// 古い draft では WT データ量の制限が無効になる
    flow_control_enabled: bool,
}

impl State {
    fn new(connection: &mut ClientConnection) -> io::Result<Self> {
        let wt = shiguredo_http3::webtransport::Settings::new()
            .wt_enabled(VarInt::from_static(1))
            .wt_initial_max_streams_uni(VarInt::from_static(10_000))
            .wt_initial_max_streams_bidi(VarInt::from_static(1_000))
            .wt_initial_max_data(VarInt::from_static(64 * 1024 * 1024))
            .enable_webtransport_draft02(true)
            .webtransport_max_sessions_draft07(VarInt::from_static(1))
            .wt_max_sessions_draft14(VarInt::from_static(1));
        let mut h3 = H3Client::new(H3Settings::default().enable_webtransport_client(wt));
        h3.set_webtransport_transport_verified(
            connection.local_max_datagram_frame_size() > 0
                && connection.remote_max_datagram_frame_size() > 0,
            connection.supports_reset_stream_at(),
        )
        .map_err(h3_error)?;
        let control = connection.open_uni_stream().map_err(io_error)?;
        let encoder = connection.open_uni_stream().map_err(io_error)?;
        let decoder = connection.open_uni_stream().map_err(io_error)?;
        let init = h3
            .init_h3_streams(control as u64, encoder as u64, decoder as u64)
            .map_err(h3_error)?;
        for (id, data) in [
            (control, init.control_data),
            (encoder, init.encoder_data),
            (decoder, init.decoder_data),
        ] {
            connection
                .write_stream(id, &data, false)
                .map_err(io_error)?;
        }
        Ok(Self {
            h3,
            session: Session::new(0),
            draining: false,
            protocol_streams: HashSet::new(),
            prefixes: HashMap::new(),
            pending: VecDeque::new(),
            flow_control_enabled: false,
        })
    }

    /// 現在の WT_MAX_DATA で送れる本文の長さを求める
    pub(super) fn available_data(&self, requested: usize) -> usize {
        if !self.flow_control_enabled {
            return requested;
        }
        self.session
            .remote_limits()
            .max_data
            .saturating_sub(self.session.flow_state().data_sent)
            .min(requested as u64) as usize
    }

    /// ローカル開始ストリームは WT ヘッダーを最初に送り、本文とは別に計上する
    pub(super) fn open_stream(
        &mut self,
        connection: &mut ClientConnection,
        id: i64,
        bidi: bool,
    ) -> io::Result<()> {
        self.h3
            .register_local_wt_stream(self.session.session_id(), id as u64)
            .map_err(h3_error)?;
        let header = StreamHeader::new(self.session.session_id())
            .map_err(|e| io_error(format!("invalid WebTransport session ID: {e:?}")))?;
        let mut data = Vec::new();
        if bidi {
            header.encode_bidirectional(&mut data);
            // ローカル開始 bidi の返信方向には WT ヘッダーが付かない。
            self.prefixes.insert(
                id,
                Prefix {
                    data: Vec::new(),
                    complete: true,
                },
            );
        } else {
            header.encode_unidirectional(&mut data);
        }
        connection
            .write_stream(id, &data, false)
            .map_err(io_error)?;
        Ok(())
    }

    /// QUIC の全イベントを HTTP/3 へ渡す。接続終了だけ false を返す
    fn input(
        &mut self,
        connection: &mut ClientConnection,
        event: &ConnectionEvent,
    ) -> io::Result<bool> {
        match event {
            ConnectionEvent::StreamData {
                stream_id,
                data,
                fin,
            } => {
                self.h3
                    .feed_stream(*stream_id as u64, data, *fin)
                    .map_err(h3_error)?;
                let credit = self.protocol_credit(*stream_id, data);
                if credit > 0 {
                    connection
                        .extend_max_stream_offset(*stream_id, credit)
                        .map_err(io_error)?;
                }
            }
            ConnectionEvent::StreamReset {
                stream_id,
                app_error_code,
                final_size,
            } => {
                self.h3
                    .stream_reset(*stream_id as u64, *app_error_code, *final_size)
                    .map_err(h3_error)?;
            }
            ConnectionEvent::StreamStopSending {
                stream_id,
                app_error_code,
            } => {
                self.h3
                    .stop_sending(*stream_id as u64, *app_error_code)
                    .map_err(h3_error)?;
            }
            ConnectionEvent::Datagram { data } => self.h3.feed_datagram(data).map_err(h3_error)?,
            ConnectionEvent::StreamClosed { stream_id, .. } => {
                self.prefixes.remove(stream_id);
                self.protocol_streams.remove(stream_id);
            }
            ConnectionEvent::ConnectionClosed { .. } => return Ok(false),
            _ => {}
        }
        Ok(true)
    }

    /// 本文だけを読み手に残し、HTTP/3 と WT ヘッダーの枠をすぐ返す
    fn protocol_credit(&mut self, id: i64, data: &[u8]) -> u64 {
        if self.protocol_streams.contains(&id) {
            return data.len() as u64;
        }
        let prefix = self.prefixes.entry(id).or_insert_with(|| Prefix {
            data: Vec::new(),
            complete: false,
        });
        if prefix.complete {
            return 0;
        }
        let before = prefix.data.len();
        prefix
            .data
            .extend(data.iter().take(16usize.saturating_sub(before)));
        let Ok((kind, len)) = shiguredo_http3::varint::decode(&prefix.data) else {
            return 0;
        };
        let wt_kind = if id & 2 != 0 { 0x54 } else { 0x41 };
        if kind.get() != wt_kind {
            self.protocol_streams.insert(id);
            self.prefixes.remove(&id);
            return (before + data.len()) as u64;
        }
        let Ok((_, session_len)) = shiguredo_http3::varint::decode(&prefix.data[len..]) else {
            return 0;
        };
        prefix.complete = true;
        prefix.data.clear();
        (len + session_len) as u64
    }

    /// h3 の送信データと送受信の WT フロー制御カプセルを QUIC へ渡す
    fn flush(&mut self, connection: &mut ClientConnection) -> io::Result<()> {
        let id = self.session.session_id();
        let capsules = self
            .h3
            .take_wt_pending_capsules(id)
            .into_iter()
            .chain(self.session.take_pending_capsules());
        for capsule in capsules {
            let mut data = Vec::new();
            capsule.encode(&mut data);
            self.h3.send_body(id, &data, false).map_err(h3_error)?;
        }
        let ids: Vec<_> = self.h3.writable_streams().collect();
        for id in ids {
            while let Some((data, fin)) = self.h3.take_stream_data(id) {
                connection
                    .write_stream(id as i64, &data, fin)
                    .map_err(io_error)?;
            }
        }
        Ok(())
    }
}

impl Driver {
    /// WT 経路だけは HTTP/3 を通す。StreamClosed の QUIC 枠返却は共通処理に任せる
    pub(super) fn transport_event(&mut self, event: ConnectionEvent) -> io::Result<bool> {
        if let Some(wt) = &mut self.wt {
            if !wt.input(&mut self.connection, &event)? {
                return self.event(event);
            }
            if matches!(event, ConnectionEvent::StreamClosed { .. }) {
                return self.event(event);
            }
            Ok(true)
        } else {
            self.event(event)
        }
    }

    pub(super) fn flush_webtransport(&mut self) -> io::Result<()> {
        if let Some(wt) = &mut self.wt {
            wt.flush(&mut self.connection)?;
        }
        Ok(())
    }

    /// 送信許可が増えたら再試行する。ストリームごとの要求順は呼び出し側が維持する
    pub(super) async fn retry_blocked_writes(&mut self) -> io::Result<()> {
        for _ in 0..self.blocked_writes.len() {
            let command = self
                .blocked_writes
                .pop_front()
                .expect("保留した送信要求があること");
            self.command(command).await?;
        }
        Ok(())
    }

    pub(super) fn wire_error_code(&self, code: u64) -> io::Result<u64> {
        if self.wt.is_none() {
            return Ok(code);
        }
        let code = u32::try_from(code)
            .map_err(|_| io_error("WebTransport application error code exceeds u32"))?;
        Ok(ApplicationErrorCode::to_http3_code(code))
    }

    /// HTTP/3 イベントを既存の MOQT ストリーム API へ変換する
    pub(super) fn drain_webtransport(&mut self) -> io::Result<()> {
        loop {
            let event = match &mut self.wt {
                None => return Ok(()),
                Some(wt) => match wt.pending.pop_front() {
                    Some(event) => Some(event),
                    None => wt.h3.poll_event().map_err(h3_error)?,
                },
            };
            let Some(event) = event else {
                return Ok(());
            };
            match event {
                Event::WebTransport(
                    WtEvent::BidiStreamOpen { stream_id, .. }
                    | WtEvent::UniStreamOpen { stream_id, .. },
                ) => {
                    self.event(ConnectionEvent::StreamOpened {
                        stream_id: stream_id as i64,
                    })?;
                }
                Event::WebTransport(
                    WtEvent::BidiStreamData { stream_id, data }
                    | WtEvent::UniStreamData { stream_id, data },
                ) => {
                    let id = stream_id as i64;
                    let size = data.len() as u64;
                    let delivered = self
                        .receivers
                        .get(&id)
                        .is_some_and(|tx| tx.send(Received::Data(data)).is_ok());
                    if !delivered {
                        self.command_consumed(id, size)?;
                    }
                }
                Event::WebTransport(
                    WtEvent::BidiStreamEnd { stream_id } | WtEvent::UniStreamEnd { stream_id },
                ) => {
                    if let Some(tx) = self.receivers.get(&(stream_id as i64)) {
                        let _ = tx.send(Received::Fin);
                    }
                }
                Event::WebTransport(WtEvent::StreamReset {
                    stream_id,
                    error_code,
                    ..
                }) => {
                    if let Some(tx) = self.receivers.remove(&(stream_id as i64)) {
                        let message = match ApplicationErrorCode::from_http3_code(error_code) {
                            Some(code) => Received::Reset(u64::from(code)),
                            None => Received::Failed(format!(
                                "WebTransport stream reset: {error_code:#x}"
                            )),
                        };
                        let _ = tx.send(message);
                    }
                }
                Event::WebTransport(WtEvent::StreamStopSending {
                    stream_id,
                    error_code,
                    ..
                }) => {
                    self.senders.remove(&(stream_id as i64));
                    let mut pending = Vec::new();
                    for (id, reply) in self.writes.drain(..) {
                        if id == stream_id as i64 {
                            let error = match ApplicationErrorCode::from_http3_code(error_code) {
                                Some(code) => io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    StreamReset(u64::from(code)),
                                ),
                                None => {
                                    io_error(format!("WebTransport STOP_SENDING: {error_code:#x}"))
                                }
                            };
                            let _ = reply.send(Err(error));
                        } else {
                            pending.push((id, reply));
                        }
                    }
                    self.writes = pending;
                }
                Event::WebTransport(WtEvent::SessionDraining { .. }) => {
                    if let Some(wt) = &mut self.wt {
                        wt.draining = true;
                    }
                }
                Event::WebTransport(WtEvent::Capsule { capsule, .. }) => {
                    if let Some(wt) = &mut self.wt {
                        wt.session
                            .process_capsule(&capsule)
                            .map_err(|e| io_error(format!("WebTransport capsule error: {e:?}")))?;
                    }
                }
                Event::WebTransport(WtEvent::SessionClosed {
                    reset_streams,
                    error_code,
                    close_error_code,
                    close_message,
                    ..
                }) => {
                    for reset in reset_streams {
                        let id = reset.stream_id as i64;
                        let _ = self.connection.reset_stream_reliable(id, error_code);
                        let _ = self.connection.stop_sending(id, error_code);
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        format!("WebTransport session closed: {close_error_code}: {close_message}"),
                    ));
                }
                Event::WebTransport(WtEvent::BufferedStreamRejected {
                    stream_id,
                    error_code,
                }) => {
                    let _ = self
                        .connection
                        .reset_stream_reliable(stream_id as i64, error_code);
                    let _ = self.connection.stop_sending(stream_id as i64, error_code);
                }
                Event::StreamReset { .. } | Event::StopSending { .. } | Event::StreamEnd { .. } => {
                    return Err(io_error("WebTransport CONNECT stream closed"));
                }
                // 負荷ツールの object は subgroup stream で運ぶ。データグラムの構文検証と
                // セッションへの関連付けは feed_datagram が行い、ここでは保持しない。
                _ => {}
            }
        }
    }

    fn command_consumed(&mut self, id: i64, size: u64) -> io::Result<()> {
        self.connection
            .extend_max_stream_offset(id, size)
            .map_err(io_error)?;
        if let Some(wt) = &mut self.wt {
            wt.h3.wt_data_consumed(wt.session.session_id(), size);
        }
        Ok(())
    }

    /// CLOSE_SESSION を FIN とともに送り、ピアの CONNECT 終了を待って接続を閉じる
    pub(super) async fn close_webtransport(&mut self, code: u64) -> io::Result<()> {
        let code = u32::try_from(code)
            .map_err(|_| io_error("WebTransport close error code exceeds u32"))?;
        if let Some(wt) = &mut self.wt {
            let mut data = Vec::new();
            Capsule::CloseSession {
                error_code: code,
                message: String::new(),
            }
            .encode(&mut data);
            wt.h3
                .send_body(wt.session.session_id(), &data, true)
                .map_err(h3_error)?;
            wt.flush(&mut self.connection)?;
        }
        // 送信キューの排出だけでは相手の受信を保証できない。CONNECT の返信方向の終了まで
        // 待ち、返信しない相手も 1 秒で打ち切る。受信中も ACK と再送タイマーを駆動する。
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            self.connection.flush().await.map_err(io_error)?;
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::select! {
                event = self.connection.recv_event() => {
                    let event = event.map_err(io_error)?;
                    let session_id = self.wt.as_ref().map(|wt| wt.session.session_id() as i64);
                    let connect_ended = match &event {
                        ConnectionEvent::StreamData { stream_id, fin: true, .. }
                        | ConnectionEvent::StreamReset { stream_id, .. }
                        | ConnectionEvent::StreamClosed { stream_id, .. } => Some(*stream_id) == session_id,
                        _ => false,
                    };
                    if !self.transport_event(event)? { return Ok(()); }
                    if connect_ended { break; }
                }
                _ = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
        }
        self.connection
            .close(0x100, b"WebTransport session closed")
            .await
            .map_err(io_error)
    }

    /// HTTP/3 の接続エラーでは規定のコードを通知して待機中のストリームを終了する
    pub(super) async fn close_failed_webtransport(&mut self, error: &io::Error) {
        if self.wt.is_none() {
            return;
        }
        let code = match error
            .get_ref()
            .and_then(|e| e.downcast_ref::<shiguredo_http3::Error>())
        {
            Some(
                shiguredo_http3::Error::ConnectionError(code)
                | shiguredo_http3::Error::StreamError(code),
            ) => code.code(),
            Some(shiguredo_http3::Error::Qpack(_)) => 0x200,
            Some(shiguredo_http3::Error::FrameDecode(_)) => 0x106,
            _ if error.kind() == io::ErrorKind::ConnectionAborted => 0x100,
            _ => 0x102,
        };
        let _ = self.connection.flush().await;
        let _ = self
            .connection
            .close(code, b"WebTransport session ended")
            .await;
    }
}

/// HTTP/3 のエラー型を保持し、接続終了コードを I/O タスクまで運ぶ
fn h3_error(error: shiguredo_http3::Error) -> io::Error {
    io::Error::other(error)
}
