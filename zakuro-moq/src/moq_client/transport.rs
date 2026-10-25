//! MOQ relay への接続 (QUIC 直接接続 / WebTransport over HTTP/3 / WebTransport over HTTP/2 / QMux)
//!
//! MOQT の接続先 URL の解釈、名前解決、TLS 設定、経路ごとのクライアント構築を担当する。
//! QUIC 直接接続は s2n-quic / ngtcp2 を、WebTransport over HTTP/2 と QMux は TLS over TCP を
//! 使う。各経路を `session.rs` からは [`MoqConnection`] として同じように扱える。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use s2n_quic::Client;
use s2n_quic::client::Connect;
use s2n_quic::connection::Connection;
use s2n_quic::provider::tls::rustls as s2n_rustls;

use crate::error::{ErrorMessage, Result};

use super::stream::{MoqBidiAcceptor, MoqConnectionHandle, MoqRecvAcceptor};
use super::{ngtcp2, qmux, wt_h2};

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
    /// ngtcp2 による WebTransport over HTTP/3
    WtH3,
    /// WebTransport over HTTP/2 (TCP + TLS + shiguredo_http2)
    ///
    /// reset を `WT_RESET_STREAM` capsule で行うため `RESET_STREAM_AT` は不要だが、
    /// relay の capsule 対応に依存する。
    WtH2,
    /// QMux (TLS over TCP + tokio-qmux)
    Qmux,
}

/// QUIC 直接接続で利用する実装
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum QuicBackend {
    /// s2n-quic
    #[default]
    S2nQuic,
    /// ngtcp2-rs の Tokio 統合
    Ngtcp2,
}

impl QuicBackend {
    /// CLI と JSONC の値から QUIC 実装を解釈する
    ///
    /// 未知の実装名はエラーになる。
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "s2n-quic" => Ok(Self::S2nQuic),
            "ngtcp2" => Ok(Self::Ngtcp2),
            _ => Err(ErrorMessage::new(format!(
                "--quic-backend は s2n-quic または ngtcp2 で指定してください: {value:?}"
            ))
            .into()),
        }
    }

    /// CLI とログに載せる実装名を返す
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::S2nQuic => "s2n-quic",
            Self::Ngtcp2 => "ngtcp2",
        }
    }
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
/// draft-ietf-moq-transport-22 §6.2 (Session establishment) が定める MOQT の
/// プロトコル識別子。zakuro は QUIC 直接接続と QMux の両方でこの値を使う。QMux は
/// individual draft Section 3.2 が native QUIC と同じ識別子を使うと定めているためである。
/// この値は draft 由来であり将来の draft 改訂で変わる可能性がある。
pub(crate) const ALPN: &[u8] = b"moqt-22";

/// MOQT URI でポートを省略したときに使う既定ポート
///
/// draft-ietf-moq-transport-22 §6.1.2 (Dereferencing a MOQT URI):
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
    /// draft-ietf-moq-transport-22 §9.1.1 (AUTHORITY): "When connecting to a server using a
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
        // (draft-ietf-moq-transport-22 §6.1.1 (Fragment Identifiers): フラグメントは
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
/// IPv6 リテラルはブラケットの内側をホスト部とする。ブラケットが無い場合、複数の `:` を
/// 含む authority はポートの区切りと区別できないためエラーにする。`name` はエラーメッセージに
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
    /// s2n-quic / QMux は未指定なら WebPKI、指定時はこの PEM だけを信頼する。
    /// ngtcp2 はシステムの信頼ストアに、この PEM を追加する。
    pub(crate) ca_cert: Option<String>,
}

/// 接続先 1 件分の状態
///
/// s2n-quic の `Client` は UDP ソケットとエンドポイントタスクを表す。UDP ソケットは異なる
/// アドレスファミリ宛に送信できないため、名前解決の結果に含まれるファミリごとに
/// クライアントを作る。名前解決結果と SNI は接続先ごとに異なるため、接続先ごとに保持する。
struct EndpointState {
    /// アドレスファミリごとの s2n-quic クライアント (bool は IPv6 かどうか)
    clients: Vec<(bool, Client)>,
    /// TLS の SNI に使うサーバー名
    server_name: String,
    /// 名前解決の結果
    addrs: Vec<SocketAddr>,
}

impl EndpointState {
    /// 各候補のアドレスファミリに合う UDP ソケットで ngtcp2 接続を試す
    ///
    /// すべての候補が失敗した場合は最後の接続エラーを返す。
    async fn connect_ngtcp2(
        &self,
        config: &shiguredo_ngtcp2_tokio::ClientConfig,
        webtransport: Option<&MoqEndpoint>,
    ) -> Result<ngtcp2::Connection> {
        let mut last_error = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            let timeout = if index == last_index {
                CONNECT_FINAL_ATTEMPT_TIMEOUT
            } else {
                CONNECT_ATTEMPT_TIMEOUT
            };
            let local = if addr.is_ipv6() {
                "[::]:0"
            } else {
                "0.0.0.0:0"
            }
            .parse()
            .expect("logical invariant: local bind address is valid");
            let connect = async {
                if let Some(endpoint) = webtransport {
                    ngtcp2::webtransport::connect(
                        *addr,
                        local,
                        &self.server_name,
                        config,
                        &endpoint.authority,
                        &endpoint.path,
                    )
                    .await
                } else {
                    ngtcp2::connect(*addr, local, &self.server_name, config).await
                }
            };
            match tokio::time::timeout(timeout, connect).await {
                Ok(Ok(connection)) => return Ok(connection),
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

    /// この接続先へ QUIC 接続を確立する
    ///
    /// 名前解決で複数のアドレスが得られた場合は順に試す。全て失敗した場合は最後のエラーを返す。
    ///
    /// # Errors
    ///
    /// 全ての接続先への接続に失敗した場合はエラーになる。
    async fn connect_quic(&self) -> Result<Connection> {
        let mut last_error: Option<ErrorMessage> = None;
        let last_index = self.addrs.len().saturating_sub(1);
        for (index, addr) in self.addrs.iter().enumerate() {
            // 接続先のアドレスファミリに合うソケットを選ぶ
            // (IPv4 ソケットから IPv6 宛には送信できないため)
            let Some((_, client)) = self
                .clients
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
            let connect = Connect::new(*addr).with_server_name(self.server_name.clone());
            match tokio::time::timeout(timeout, client.connect(connect)).await {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(e)) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への接続に失敗しました ({addr}): {e}"
                    )));
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への接続がタイムアウトしました ({addr})"
                    )));
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
                    )));
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への QMux 接続がタイムアウトしました ({addr})"
                    )));
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
                    )));
                }
                Err(_) => {
                    last_error = Some(ErrorMessage::new(format!(
                        "MOQ relay への WebTransport over HTTP/2 接続がタイムアウトしました ({addr})"
                    )));
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
/// QUIC 直接接続では s2n-quic の `Client` (UDP ソケットとエンドポイントタスク) を接続先ごとに
/// 共有する。仮想クライアントごとに作るとソケットとタスクが仮想クライアント数だけ増えるため、
/// インスタンスで 1 つ作って clone する。QMux と WebTransport over HTTP/2 では TCP + TLS 接続を
/// 仮想クライアントごとに張るため、ここで共有するのは TLS 設定と接続先ごとの名前解決結果だけである。
/// ngtcp2 も接続ごとに UDP ソケットと駆動タスクを作り、接続設定と名前解決結果を共有する。
/// GOAWAY の New Session URI で移行先が変わると接続先が増えるため、接続先ごとの状態を
/// キャッシュして仮想クライアント間で共有する。
#[derive(Clone)]
pub(crate) struct MoqClientContext {
    /// 下位トランスポート種別 (インスタンス内で固定)
    transport: MoqTransport,
    /// QUIC 直接接続の TLS クライアント (移行先が増えても作り直さない)
    quic_tls: Option<s2n_rustls::Client>,
    /// ngtcp2 の接続設定 (接続ごとの UDP ソケットは共有しない)
    ngtcp2_config: Option<shiguredo_ngtcp2_tokio::ClientConfig>,
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
        quic_backend: QuicBackend,
    ) -> Result<Self> {
        let (quic_tls, ngtcp2_config, qmux_tls, wt_h2_tls) = match (transport, quic_backend) {
            (MoqTransport::Quic, QuicBackend::S2nQuic) => {
                (Some(build_quic_tls(tls_options)?), None, None, None)
            }
            (MoqTransport::Quic, QuicBackend::Ngtcp2) => {
                (None, Some(ngtcp2::build_config(tls_options)?), None, None)
            }
            (MoqTransport::WtH3, QuicBackend::Ngtcp2) => (
                None,
                Some(ngtcp2::webtransport::build_config(tls_options)?),
                None,
                None,
            ),
            (MoqTransport::WtH3, QuicBackend::S2nQuic) => {
                return Err(
                    ErrorMessage::new("wt-h3 は ngtcp2 バックエンドで利用してください").into(),
                );
            }
            // WebTransport over HTTP/2 は TCP + TLS 上で動くため QUIC 実装を使わない。
            // `--quic-backend` の指定は args.rs で拒否する。
            (MoqTransport::WtH2, _) => (None, None, None, Some(build_wt_h2_tls(tls_options)?)),
            (MoqTransport::Qmux, _) => (None, None, Some(build_qmux_tls(tls_options)?), None),
        };
        let state = build_endpoint_state(endpoint, quic_tls.as_ref()).await?;
        let mut endpoints = HashMap::new();
        endpoints.insert(EndpointKey::new(endpoint), Arc::new(state));
        Ok(Self {
            transport,
            quic_tls,
            ngtcp2_config,
            qmux_tls,
            wt_h2_tls,
            endpoints: Arc::new(tokio::sync::Mutex::new(endpoints)),
        })
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
                let built = Arc::new(build_endpoint_state(endpoint, self.quic_tls.as_ref()).await?);
                self.endpoints
                    .lock()
                    .await
                    .entry(key)
                    .or_insert_with(|| Arc::clone(&built))
                    .clone()
            }
        };
        match self.transport {
            MoqTransport::Quic => {
                if let Some(config) = &self.ngtcp2_config {
                    Ok(MoqConnection::Ngtcp2(
                        state.connect_ngtcp2(config, None).await?,
                    ))
                } else {
                    Ok(MoqConnection::Quic(state.connect_quic().await?))
                }
            }
            MoqTransport::WtH3 => {
                let config = self
                    .ngtcp2_config
                    .as_ref()
                    .expect("WT 経路には ngtcp2 設定があること");
                Ok(MoqConnection::WebTransport(
                    state.connect_ngtcp2(config, Some(endpoint)).await?,
                ))
            }
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
    /// QUIC 直接接続
    Quic(Connection),
    /// ngtcp2 による QUIC 直接接続
    Ngtcp2(ngtcp2::Connection),
    /// ngtcp2 による WebTransport over HTTP/3 接続
    WebTransport(ngtcp2::Connection),
    /// WebTransport over HTTP/2 接続
    WtH2(wt_h2::Connection),
    /// QMux 接続
    Qmux(Arc<tokio_qmux::Connection>),
}

impl MoqConnection {
    /// この接続のトランスポート種別を返す
    pub(crate) fn transport(&self) -> MoqTransport {
        match self {
            Self::Quic(_) | Self::Ngtcp2(_) => MoqTransport::Quic,
            Self::WebTransport(_) => MoqTransport::WtH3,
            Self::WtH2(_) => MoqTransport::WtH2,
            Self::Qmux(_) => MoqTransport::Qmux,
        }
    }

    /// 接続ハンドルと accept 用の口へ分割する
    pub(crate) fn split(self) -> (MoqConnectionHandle, MoqRecvAcceptor, MoqBidiAcceptor) {
        match self {
            Self::Quic(connection) => {
                let (handle, acceptor) = connection.split();
                let (bidi, recv) = acceptor.split();
                (
                    MoqConnectionHandle::Quic(handle),
                    MoqRecvAcceptor::Quic(recv),
                    MoqBidiAcceptor::Quic(bidi),
                )
            }
            Self::Qmux(connection) => (
                MoqConnectionHandle::Qmux(Arc::clone(&connection)),
                MoqRecvAcceptor::Qmux(Arc::clone(&connection)),
                MoqBidiAcceptor::Qmux(connection),
            ),
            Self::Ngtcp2(connection) | Self::WebTransport(connection) => {
                let (handle, recv, bidi) = connection.split();
                (
                    MoqConnectionHandle::Ngtcp2(handle),
                    MoqRecvAcceptor::Ngtcp2(recv),
                    MoqBidiAcceptor::Ngtcp2(bidi),
                )
            }
            Self::WtH2(connection) => {
                let (handle, recv, bidi) = connection.split();
                (
                    MoqConnectionHandle::WtH2(handle),
                    MoqRecvAcceptor::WtH2(recv),
                    MoqBidiAcceptor::WtH2(bidi),
                )
            }
        }
    }
}

/// 接続先の名前解決と QUIC クライアントの構築を行う
///
/// QMux / ngtcp2 / WebTransport over HTTP/2 経路では共有 UDP ソケットを使わないため
/// `tls` に `None` を渡し、`clients` を空にする。
///
/// # Errors
///
/// 名前解決に失敗した場合、またはソケットのバインドに失敗した場合はエラーになる。
async fn build_endpoint_state(
    endpoint: &MoqEndpoint,
    tls: Option<&s2n_rustls::Client>,
) -> Result<EndpointState> {
    let addrs = resolve(&endpoint.host, endpoint.port).await?;
    let mut clients = Vec::new();
    if let Some(tls) = tls {
        for is_ipv6 in [false, true] {
            if !addrs.iter().any(|addr| addr.is_ipv6() == is_ipv6) {
                continue;
            }
            clients.push((is_ipv6, build_client(tls, is_ipv6)?));
        }
    }
    Ok(EndpointState {
        clients,
        server_name: endpoint.host.clone(),
        addrs,
    })
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

/// 指定したアドレスファミリ用の s2n-quic クライアントを構築する
///
/// # Errors
///
/// TLS 設定、UDP ソケットの作成、limits の設定、クライアントの起動に失敗した場合は
/// エラーになる。
fn build_client(tls: &s2n_rustls::Client, is_ipv6: bool) -> Result<Client> {
    let datagram_endpoint = s2n_quic::provider::datagram::default::Endpoint::builder()
        .with_recv_capacity(64)
        .map_err(|e| ErrorMessage::new(format!("datagram endpoint の構築に失敗しました: {e}")))?
        .build()
        .expect("datagram endpoint must build after recv capacity is set");

    let client = Client::builder()
        .with_tls(tls.clone())
        .map_err(|e| ErrorMessage::new(format!("TLS 設定に失敗しました: {e}")))?
        .with_io(local_bind_addr(is_ipv6))
        .map_err(|e| ErrorMessage::new(format!("UDP ソケットの作成に失敗しました: {e}")))?
        .with_datagram(datagram_endpoint)
        .map_err(|e| ErrorMessage::new(format!("datagram provider の設定に失敗しました: {e}")))?
        .with_limits(build_limits()?)
        .map_err(|e| ErrorMessage::new(format!("limits provider の設定に失敗しました: {e}")))?
        .start()
        .map_err(|e| ErrorMessage::new(format!("QUIC クライアントの起動に失敗しました: {e}")))?;
    Ok(client)
}

/// アドレスファミリに合わせたローカルバインドアドレスを返す
fn local_bind_addr(is_ipv6: bool) -> &'static str {
    if is_ipv6 { "[::]:0" } else { "0.0.0.0:0" }
}

/// rustls のクライアント設定を構築する (ALPN は呼び出し側が設定する)
///
/// # Errors
///
/// `--ca-cert` に指定した PEM の読み込みに失敗した場合、ルート証明書が 1 件も
/// 得られない場合、または TLS 設定の構築に失敗した場合はエラーになる。
fn build_rustls_config(options: &MoqTlsOptions) -> Result<rustls::ClientConfig> {
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

/// QUIC 直接接続用の TLS クライアントを構築する
///
/// # Errors
///
/// TLS 設定の構築に失敗した場合はエラーになる。
fn build_quic_tls(options: &MoqTlsOptions) -> Result<s2n_rustls::Client> {
    let mut config = build_rustls_config(options)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(s2n_rustls::Client::from(config))
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

/// QUIC の接続 limits を構築する
///
/// 既定値 (remote uni stream 100 本) のままだと、relay が開けるストリームの累積上限に
/// 達して購読が止まる。負荷試験では購読側の本数も増えるため余裕を持たせる。
///
/// # Errors
///
/// いずれかの limits 値が s2n-quic の許容範囲外の場合はエラーになる。
fn build_limits() -> Result<s2n_quic::provider::limits::Limits> {
    let limits = s2n_quic::provider::limits::Limits::new()
        .with_max_open_remote_unidirectional_streams(10_000)
        .map_err(|e| {
            ErrorMessage::new(format!("remote uni stream limit の設定に失敗しました: {e}"))
        })?
        .with_max_open_remote_bidirectional_streams(1_000)
        .map_err(|e| {
            ErrorMessage::new(format!(
                "remote bidi stream limit の設定に失敗しました: {e}"
            ))
        })?
        .with_data_window(64 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("data window の設定に失敗しました: {e}")))?
        .with_unidirectional_data_window(16 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("uni stream window の設定に失敗しました: {e}")))?
        .with_bidirectional_local_data_window(16 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("bidi local window の設定に失敗しました: {e}")))?
        .with_bidirectional_remote_data_window(16 * 1024 * 1024)
        .map_err(|e| ErrorMessage::new(format!("bidi remote window の設定に失敗しました: {e}")))?;
    Ok(limits)
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

    /// 接続先のアドレスファミリに合わせてローカルアドレスを選ぶこと
    #[test]
    fn local_bind_addr_matches_address_family() {
        assert_eq!(local_bind_addr(false), "0.0.0.0:0");
        assert_eq!(local_bind_addr(true), "[::]:0");
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
    /// QMux 上の MOQT は native QUIC と同じ `moqt-22` を使う。ALPN が無いと
    /// rustls のハンドシェイク自体は成功してしまい、QMux のトランスポートパラメータを
    /// 待ち続けるため、確実に載っていることを固定する。
    #[test]
    fn qmux_tls_advertises_moqt_alpn() {
        let tls = build_qmux_tls(&MoqTlsOptions::default()).expect("TLS 設定の構築に成功すること");
        assert_eq!(tls.alpn_protocols, vec![b"moqt-22".to_vec()]);
    }

    /// `--insecure` でも QMux の ALPN を広告すること
    #[test]
    fn qmux_tls_insecure_advertises_moqt_alpn() {
        let options = MoqTlsOptions {
            insecure: true,
            ca_cert: None,
        };
        let tls = build_qmux_tls(&options).expect("TLS 設定の構築に成功すること");
        assert_eq!(tls.alpn_protocols, vec![b"moqt-22".to_vec()]);
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
    /// s2n-quic のクライアントを持たせてはならない。
    #[tokio::test]
    async fn qmux_context_does_not_build_quic_clients() {
        let endpoint =
            MoqEndpoint::parse("moqt://127.0.0.1:4433/").expect("接続先の解釈に成功すること");
        let context = MoqClientContext::new(
            &endpoint,
            &MoqTlsOptions::default(),
            MoqTransport::Qmux,
            QuicBackend::default(),
        )
        .await
        .expect("コンテキストの構築に成功すること");
        assert!(context.quic_tls.is_none());
        assert!(context.qmux_tls.is_some());
        let endpoints = context.endpoints.lock().await;
        let state = endpoints
            .values()
            .next()
            .expect("接続先が登録されていること");
        assert!(
            state.clients.is_empty(),
            "QMux では QUIC クライアントを作らないこと"
        );
    }

    /// WebTransport over HTTP/2 の TLS 設定が `h2` を広告すること
    ///
    /// HTTP/2 の ALPN は `h2` である (RFC 9113 §3.3)。MOQT の ALPN (`moqt-22`) は
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
        let context = MoqClientContext::new(
            &endpoint,
            &MoqTlsOptions::default(),
            MoqTransport::WtH2,
            QuicBackend::default(),
        )
        .await
        .expect("コンテキストの構築に成功すること");
        assert!(context.quic_tls.is_none());
        assert!(context.ngtcp2_config.is_none());
        assert!(context.qmux_tls.is_none());
        assert!(context.wt_h2_tls.is_some());
        let endpoints = context.endpoints.lock().await;
        let state = endpoints
            .values()
            .next()
            .expect("接続先が登録されていること");
        assert!(
            state.clients.is_empty(),
            "WebTransport over HTTP/2 では QUIC クライアントを作らないこと"
        );
    }
}
