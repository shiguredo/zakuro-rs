//! MOQ relay への接続 (QUIC 直接接続 / WebTransport over HTTP/3 / WebTransport over HTTP/2 / QMux)
//!
//! MOQT の接続先 URL の解釈、名前解決、TLS 設定、経路ごとのクライアント構築を担当する。
//! QUIC 直接接続は Cargo feature で選んだバックエンド (tobi-quic / s2n-quic / ngtcp2)、
//! WebTransport over HTTP/3 は tobi-webtransport、WebTransport over HTTP/2 と QMux は
//! TLS over TCP を使う。各経路を `session.rs` からは [`MoqConnection`] として同じように
//! 扱える。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;

use crate::error::{ErrorMessage, Result};

use super::stream::{MoqBidiAcceptor, MoqConnectionHandle, MoqDatagramReceiver, MoqRecvAcceptor};
use super::{qmux, quic, wt_h2, wt_h3};

/// 下位トランスポート種別
///
/// QUIC、WebTransport over HTTP/3、WebTransport over HTTP/2、QMux
/// (draft-ietf-quic-qmux-02) を選ぶ。URL はいずれも `moqt://` で、経路は CLI の
/// `--transport` で選ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MoqTransport {
    /// QUIC 直接接続
    #[default]
    Quic,
    /// tobi-webtransport による WebTransport over HTTP/3
    WtH3,
    /// WebTransport over HTTP/2 (TCP + TLS + shiguredo_http2)
    ///
    /// reset を `WT_RESET_STREAM` capsule で行うため `RESET_STREAM_AT` は不要だが、
    /// relay の capsule 対応に依存する。
    WtH2,
    /// QMux (TLS over TCP + tokio-qmux)
    Qmux,
}

impl MoqTransport {
    /// CLI と JSONC の値から経路を解釈する
    ///
    /// # Errors
    ///
    /// `quic` / `wt-h3` / `wt-h2` / `qmux` 以外の値はエラーになる。
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "quic" => Ok(Self::Quic),
            "wt-h3" => Ok(Self::WtH3),
            "wt-h2" => Ok(Self::WtH2),
            "qmux" => Ok(Self::Qmux),
            _ => Err(ErrorMessage::new(format!(
                "--transport は quic、wt-h3、wt-h2、qmux で指定してください: {value:?}"
            ))
            .into()),
        }
    }

    /// CLI とログに載せる名前を返す
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Quic => "quic",
            Self::WtH3 => "wt-h3",
            Self::WtH2 => "wt-h2",
            Self::Qmux => "qmux",
        }
    }
}

/// MOQT の ALPN プロトコル識別子
///
/// draft-ietf-moq-transport-23 §6.2 (Session establishment) が定める MOQT の
/// プロトコル識別子。zakuro は QUIC 直接接続と QMux の両方でこの値を使う。QMux は
/// individual draft Section 3.2 が native QUIC と同じ識別子を使うと定めているためである。
/// この値は draft 由来であり将来の draft 改訂で変わる可能性がある。
pub(crate) const ALPN: &[u8] = b"moqt-23";

/// MOQT URI でポートを省略したときに使う既定ポート
///
/// draft-ietf-moq-transport-23 §6.1.2 (Dereferencing a MOQT URI):
/// "If the port is omitted in the URI, a default port of 443 is used."
const DEFAULT_PORT: u16 = 443;

/// 名前解決の打ち切り時間
///
/// `tokio::net::lookup_host` は OS のリゾルバに委譲するため、リゾルバが応答しないと
/// 数十秒から数分ブロックする。接続タイムアウトとは別にここで打ち切る。
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// 接続先 1 件あたりの試行時間 (最後の候補以外)
///
/// 到達できないアドレスへの QUIC 接続はタイムアウトでしか失敗しないため、
/// 候補ごとに短く打ち切って次の候補へ進む。
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);

/// 最後の接続先候補に与える時間
///
/// 負荷試験では仮想クライアントが同時に接続するため、ハンドシェイクに時間がかかる。
/// 最後の候補で打ち切ると再接続の無駄が増えるため、ここだけ長めに取る。
const CONNECT_FINAL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(15);

/// MOQ relay の接続先
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MoqEndpoint {
    /// TLS の SNI と名前解決に使うホスト部 (ポートを含まない)
    pub(crate) host: String,
    /// 接続先ポート (URL で省略された場合は 443)
    pub(crate) port: u16,
    /// SETUP の AUTHORITY に載せる URL の authority 部
    ///
    /// draft-ietf-moq-transport-23 §9.1.1 (AUTHORITY): "When connecting to a server using a
    /// URI with the "moqt" scheme, the client MUST set the AUTHORITY option to the authority
    /// portion of the URI." に従い、URL に書かれた authority をそのまま使う。
    pub(crate) authority: String,
    /// SETUP の PATH に載せるパス (省略時は `/`)
    pub(crate) path: String,
}

impl MoqEndpoint {
    /// `moqt://` URL を解釈する
    ///
    /// 対応する形式は `moqt://<host>[:<port>][/<path>]` のみである。IPv6 リテラルは
    /// `moqt://[::1]:4433/` のようにブラケットで囲む。
    ///
    /// # Errors
    ///
    /// スキームが `moqt` でない場合、ホスト部が空の場合、ポートが数値でない場合はエラーになる。
    pub(crate) fn parse(url: &str) -> Result<Self> {
        Self::parse_with_name(url, "--url")
    }

    /// GOAWAY の New Session URI を解釈する
    ///
    /// CLI 引数ではない URL を解釈するため、エラーメッセージに `--url` を含めない。
    ///
    /// # Errors
    ///
    /// スキームが `moqt` でない場合、ホスト部が空の場合、ポートが数値でない場合はエラーになる。
    pub(crate) fn parse_new_session_uri(url: &str) -> Result<Self> {
        Self::parse_with_name(url, "GOAWAY の New Session URI")
    }

    /// URL を解釈する
    ///
    /// `name` はエラーメッセージに載せる呼び名である (CLI 引数か New Session URI か)。
    fn parse_with_name(url: &str, name: &str) -> Result<Self> {
        let rest = url.strip_prefix("moqt://").ok_or_else(|| {
            ErrorMessage::new(format!("{name} は moqt:// で始まる必要があります: {url:?}"))
        })?;

        // フラグメントは MOQT の接続に使わないため取り除く
        // (draft-ietf-moq-transport-23 §6.1.1 (Fragment Identifiers): フラグメントは
        // 送信せず、接続後にクライアントがローカルで処理する)
        let rest = rest.split('#').next().unwrap_or(rest);

        // authority は最初の `/` または `?` まで、それ以降は path
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let path = if end == rest.len() {
            "/".to_string()
        } else if rest.as_bytes()[end] == b'?' {
            // path が空でクエリのみの場合はルートを補う
            format!("/{}", &rest[end..])
        } else {
            rest[end..].to_string()
        };
        if authority.is_empty() {
            return Err(
                ErrorMessage::new(format!("{name} にホストが指定されていません: {url:?}")).into(),
            );
        }

        let (host, port) = split_host_port(authority, name)?;
        if host.is_empty() {
            return Err(
                ErrorMessage::new(format!("{name} にホストが指定されていません: {url:?}")).into(),
            );
        }

        Ok(Self {
            host,
            port: port.unwrap_or(DEFAULT_PORT),
            authority: authority.to_string(),
            path,
        })
    }
}

/// authority からホスト部とポート部を分離する
///
/// IPv6 リテラルはブラケットの内側をホスト部とする。ブラケットが無い場合、複数の `:`
/// を含む authority はポートの区切りと区別できないためエラーにする。`name` はエラーメッセージに
/// 載せる呼び名である。
fn split_host_port(authority: &str, name: &str) -> Result<(String, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            ErrorMessage::new(format!("{name} の IPv6 アドレスが不正です: {authority:?}"))
        })?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            None
        } else {
            let value = after.strip_prefix(':').ok_or_else(|| {
                ErrorMessage::new(format!("{name} のポート指定が不正です: {authority:?}"))
            })?;
            Some(parse_port(value, authority, name)?)
        };
        return Ok((host.to_string(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') {
                return Err(ErrorMessage::new(format!(
                    "{name} の IPv6 アドレスはブラケットで囲む必要があります: {authority:?}"
                ))
                .into());
            }
            Ok((host.to_string(), Some(parse_port(port, authority, name)?)))
        }
        None => Ok((authority.to_string(), None)),
    }
}

/// ポート文字列を数値に変換する
fn parse_port(value: &str, authority: &str, name: &str) -> Result<u16> {
    value.parse::<u16>().map_err(|_| {
        ErrorMessage::new(format!("{name} のポート指定が不正です: {authority:?}")).into()
    })
}

/// MOQ relay へ接続するときの TLS 設定
#[derive(Debug, Clone, Default)]
pub(crate) struct MoqTlsOptions {
    /// 証明書検証をスキップする (`--insecure`)
    pub(crate) insecure: bool,
    /// 信頼する CA 証明書 (PEM) のパス (`--ca-cert`)
    ///
    /// 未指定なら WebPKI、指定時はこの PEM だけを信頼する。
    pub(crate) ca_cert: Option<String>,
}

/// 接続先 1 件分の状態
///
/// 名前解決結果と SNI は接続先ごとに異なるため、接続先ごとに保持する。ネイティブ QUIC の
/// クライアント (UDP ソケットとエンドポイントタスク) はインスタンス内の仮想クライアントで
/// 共有する。有効なバックエンドのフィールドだけが存在する。
struct EndpointState {
    /// tobi-quic のクライアント (bool は IPv6 かどうか)
    #[cfg(feature = "quic-tobi")]
    tobi_clients: Vec<(bool, tobi_quic::Client)>,
    /// s2n-quic のクライアント (bool は IPv6 かどうか)
    #[cfg(feature = "quic-s2n")]
    s2n_clients: Vec<(bool, s2n_quic::Client)>,
    /// ngtcp2 の接続設定 (接続ごとに UDP ソケットを作る)
    #[cfg(feature = "quic-ngtcp2")]
    ngtcp2_config: Option<Arc<shiguredo_ngtcp2_tokio::ClientConfig>>,
    /// WebTransport over HTTP/3 のクライアント (bool は IPv6 かどうか)
    wt_clients: Vec<(bool, tobi_webtransport::client::Client)>,
    /// TLS の SNI に使うサーバー名
    server_name: String,
    /// 名前解決の結果
    addrs: Vec<SocketAddr>,
}

impl EndpointState {
    /// ネイティブ QUIC のクライアントを構築する (tobi-quic)
    ///
    /// # Errors
    ///
    /// クライアントの起動に失敗した場合はエラーになる。
    #[cfg(feature = "quic-tobi")]
    fn build_quic_clients(&mut self, tls: &Arc<rustls::ClientConfig>) -> Result<()> {
        for is_ipv6 in [false, true] {
            if !self.addrs.iter().any(|addr| addr.is_ipv6() == is_ipv6) {
                continue;
            }
            self.tobi_clients
                .push((is_ipv6, quic::tobi::build_client(tls, is_ipv6)?));
        }
        Ok(())
    }

    /// ネイティブ QUIC のクライアントを構築する (s2n-quic)
    ///
    /// # Errors
    ///
    /// クライアントの起動に失敗した場合はエラーになる。
    #[cfg(feature = "quic-s2n")]
    fn build_quic_clients(&mut self, tls: &quic::s2n::Tls) -> Result<()> {
        for is_ipv6 in [false, true] {
            if !self.addrs.iter().any(|addr| addr.is_ipv6() == is_ipv6) {
                continue;
            }
            self.s2n_clients
                .push((is_ipv6, quic::s2n::build_client(tls, is_ipv6)?));
        }
        Ok(())
    }

    /// ネイティブ QUIC の接続設定を保持する (ngtcp2)
    ///
    /// # Errors
    ///
    /// 設定が渡されなかった場合はエラーになる。
    #[cfg(feature = "quic-ngtcp2")]
    fn build_quic_clients(
        &mut self,
        config: &Arc<shiguredo_ngtcp2_tokio::ClientConfig>,
    ) -> Result<()> {
        self.ngtcp2_config = Some(Arc::clone(config));
        Ok(())
    }

    /// WebTransport over HTTP/3 のクライアントを構築する
    ///
    /// # Errors
    ///
    /// クライアントの起動に失敗した場合はエラーになる。
    fn build_wt_clients(&mut self, tls: &Arc<rustls::ClientConfig>) -> Result<()> {
        for is_ipv6 in [false, true] {
            if !self.addrs.iter().any(|addr| addr.is_ipv6() == is_ipv6) {
                continue;
            }
            self.wt_clients
                .push((is_ipv6, wt_h3::build_client(Arc::clone(tls), is_ipv6)?));
        }
        Ok(())
    }

    /// この接続先へ QUIC 接続を確立する (tobi-quic)
    ///
    /// 名前解決で複数のアドレスが得られた場合は順に試す。全て失敗した場合は最後のエラーを返す。
    ///
    /// # Errors
    ///
    /// 全ての接続先への接続に失敗した場合はエラーになる。
    #[cfg(feature = "quic-tobi")]
    async fn connect_quic(&self) -> Result<MoqConnection> {
        let mut last_error: Option<ErrorMessage> = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            // 接続先のアドレスファミリに合うソケットを選ぶ
            // (IPv4 ソケットから IPv6 宛には送信できないため)
            let Some((_, client)) = self
                .tobi_clients
                .iter()
                .find(|(is_ipv6, _)| *is_ipv6 == addr.is_ipv6())
            else {
                continue;
            };
            let timeout = if index == last_index {
                CONNECT_FINAL_ATTEMPT_TIMEOUT
            } else {
                CONNECT_ATTEMPT_TIMEOUT
            };
            match tokio::time::timeout(
                timeout,
                quic::tobi::connect(client, *addr, &self.server_name),
            )
            .await
            {
                Ok(Ok(connection)) => return Ok(MoqConnection::Quic(connection)),
                Ok(Err(e)) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への接続に失敗しました ({addr}): {e}"
                    )))
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への接続がタイムアウトしました ({addr})"
                    )))
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ErrorMessage::new("MOQ relay への接続先がありません"))
            .into())
    }

    /// この接続先へ QUIC 接続を確立する (s2n-quic)
    ///
    /// # Errors
    ///
    /// 全ての接続先への接続に失敗した場合はエラーになる。
    #[cfg(feature = "quic-s2n")]
    async fn connect_quic(&self) -> Result<MoqConnection> {
        let mut last_error: Option<ErrorMessage> = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            let Some((_, client)) = self
                .s2n_clients
                .iter()
                .find(|(is_ipv6, _)| *is_ipv6 == addr.is_ipv6())
            else {
                continue;
            };
            let timeout = if index == last_index {
                CONNECT_FINAL_ATTEMPT_TIMEOUT
            } else {
                CONNECT_ATTEMPT_TIMEOUT
            };
            match tokio::time::timeout(
                timeout,
                quic::s2n::connect(client, *addr, &self.server_name),
            )
            .await
            {
                Ok(Ok(connection)) => return Ok(MoqConnection::S2n(connection)),
                Ok(Err(e)) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への接続に失敗しました ({addr}): {e}"
                    )))
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への接続がタイムアウトしました ({addr})"
                    )))
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ErrorMessage::new("MOQ relay への接続先がありません"))
            .into())
    }

    /// この接続先へ QUIC 接続を確立する (ngtcp2)
    ///
    /// ngtcp2 は接続ごとに UDP ソケットを作るため、候補のアドレスファミリに合わせて
    /// ローカルアドレスを選ぶ。
    ///
    /// # Errors
    ///
    /// 全ての接続先への接続に失敗した場合はエラーになる。
    #[cfg(feature = "quic-ngtcp2")]
    async fn connect_quic(&self) -> Result<MoqConnection> {
        let config = self
            .ngtcp2_config
            .as_ref()
            .ok_or_else(|| ErrorMessage::new("ngtcp2 の接続設定が構築されていません"))?;
        let mut last_error: Option<ErrorMessage> = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            let timeout = if index == last_index {
                CONNECT_FINAL_ATTEMPT_TIMEOUT
            } else {
                CONNECT_ATTEMPT_TIMEOUT
            };
            let local = quic::local_bind_addr(addr.is_ipv6());
            match tokio::time::timeout(
                timeout,
                quic::ngtcp2::connect(*addr, local, &self.server_name, config),
            )
            .await
            {
                Ok(Ok(connection)) => return Ok(MoqConnection::Ngtcp2(connection)),
                Ok(Err(e)) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への ngtcp2 接続に失敗しました ({addr}): {e}"
                    )))
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への ngtcp2 接続がタイムアウトしました ({addr})"
                    )))
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ErrorMessage::new("MOQ relay への接続先がありません"))
            .into())
    }

    /// この接続先へ WebTransport over HTTP/3 接続を確立する
    ///
    /// QUIC 直接接続と同様に、名前解決で複数のアドレスが得られた場合は順に試し、
    /// 候補ごとにタイムアウトで打ち切る。
    ///
    /// # Errors
    ///
    /// 全ての接続先への QUIC + HTTP/3 + WebTransport 接続に失敗した場合はエラーになる。
    async fn connect_wt_h3(&self, endpoint: &MoqEndpoint) -> Result<wt_h3::Connection> {
        let mut last_error: Option<ErrorMessage> = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            let Some((_, client)) = self
                .wt_clients
                .iter()
                .find(|(is_ipv6, _)| *is_ipv6 == addr.is_ipv6())
            else {
                continue;
            };
            let timeout = if index == last_index {
                CONNECT_FINAL_ATTEMPT_TIMEOUT
            } else {
                CONNECT_ATTEMPT_TIMEOUT
            };
            match tokio::time::timeout(
                timeout,
                wt_h3::connect(
                    client,
                    *addr,
                    &self.server_name,
                    &endpoint.authority,
                    &endpoint.path,
                ),
            )
            .await
            {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(e)) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への WebTransport over HTTP/3 接続に失敗しました ({addr}): {e}"
                    )))
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への WebTransport over HTTP/3 接続がタイムアウトしました ({addr})"
                    )))
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ErrorMessage::new("MOQ relay への接続先がありません"))
            .into())
    }

    /// この接続先へ QMux 接続 (TLS over TCP) を確立する
    ///
    /// QUIC 直接接続と同様に、名前解決で複数のアドレスが得られた場合は順に試し、候補ごとに
    /// タイムアウトで打ち切る。QMux は仮想クライアントごとに独立した TCP 接続を張るため、
    /// UDP ソケットのようにインスタンス内で共有できない。
    ///
    /// # Errors
    ///
    /// 全ての接続先への TCP + TLS 接続に失敗した場合はエラーになる。
    async fn connect_qmux(
        &self,
        tls: &Arc<rustls::ClientConfig>,
    ) -> Result<Arc<tokio_qmux::Connection>> {
        let mut last_error: Option<ErrorMessage> = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            let timeout = if index == last_index {
                CONNECT_FINAL_ATTEMPT_TIMEOUT
            } else {
                CONNECT_ATTEMPT_TIMEOUT
            };
            // TCP 接続と TLS ハンドシェイクをまとめて打ち切る。QMux の接続生成はローカル処理
            // のため、ここには含めない
            let connect = qmux::connect(*addr, &self.server_name, Arc::clone(tls));
            match tokio::time::timeout(timeout, connect).await {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(e)) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への QMux 接続に失敗しました ({addr}): {e}"
                    )))
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への QMux 接続がタイムアウトしました ({addr})"
                    )))
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ErrorMessage::new("MOQ relay への接続先がありません"))
            .into())
    }

    /// この接続先へ WebTransport over HTTP/2 接続 (TLS over TCP) を確立する
    ///
    /// QUIC 直接接続や QMux と同様に、名前解決で複数のアドレスが得られた場合は順に試し、
    /// 候補ごとにタイムアウトで打ち切る。仮想クライアントごとに独立した TCP 接続を張る。
    ///
    /// # Errors
    ///
    /// 全ての接続先への TCP + TLS + HTTP/2 / WebTransport 接続に失敗した場合はエラーになる。
    async fn connect_wt_h2(
        &self,
        tls: &Arc<rustls::ClientConfig>,
        endpoint: &MoqEndpoint,
    ) -> Result<wt_h2::Connection> {
        let mut last_error: Option<ErrorMessage> = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            let timeout = if index == last_index {
                CONNECT_FINAL_ATTEMPT_TIMEOUT
            } else {
                CONNECT_ATTEMPT_TIMEOUT
            };
            // TCP 接続、TLS ハンドシェイク、HTTP/2 の SETTINGS 交換、WebTransport の
            // Extended CONNECT をまとめて打ち切る
            let connect = wt_h2::connect(
                *addr,
                &self.server_name,
                &endpoint.authority,
                &endpoint.path,
                tls,
            );
            match tokio::time::timeout(timeout, connect).await {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(e)) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への WebTransport over HTTP/2 接続に失敗しました ({addr}): {e}"
                    )))
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への WebTransport over HTTP/2 接続がタイムアウトしました ({addr})"
                    )))
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ErrorMessage::new("MOQ relay への接続先がありません"))
            .into())
    }
}

/// 接続先キャッシュのキー
///
/// 名前解決・SNI・UDP ソケットは host と port だけで決まる。authority / path は SETUP に
/// 載せる値であり `SessionConfig` 側で扱うため、path や query だけが異なる New Session URI
/// でソケットを作り直さないようキーに含めない。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EndpointKey {
    /// TLS の SNI と名前解決に使うホスト部
    host: String,
    /// 接続先ポート
    port: u16,
}

impl EndpointKey {
    /// 接続先からキーを作る
    ///
    /// ホスト名の大文字小文字は DNS では区別されないため、キーでは小文字に正規化する。
    /// IPv6 リテラルの表記揺れまでは正規化しない (同じ接続先に余分なソケットを作り得るが、
    /// 誤って別の接続先を共有することはない)。
    fn new(endpoint: &MoqEndpoint) -> Self {
        Self {
            host: endpoint.host.to_ascii_lowercase(),
            port: endpoint.port,
        }
    }
}

/// インスタンス内の仮想クライアントで共有する接続コンテキスト
///
/// ネイティブ QUIC と WebTransport over HTTP/3 では QUIC の `Client` (UDP ソケットと
/// エンドポイントタスク) を接続先ごとに共有する。仮想クライアントごとに作るとソケットと
/// タスクが仮想クライアント数だけ増えるため、インスタンスで 1 つ作って clone する。
/// QMux と WebTransport over HTTP/2 では TCP + TLS 接続を仮想クライアントごとに張るため、
/// ここで共有するのは TLS 設定と接続先ごとの名前解決結果だけである。
/// GOAWAY の New Session URI で移行先が変わると接続先が増えるため、接続先ごとの状態を
/// キャッシュして仮想クライアント間で共有する。
#[derive(Clone)]
pub(crate) struct MoqClientContext {
    /// 下位トランスポート種別 (インスタンス内で固定)
    transport: MoqTransport,
    /// tobi-quic の TLS 設定 (QUIC 直接接続と WebTransport over HTTP/3 で共有する)
    #[cfg(feature = "quic-tobi")]
    quic_tls: Option<Arc<rustls::ClientConfig>>,
    /// s2n-quic の TLS 設定 (移行先が増えても作り直さない)
    #[cfg(feature = "quic-s2n")]
    quic_tls: Option<quic::s2n::Tls>,
    /// ngtcp2 の接続設定 (接続ごとの UDP ソケットは共有しない)
    #[cfg(feature = "quic-ngtcp2")]
    ngtcp2_config: Option<Arc<shiguredo_ngtcp2_tokio::ClientConfig>>,
    /// WebTransport over HTTP/3 の TLS 設定 (ALPN は tobi-webtransport が設定する)
    wt_h3_tls: Option<Arc<rustls::ClientConfig>>,
    /// QMux の TLS 設定 (TCP + TLS のハンドシェイクに使う)
    ///
    /// rustls の `ClientConfig` は接続ごとに clone せず `Arc` で共有できる。
    qmux_tls: Option<Arc<rustls::ClientConfig>>,
    /// WebTransport over HTTP/2 の TLS 設定 (ALPN `h2`、TCP + TLS で使う)
    wt_h2_tls: Option<Arc<rustls::ClientConfig>>,
    /// 接続先ごとの状態 (GOAWAY の New Session URI で増える)
    ///
    /// `tokio::sync::Mutex` を使う理由: 複数の仮想クライアントが同じ移行先へ同時に接続する
    /// ため、キャッシュへの読み書きを直列化する。名前解決・接続処理などの await を伴う処理は
    /// ロックを持たずに行う (ロックを await で保持すると、全仮想クライアントの接続が直列化
    /// される)。
    endpoints: Arc<tokio::sync::Mutex<HashMap<EndpointKey, Arc<EndpointState>>>>,
}

impl MoqClientContext {
    /// 初期接続先の名前解決と経路ごとのクライアント構築を行う
    ///
    /// # Errors
    ///
    /// 名前解決に失敗した場合、TLS 設定が不正な場合、ソケットのバインドに失敗した場合は
    /// エラーになる。
    pub(crate) async fn new(
        endpoint: &MoqEndpoint,
        tls_options: &MoqTlsOptions,
        transport: MoqTransport,
    ) -> Result<Self> {
        let mut context = Self {
            transport,
            #[cfg(feature = "quic-tobi")]
            quic_tls: None,
            #[cfg(feature = "quic-s2n")]
            quic_tls: None,
            #[cfg(feature = "quic-ngtcp2")]
            ngtcp2_config: None,
            wt_h3_tls: None,
            qmux_tls: None,
            wt_h2_tls: None,
            endpoints: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        };
        match transport {
            MoqTransport::Quic => context.init_quic(tls_options)?,
            MoqTransport::WtH3 => context.wt_h3_tls = Some(build_quic_base_tls(tls_options)?),
            // WebTransport over HTTP/2 は TCP + TLS 上で動くため QUIC 実装を使わない
            MoqTransport::WtH2 => context.wt_h2_tls = Some(build_wt_h2_tls(tls_options)?),
            MoqTransport::Qmux => context.qmux_tls = Some(build_qmux_tls(tls_options)?),
        }
        let state = build_endpoint_state(endpoint, &context).await?;
        context
            .endpoints
            .lock()
            .await
            .insert(EndpointKey::new(endpoint), Arc::new(state));
        Ok(context)
    }

    /// QUIC 直接接続の設定を構築する (tobi-quic)
    ///
    /// # Errors
    ///
    /// TLS 設定の構築に失敗した場合はエラーになる。
    #[cfg(feature = "quic-tobi")]
    fn init_quic(&mut self, tls_options: &MoqTlsOptions) -> Result<()> {
        self.quic_tls = Some(build_quic_base_tls(tls_options)?);
        Ok(())
    }

    /// QUIC 直接接続の設定を構築する (s2n-quic)
    ///
    /// # Errors
    ///
    /// TLS 設定の構築に失敗した場合はエラーになる。
    #[cfg(feature = "quic-s2n")]
    fn init_quic(&mut self, tls_options: &MoqTlsOptions) -> Result<()> {
        self.quic_tls = Some(quic::s2n::build_tls(tls_options)?);
        Ok(())
    }

    /// QUIC 直接接続の設定を構築する (ngtcp2)
    ///
    /// # Errors
    ///
    /// TLS 設定の構築に失敗した場合、または `--ca-cert` の読み込みに失敗した場合は
    /// エラーになる。
    #[cfg(feature = "quic-ngtcp2")]
    fn init_quic(&mut self, tls_options: &MoqTlsOptions) -> Result<()> {
        self.ngtcp2_config = Some(Arc::new(quic::ngtcp2::build_config(tls_options)?));
        Ok(())
    }

    /// 指定した接続先へ選択中のトランスポートで接続を確立する
    ///
    /// 初めての接続先の場合は名前解決とクライアントの構築を行う。同じ接続先に対しては
    /// インスタンス内で状態を共有する。
    ///
    /// # Errors
    ///
    /// 名前解決に失敗した場合、TLS 設定が不正な場合、ソケットのバインドに失敗した場合、
    /// 全ての接続先への接続に失敗した場合はエラーになる。
    pub(crate) async fn connect_to(&self, endpoint: &MoqEndpoint) -> Result<MoqConnection> {
        let key = EndpointKey::new(endpoint);
        // キャッシュ済みの場合は名前解決もソケット作成もしない。
        // MutexGuard を if let の scrutinee に置くと guard が then ブロックの終わりまで
        // 生存し、接続処理の await 中もロックを保持してしまうため、ここで guard を落とす
        let cached = {
            let endpoints = self.endpoints.lock().await;
            endpoints.get(&key).cloned()
        };
        let state = match cached {
            Some(state) => state,
            None => {
                // 初めての接続先はロックを保持せずに構築する。同じ接続先へ複数の仮想
                // クライアントが同時に接続した場合は、先に登録された状態を使い、余分に
                // 作った状態は drop する (UDP ソケットも閉じる)。名前解決の失敗でロック待ちが
                // 連鎖しないことを優先する
                let built = Arc::new(build_endpoint_state(endpoint, self).await?);
                self.endpoints
                    .lock()
                    .await
                    .entry(key)
                    .or_insert_with(|| Arc::clone(&built))
                    .clone()
            }
        };
        match self.transport {
            MoqTransport::Quic => state.connect_quic().await,
            MoqTransport::WtH3 => Ok(MoqConnection::WtH3(state.connect_wt_h3(endpoint).await?)),
            MoqTransport::WtH2 => {
                let tls = self
                    .wt_h2_tls
                    .as_ref()
                    .expect("logical invariant: WebTransport over HTTP/2 always has a TLS config");
                Ok(MoqConnection::WtH2(
                    state.connect_wt_h2(tls, endpoint).await?,
                ))
            }
            MoqTransport::Qmux => {
                let tls = self
                    .qmux_tls
                    .as_ref()
                    .expect("logical invariant: QMux transport always has a TLS config");
                Ok(MoqConnection::Qmux(state.connect_qmux(tls).await?))
            }
        }
    }
}

/// 確立済みの接続
///
/// `session.rs` は経路を意識せずにこの型を分割し、ストリームの送受信と接続の終了を行う。
pub(crate) enum MoqConnection {
    /// tobi-quic による QUIC 直接接続
    #[cfg(feature = "quic-tobi")]
    Quic(tobi_quic::Connection),
    /// s2n-quic による QUIC 直接接続
    #[cfg(feature = "quic-s2n")]
    S2n(s2n_quic::connection::Connection),
    /// ngtcp2 による QUIC 直接接続
    #[cfg(feature = "quic-ngtcp2")]
    Ngtcp2(quic::ngtcp2::Connection),
    /// WebTransport over HTTP/3 接続
    WtH3(wt_h3::Connection),
    /// WebTransport over HTTP/2 接続
    WtH2(wt_h2::Connection),
    /// QMux 接続
    Qmux(Arc<tokio_qmux::Connection>),
}

impl MoqConnection {
    /// この接続のトランスポート種別を返す
    pub(crate) fn transport(&self) -> MoqTransport {
        match self {
            #[cfg(feature = "quic-tobi")]
            Self::Quic(_) => MoqTransport::Quic,
            #[cfg(feature = "quic-s2n")]
            Self::S2n(_) => MoqTransport::Quic,
            #[cfg(feature = "quic-ngtcp2")]
            Self::Ngtcp2(_) => MoqTransport::Quic,
            Self::WtH3(_) => MoqTransport::WtH3,
            Self::WtH2(_) => MoqTransport::WtH2,
            Self::Qmux(_) => MoqTransport::Qmux,
        }
    }

    /// 接続ハンドル、accept 用の口、datagram の受け口へ分割する
    pub(crate) fn split(
        self,
    ) -> (
        MoqConnectionHandle,
        MoqRecvAcceptor,
        MoqBidiAcceptor,
        MoqDatagramReceiver,
    ) {
        match self {
            #[cfg(feature = "quic-tobi")]
            Self::Quic(connection) => {
                // 単方向と双方向で acceptor を分ける。`split` は呼ぶたびに新しい
                // `StreamAcceptor` を作るため、2 回呼んで 1 つずつ使う
                let (handle, recv_acceptor) = connection.split();
                let (_, bidi_acceptor) = connection.split();
                (
                    MoqConnectionHandle::Quic(handle),
                    MoqRecvAcceptor::Quic(recv_acceptor),
                    MoqBidiAcceptor::Quic(bidi_acceptor),
                    MoqDatagramReceiver::Quic(connection),
                )
            }
            #[cfg(feature = "quic-s2n")]
            Self::S2n(connection) => {
                let (handle, acceptor) = connection.split();
                let (bidi, recv) = acceptor.split();
                (
                    MoqConnectionHandle::S2n(handle.clone()),
                    MoqRecvAcceptor::S2n(recv),
                    MoqBidiAcceptor::S2n(bidi),
                    MoqDatagramReceiver::S2n(handle),
                )
            }
            #[cfg(feature = "quic-ngtcp2")]
            Self::Ngtcp2(connection) => {
                let (handle, recv, bidi, datagram) = connection.split();
                (
                    MoqConnectionHandle::Ngtcp2(handle),
                    MoqRecvAcceptor::Ngtcp2(recv),
                    MoqBidiAcceptor::Ngtcp2(bidi),
                    MoqDatagramReceiver::Ngtcp2(datagram),
                )
            }
            Self::WtH3(connection) => {
                let (handle, recv, bidi, datagram) = connection.split();
                (
                    MoqConnectionHandle::WtH3(handle),
                    MoqRecvAcceptor::WtH3(recv),
                    MoqBidiAcceptor::WtH3(bidi),
                    MoqDatagramReceiver::WtH3(datagram),
                )
            }
            Self::WtH2(connection) => {
                let (handle, recv, bidi, datagram) = connection.split();
                (
                    MoqConnectionHandle::WtH2(handle),
                    MoqRecvAcceptor::WtH2(recv),
                    MoqBidiAcceptor::WtH2(bidi),
                    MoqDatagramReceiver::WtH2(datagram),
                )
            }
            Self::Qmux(connection) => (
                MoqConnectionHandle::Qmux(Arc::clone(&connection)),
                MoqRecvAcceptor::Qmux(Arc::clone(&connection)),
                MoqBidiAcceptor::Qmux(Arc::clone(&connection)),
                MoqDatagramReceiver::Qmux(connection),
            ),
        }
    }
}

/// 接続先の名前解決と経路ごとのクライアントの構築を行う
///
/// WebTransport over HTTP/2 と QMux は UDP を使わないためクライアントを作らない。
///
/// # Errors
///
/// 名前解決に失敗した場合、またはソケットのバインドに失敗した場合はエラーになる。
async fn build_endpoint_state(
    endpoint: &MoqEndpoint,
    context: &MoqClientContext,
) -> Result<EndpointState> {
    let addrs = resolve(&endpoint.host, endpoint.port).await?;
    let mut state = EndpointState {
        #[cfg(feature = "quic-tobi")]
        tobi_clients: Vec::new(),
        #[cfg(feature = "quic-s2n")]
        s2n_clients: Vec::new(),
        #[cfg(feature = "quic-ngtcp2")]
        ngtcp2_config: None,
        wt_clients: Vec::new(),
        server_name: endpoint.host.clone(),
        addrs,
    };
    match context.transport {
        MoqTransport::Quic => {
            #[cfg(feature = "quic-tobi")]
            state.build_quic_clients(
                context
                    .quic_tls
                    .as_ref()
                    .expect("logical invariant: QUIC transport always has a TLS config"),
            )?;
            #[cfg(feature = "quic-s2n")]
            state.build_quic_clients(
                context
                    .quic_tls
                    .as_ref()
                    .expect("logical invariant: QUIC transport always has a TLS config"),
            )?;
            #[cfg(feature = "quic-ngtcp2")]
            state.build_quic_clients(
                context
                    .ngtcp2_config
                    .as_ref()
                    .expect("logical invariant: QUIC transport always has a ngtcp2 config"),
            )?;
        }
        MoqTransport::WtH3 => state.build_wt_clients(
            context
                .wt_h3_tls
                .as_ref()
                .expect("logical invariant: wt-h3 transport always has a TLS config"),
        )?,
        MoqTransport::WtH2 | MoqTransport::Qmux => {}
    }
    Ok(state)
}

/// ホスト名とポートを解決する
///
/// # Errors
///
/// 解決が [`RESOLVE_TIMEOUT`] を超えた場合、またはアドレスが 1 件も得られなかった場合は
/// エラーになる。
async fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let host_port = if host.contains(':') {
        // IPv6 リテラルはブラケットで囲まないとポートと区別できない
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let addrs = tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host(&host_port))
        .await
        .map_err(|_| {
            ErrorMessage::new(format!(
                "MOQ relay の名前解決がタイムアウトしました: {host}"
            ))
        })?
        .map_err(|e| ErrorMessage::new(format!("MOQ relay の名前解決に失敗しました: {e}")))?;
    let addrs: Vec<SocketAddr> = addrs.collect();
    if addrs.is_empty() {
        return Err(ErrorMessage::new(format!(
            "MOQ relay のアドレスが解決できませんでした: {host}"
        ))
        .into());
    }
    Ok(addrs)
}

/// rustls のクライアント設定を構築する (ALPN は呼び出し側が設定する)
///
/// # Errors
///
/// `--ca-cert` に指定した PEM の読み込みに失敗した場合、ルート証明書が 1 件も
/// 得られない場合、または TLS 設定の構築に失敗した場合はエラーになる。
pub(crate) fn build_rustls_config(options: &MoqTlsOptions) -> Result<rustls::ClientConfig> {
    let config = if options.insecure {
        // `--insecure` 指定時の開発用経路である。検証をしない旨は呼び出し側で警告ログに出す
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| ErrorMessage::new(format!("TLS バージョンの設定に失敗しました: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier))
        .with_no_client_auth()
    } else {
        let roots = build_root_store(options)?;
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| ErrorMessage::new(format!("TLS バージョンの設定に失敗しました: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth()
    };
    Ok(config)
}

/// 信頼するルート証明書を構築する
///
/// `--ca-cert` が指定された場合はその PEM だけを信頼し、未指定の場合は WebPKI の
/// ルート証明書を使う。
///
/// # Errors
///
/// `--ca-cert` の読み込み・PEM のパース・ルート証明書の追加に失敗した場合、または
/// 証明書が 1 件も含まれていない場合はエラーになる。
fn build_root_store(options: &MoqTlsOptions) -> Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    let Some(path) = options.ca_cert.as_deref() else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        return Ok(roots);
    };

    let pem = std::fs::read_to_string(path).map_err(|e| {
        ErrorMessage::new(format!(
            "--ca-cert の CA 証明書の読み込みに失敗しました ({path}): {e}"
        ))
    })?;
    let mut count = 0;
    for certificate in rustls::pki_types::CertificateDer::pem_slice_iter(pem.as_bytes()) {
        let certificate = certificate.map_err(|e| {
            ErrorMessage::new(format!(
                "--ca-cert の CA 証明書のパースに失敗しました ({path}): {e}"
            ))
        })?;
        roots.add(certificate).map_err(|e| {
            ErrorMessage::new(format!(
                "--ca-cert の CA 証明書の追加に失敗しました ({path}): {e}"
            ))
        })?;
        count += 1;
    }
    if count == 0 {
        return Err(ErrorMessage::new(format!(
            "--ca-cert の CA 証明書に PEM 形式の証明書が含まれていません ({path})"
        ))
        .into());
    }
    Ok(roots)
}

/// ALPN を設定しない QUIC 用の TLS 設定を構築する
///
/// ALPN は経路ごとに異なる (`moqt-23` と `h3`) ため、ここでは設定しない。tobi-quic は
/// クライアント構築時に、tobi-webtransport は接続時に ALPN を設定する。
///
/// # Errors
///
/// TLS 設定の構築に失敗した場合はエラーになる。
fn build_quic_base_tls(options: &MoqTlsOptions) -> Result<Arc<rustls::ClientConfig>> {
    Ok(Arc::new(build_rustls_config(options)?))
}

/// QMux 用の TLS 設定を構築する
///
/// # Errors
///
/// TLS 設定の構築に失敗した場合はエラーになる。
fn build_qmux_tls(options: &MoqTlsOptions) -> Result<Arc<rustls::ClientConfig>> {
    let mut config = build_rustls_config(options)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Arc::new(config))
}

/// WebTransport over HTTP/2 用の TLS 設定を構築する
///
/// HTTP/2 の ALPN 識別子は `h2` である (RFC 9113 §3.3)。WebTransport over HTTP/2 は
/// relay の `WT_RESET_STREAM` capsule 対応に依存するため、RESET_STREAM_AT を必要としない。
///
/// # Errors
///
/// TLS 設定の構築に失敗した場合はエラーになる。
fn build_wt_h2_tls(options: &MoqTlsOptions) -> Result<Arc<rustls::ClientConfig>> {
    let mut config = build_rustls_config(options)?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(config))
}

/// 証明書検証をスキップする Verifier (`--insecure` 専用)
#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ホストとポートを指定した URL を解釈できること
    #[test]
    fn parse_url_with_host_and_port() {
        let endpoint =
            MoqEndpoint::parse("moqt://relay.example.com:4433").expect("解釈に成功すること");
        assert_eq!(endpoint.host, "relay.example.com");
        assert_eq!(endpoint.port, 4433);
        assert_eq!(endpoint.authority, "relay.example.com:4433");
        assert_eq!(endpoint.path, "/");
    }

    /// ポートを省略した場合は 443 を使い、authority は URL の表記のままにすること
    #[test]
    fn parse_url_defaults_port_to_443() {
        let endpoint = MoqEndpoint::parse("moqt://relay.example.com/").expect("解釈に成功すること");
        assert_eq!(endpoint.host, "relay.example.com");
        assert_eq!(endpoint.port, 443);
        assert_eq!(endpoint.authority, "relay.example.com");
        assert_eq!(endpoint.path, "/");
    }

    /// パス付きの URL からパスを取り出せること
    #[test]
    fn parse_url_keeps_path() {
        let endpoint = MoqEndpoint::parse("moqt://relay.example.com:4433/live/room")
            .expect("解釈に成功すること");
        assert_eq!(endpoint.host, "relay.example.com");
        assert_eq!(endpoint.port, 4433);
        assert_eq!(endpoint.path, "/live/room");
    }

    /// IPv6 リテラルはブラケットを外してホスト部にすること
    #[test]
    fn parse_url_with_ipv6_literal() {
        let endpoint =
            MoqEndpoint::parse("moqt://[2001:db8::1]:4443/").expect("解釈に成功すること");
        assert_eq!(endpoint.host, "2001:db8::1");
        assert_eq!(endpoint.port, 4443);
        assert_eq!(endpoint.authority, "[2001:db8::1]:4443");
    }

    /// ブラケット無しの IPv6 リテラルは拒否すること (ポートと区別できないため)
    #[test]
    fn parse_url_rejects_bare_ipv6_literal() {
        assert!(MoqEndpoint::parse("moqt://2001:db8::1/").is_err());
    }

    /// moqt 以外のスキームは拒否すること
    #[test]
    fn parse_url_rejects_other_scheme() {
        assert!(MoqEndpoint::parse("https://relay.example.com/").is_err());
    }

    /// ポートが数値でない場合は拒否すること
    #[test]
    fn parse_url_rejects_invalid_port() {
        assert!(MoqEndpoint::parse("moqt://relay.example.com:abc/").is_err());
    }

    /// ホストが空の場合は拒否すること
    #[test]
    fn parse_url_rejects_empty_host() {
        assert!(MoqEndpoint::parse("moqt://").is_err());
        assert!(MoqEndpoint::parse("moqt://:4433/").is_err());
    }

    /// `--transport` の値を解釈できること
    #[test]
    fn parse_transport_accepts_supported_values() {
        assert_eq!(
            MoqTransport::parse("quic").expect("quic は受理されること"),
            MoqTransport::Quic
        );
        assert_eq!(
            MoqTransport::parse("qmux").expect("qmux は受理されること"),
            MoqTransport::Qmux
        );
        assert_eq!(
            MoqTransport::parse("wt-h3").expect("wt-h3 は受理されること"),
            MoqTransport::WtH3
        );
        assert_eq!(
            MoqTransport::parse("wt-h2").expect("wt-h2 は受理されること"),
            MoqTransport::WtH2
        );
        // ログと CLI で同じ表記を使う
        assert_eq!(MoqTransport::Quic.as_str(), "quic");
        assert_eq!(MoqTransport::Qmux.as_str(), "qmux");
        assert_eq!(MoqTransport::WtH3.as_str(), "wt-h3");
        assert_eq!(MoqTransport::WtH2.as_str(), "wt-h2");
    }

    /// 未対応の transport 名・表記ゆれ・空文字は拒否すること
    #[test]
    fn parse_transport_rejects_unknown_values() {
        for value in ["QUIC", "QMux", "wt-h2 ", "quic-direct", ""] {
            assert!(
                MoqTransport::parse(value).is_err(),
                "受理してはならない: {value:?}"
            );
        }
    }

    /// クエリのみの URL はパスを `/` で補うこと
    #[test]
    fn parse_url_with_query_only() {
        let endpoint =
            MoqEndpoint::parse("moqt://relay.example.com?x=1").expect("解釈に成功すること");
        assert_eq!(endpoint.host, "relay.example.com");
        assert_eq!(endpoint.authority, "relay.example.com");
        assert_eq!(endpoint.path, "/?x=1");
    }

    /// フラグメントは接続に使わないため取り除くこと
    #[test]
    fn parse_url_strips_fragment() {
        let endpoint =
            MoqEndpoint::parse("moqt://relay.example.com/path#frag").expect("解釈に成功すること");
        assert_eq!(endpoint.host, "relay.example.com");
        assert_eq!(endpoint.path, "/path");

        // authority 直後のフラグメントもホスト部に混ざらないこと
        let endpoint =
            MoqEndpoint::parse("moqt://relay.example.com#frag").expect("解釈に成功すること");
        assert_eq!(endpoint.host, "relay.example.com");
        assert_eq!(endpoint.path, "/");
    }

    /// 空のポート指定は拒否すること
    #[test]
    fn parse_url_rejects_empty_port() {
        assert!(MoqEndpoint::parse("moqt://relay.example.com:/").is_err());
    }

    /// ポート 0 は解釈でき、そのまま保持されること (接続時に OS が拒否する)
    #[test]
    fn parse_url_keeps_port_zero() {
        let endpoint =
            MoqEndpoint::parse("moqt://relay.example.com:0/").expect("解釈に成功すること");
        assert_eq!(endpoint.port, 0);
    }

    /// 同じ host / port で path や query だけが違う接続先は同じキャッシュキーになること
    ///
    /// New Session URI が path / query だけ変わる場合に UDP ソケットを作り直さない。
    #[test]
    fn endpoint_key_ignores_path_and_query() {
        let a =
            MoqEndpoint::parse("moqt://relay.example.com:4433/live").expect("解釈に成功すること");
        let b = MoqEndpoint::parse("moqt://relay.example.com:4433/other?token=abc")
            .expect("解釈に成功すること");
        assert_eq!(EndpointKey::new(&a), EndpointKey::new(&b));
    }

    /// host または port が違う接続先は別のキャッシュキーになること
    #[test]
    fn endpoint_key_separates_host_and_port() {
        let base =
            MoqEndpoint::parse("moqt://relay.example.com:4433/live").expect("解釈に成功すること");
        let other_host =
            MoqEndpoint::parse("moqt://relay2.example.com:4433/live").expect("解釈に成功すること");
        let other_port =
            MoqEndpoint::parse("moqt://relay.example.com:4443/live").expect("解釈に成功すること");
        assert_ne!(EndpointKey::new(&base), EndpointKey::new(&other_host));
        assert_ne!(EndpointKey::new(&base), EndpointKey::new(&other_port));
    }

    /// host の大文字小文字はキーで正規化されること
    #[test]
    fn endpoint_key_normalizes_host_case() {
        let lower =
            MoqEndpoint::parse("moqt://relay.example.com:4433/").expect("解釈に成功すること");
        let upper =
            MoqEndpoint::parse("moqt://RELAY.EXAMPLE.COM:4433/").expect("解釈に成功すること");
        assert_eq!(EndpointKey::new(&lower), EndpointKey::new(&upper));
    }

    /// New Session URI のエラーが CLI 引数の `--url` に言及しないこと
    #[test]
    fn parse_new_session_uri_reports_its_own_name() {
        let error = MoqEndpoint::parse_new_session_uri("https://relay.example.com")
            .expect_err("エラーになること");
        let message = format!("{error}");
        assert!(
            message.contains("New Session URI"),
            "New Session URI のエラーだと分かること: {message}"
        );
        assert!(
            !message.contains("--url"),
            "CLI 引数の --url に言及しないこと: {message}"
        );
    }

    /// QMux の TLS 設定が MOQT の ALPN を広告すること
    ///
    /// QMux 上の MOQT は native QUIC と同じ `moqt-23` を使う。ALPN が無いと
    /// rustls のハンドシェイク自体は成功してしまい、QMux のトランスポートパラメータを
    /// 待ち続けるため、確実に載っていることを固定する。
    #[test]
    fn qmux_tls_advertises_moqt_alpn() {
        let tls = build_qmux_tls(&MoqTlsOptions::default()).expect("TLS 設定の構築に成功すること");
        assert_eq!(tls.alpn_protocols, vec![b"moqt-23".to_vec()]);
    }

    /// `--insecure` でも QMux の ALPN を広告すること
    #[test]
    fn qmux_tls_insecure_advertises_moqt_alpn() {
        let options = MoqTlsOptions {
            insecure: true,
            ca_cert: None,
        };
        let tls = build_qmux_tls(&options).expect("TLS 設定の構築に成功すること");
        assert_eq!(tls.alpn_protocols, vec![b"moqt-23".to_vec()]);
    }

    /// tobi-quic 用の TLS 設定には ALPN を載せないこと
    ///
    /// QUIC 直接接続は `moqt-23`、WebTransport over HTTP/3 は `h3` と経路で ALPN が
    /// 異なるため、共有する TLS 設定には載せず、クライアント構築時に設定する。
    #[cfg(feature = "quic-tobi")]
    #[test]
    fn quic_tls_does_not_set_alpn() {
        let tls =
            build_quic_base_tls(&MoqTlsOptions::default()).expect("TLS 設定の構築に成功すること");
        assert!(tls.alpn_protocols.is_empty());
    }

    /// 存在しない `--ca-cert` はエラーになること
    #[test]
    fn ca_cert_missing_file_is_error() {
        let options = MoqTlsOptions {
            insecure: false,
            ca_cert: Some("/nonexistent/zakuro-ca.pem".to_string()),
        };
        assert!(build_rustls_config(&options).is_err());
    }

    /// QMux の接続コンテキストが QUIC の UDP ソケットを作らないこと
    ///
    /// QMux は仮想クライアントごとに TCP を張るため、QUIC の `EndpointState` に
    /// tobi-quic のクライアントを持たせてはならない。
    #[tokio::test]
    async fn qmux_context_does_not_build_quic_clients() {
        let endpoint =
            MoqEndpoint::parse("moqt://127.0.0.1:4433/").expect("接続先の解釈に成功すること");
        let context =
            MoqClientContext::new(&endpoint, &MoqTlsOptions::default(), MoqTransport::Qmux)
                .await
                .expect("コンテキストの構築に成功すること");
        #[cfg(feature = "quic-tobi")]
        assert!(context.quic_tls.is_none());
        #[cfg(feature = "quic-s2n")]
        assert!(context.quic_tls.is_none());
        #[cfg(feature = "quic-ngtcp2")]
        assert!(context.ngtcp2_config.is_none());
        assert!(context.qmux_tls.is_some());
        let endpoints = context.endpoints.lock().await;
        let state = endpoints
            .values()
            .next()
            .expect("接続先が登録されていること");
        #[cfg(feature = "quic-tobi")]
        assert!(
            state.tobi_clients.is_empty(),
            "QMux では QUIC クライアントを作らないこと"
        );
        #[cfg(feature = "quic-s2n")]
        assert!(
            state.s2n_clients.is_empty(),
            "QMux では QUIC クライアントを作らないこと"
        );
        #[cfg(feature = "quic-ngtcp2")]
        assert!(
            state.ngtcp2_config.is_none(),
            "QMux では QUIC の接続設定を作らないこと"
        );
        assert!(state.wt_clients.is_empty());
    }

    /// WebTransport over HTTP/2 の TLS 設定が `h2` を広告すること
    ///
    /// HTTP/2 の ALPN は `h2` である (RFC 9113 §3.3)。MOQT の ALPN (`moqt-23`) は
    /// WebTransport のサブプロトコル (WT-Available-Protocols) で通知する。
    #[test]
    fn wt_h2_tls_advertises_h2_alpn() {
        let tls = build_wt_h2_tls(&MoqTlsOptions::default()).expect("TLS 設定の構築に成功すること");
        assert_eq!(tls.alpn_protocols, vec![b"h2".to_vec()]);
    }

    /// WebTransport over HTTP/2 の接続コンテキストが QUIC の設定を作らないこと
    ///
    /// WebTransport over HTTP/2 は TCP + TLS 上で動くため、UDP ソケットも QUIC の設定も
    /// 作らない。TLS 設定だけを仮想クライアント間で共有する。
    #[tokio::test]
    async fn wt_h2_context_does_not_build_quic_clients() {
        let endpoint =
            MoqEndpoint::parse("moqt://127.0.0.1:4433/").expect("接続先の解釈に成功すること");
        let context =
            MoqClientContext::new(&endpoint, &MoqTlsOptions::default(), MoqTransport::WtH2)
                .await
                .expect("コンテキストの構築に成功すること");
        #[cfg(feature = "quic-tobi")]
        assert!(context.quic_tls.is_none());
        #[cfg(feature = "quic-s2n")]
        assert!(context.quic_tls.is_none());
        #[cfg(feature = "quic-ngtcp2")]
        assert!(context.ngtcp2_config.is_none());
        assert!(context.qmux_tls.is_none());
        assert!(context.wt_h2_tls.is_some());
        let endpoints = context.endpoints.lock().await;
        let state = endpoints
            .values()
            .next()
            .expect("接続先が登録されていること");
        #[cfg(feature = "quic-tobi")]
        assert!(
            state.tobi_clients.is_empty(),
            "WebTransport over HTTP/2 では QUIC クライアントを作らないこと"
        );
        #[cfg(feature = "quic-s2n")]
        assert!(
            state.s2n_clients.is_empty(),
            "WebTransport over HTTP/2 では QUIC クライアントを作らないこと"
        );
        #[cfg(feature = "quic-ngtcp2")]
        assert!(
            state.ngtcp2_config.is_none(),
            "WebTransport over HTTP/2 では QUIC の接続設定を作らないこと"
        );
        assert!(state.wt_clients.is_empty());
    }

    /// QUIC の接続コンテキストがアドレスファミリごとのクライアントを作ること
    ///
    /// 名前解決の結果に IPv6 が含まれなければ IPv4 用の 1 つだけを作る。UDP ソケットの
    /// バインドまで含めて確認するため、loopback の実アドレスを使う。
    #[tokio::test]
    async fn quic_context_builds_client_per_address_family() {
        let endpoint =
            MoqEndpoint::parse("moqt://127.0.0.1:4433/").expect("接続先の解釈に成功すること");
        let context =
            MoqClientContext::new(&endpoint, &MoqTlsOptions::default(), MoqTransport::Quic)
                .await
                .expect("コンテキストの構築に成功すること");
        let endpoints = context.endpoints.lock().await;
        let state = endpoints
            .values()
            .next()
            .expect("接続先が登録されていること");
        #[cfg(feature = "quic-tobi")]
        {
            assert_eq!(
                state.tobi_clients.len(),
                1,
                "IPv4 のクライアントだけを作ること"
            );
            assert!(!state.tobi_clients[0].0, "IPv4 用のクライアントであること");
        }
        #[cfg(feature = "quic-s2n")]
        {
            assert_eq!(
                state.s2n_clients.len(),
                1,
                "IPv4 のクライアントだけを作ること"
            );
            assert!(!state.s2n_clients[0].0, "IPv4 用のクライアントであること");
        }
        #[cfg(feature = "quic-ngtcp2")]
        {
            assert!(
                state.ngtcp2_config.is_some(),
                "接続設定を作ること (接続ごとにソケットを作る)"
            );
        }
        assert!(state.wt_clients.is_empty());
    }

    /// WebTransport over HTTP/3 の接続コンテキストがクライアントを作ること
    #[tokio::test]
    async fn wt_h3_context_builds_client_per_address_family() {
        let endpoint =
            MoqEndpoint::parse("moqt://127.0.0.1:4433/").expect("接続先の解釈に成功すること");
        let context =
            MoqClientContext::new(&endpoint, &MoqTlsOptions::default(), MoqTransport::WtH3)
                .await
                .expect("コンテキストの構築に成功すること");
        let endpoints = context.endpoints.lock().await;
        let state = endpoints
            .values()
            .next()
            .expect("接続先が登録されていること");
        assert_eq!(
            state.wt_clients.len(),
            1,
            "IPv4 のクライアントだけを作ること"
        );
        assert!(!state.wt_clients[0].0, "IPv4 用のクライアントであること");
    }
}
