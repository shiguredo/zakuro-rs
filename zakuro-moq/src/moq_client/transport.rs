//! MOQ relay への QUIC 接続 (s2n-quic + rustls)
//!
//! MOQT の接続先 URL の解釈、名前解決、TLS 設定、s2n-quic のクライアント構築を担当する。

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use s2n_quic::Client;
use s2n_quic::client::Connect;
use s2n_quic::connection::Connection;
use s2n_quic::provider::tls::rustls as s2n_rustls;

use crate::error::{ErrorMessage, Result};

/// QUIC 直接接続の ALPN プロトコル識別子
///
/// draft-ietf-moq-transport-22 §6.2 (Session establishment) が定める MOQT の
/// プロトコル識別子。WebTransport 経路は `h3` / `h2` を使うが、zakuro は QUIC 直接接続のみ使う。
/// この値は draft 由来であり将来の draft 改訂で変わる可能性がある。
const ALPN: &[u8] = b"moqt-22";

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
        let rest = url.strip_prefix("moqt://").ok_or_else(|| {
            ErrorMessage::new(format!("--url は moqt:// で始まる必要があります: {url}"))
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
                ErrorMessage::new(format!("--url にホストが指定されていません: {url}")).into(),
            );
        }

        let (host, port) = split_host_port(authority)?;
        if host.is_empty() {
            return Err(
                ErrorMessage::new(format!("--url にホストが指定されていません: {url}")).into(),
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
/// 含む authority はポートの区切りと区別できないためエラーにする。
fn split_host_port(authority: &str) -> Result<(String, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            ErrorMessage::new(format!("--url の IPv6 アドレスが不正です: {authority}"))
        })?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            None
        } else {
            let value = after.strip_prefix(':').ok_or_else(|| {
                ErrorMessage::new(format!("--url のポート指定が不正です: {authority}"))
            })?;
            Some(parse_port(value, authority)?)
        };
        return Ok((host.to_string(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') {
                return Err(ErrorMessage::new(format!(
                    "--url の IPv6 アドレスはブラケットで囲む必要があります: {authority}"
                ))
                .into());
            }
            Ok((host.to_string(), Some(parse_port(port, authority)?)))
        }
        None => Ok((authority.to_string(), None)),
    }
}

/// ポート文字列を数値に変換する
fn parse_port(value: &str, authority: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .map_err(|_| ErrorMessage::new(format!("--url のポート指定が不正です: {authority}")).into())
}

/// MOQ relay へ接続するときの TLS 設定
#[derive(Debug, Clone, Default)]
pub(crate) struct MoqTlsOptions {
    /// 証明書検証をスキップする (`--insecure`)
    pub(crate) insecure: bool,
    /// 信頼する CA 証明書 (PEM) のパス (`--ca-cert`)
    ///
    /// 未指定の場合は WebPKI のルート証明書で検証する。
    pub(crate) ca_cert: Option<String>,
}

/// インスタンス内の仮想クライアントで共有する接続コンテキスト
///
/// s2n-quic の `Client` は 1 つの UDP ソケットとエンドポイントタスクを表す。仮想クライアントごとに
/// 作るとソケットとタスクが仮想クライアント数だけ増えるため、インスタンスで 1 つ作って clone する。
#[derive(Clone)]
pub(crate) struct MoqClientContext {
    /// アドレスファミリごとの s2n-quic クライアント (bool は IPv6 かどうか)
    ///
    /// UDP ソケットは異なるアドレスファミリ宛に送信できないため、名前解決の結果に含まれる
    /// ファミリごとにクライアント (ソケット) を作る。通常は片方だけになる。
    clients: Vec<(bool, Client)>,
    server_name: String,
    addrs: Vec<SocketAddr>,
}

impl MoqClientContext {
    /// 名前解決と s2n-quic クライアントの構築を行う
    ///
    /// # Errors
    ///
    /// 名前解決に失敗した場合、TLS 設定が不正な場合、ソケットのバインドに失敗した場合は
    /// エラーになる。
    pub(crate) async fn new(endpoint: &MoqEndpoint, tls_options: &MoqTlsOptions) -> Result<Self> {
        let addrs = resolve(&endpoint.host, endpoint.port).await?;
        if addrs.is_empty() {
            return Err(ErrorMessage::new("MOQ relay のアドレスが解決できませんでした").into());
        }

        let tls = build_tls(tls_options)?;
        let mut clients = Vec::new();
        for is_ipv6 in [false, true] {
            if !addrs.iter().any(|addr| addr.is_ipv6() == is_ipv6) {
                continue;
            }
            clients.push((is_ipv6, build_client(&tls, is_ipv6)?));
        }

        Ok(Self {
            clients,
            server_name: endpoint.host.clone(),
            addrs,
        })
    }

    /// 仮想クライアント 1 本分の QUIC 接続を確立する
    ///
    /// 名前解決で複数のアドレスが得られた場合は順に試す。全て失敗した場合は最後のエラーを返す。
    ///
    /// # Errors
    ///
    /// 全ての接続先への接続に失敗した場合はエラーになる。
    pub(crate) async fn connect(&self) -> Result<Connection> {
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

/// TLS クライアントを構築する
///
/// # Errors
///
/// `--ca-cert` に指定した PEM の読み込みに失敗した場合、または TLS 設定の構築に
/// 失敗した場合はエラーになる。
fn build_tls(options: &MoqTlsOptions) -> Result<s2n_rustls::Client> {
    if options.insecure {
        // 検証をスキップする場合でも、ALPN は MOQT のプロトコル識別子を広告する必要がある
        return insecure_tls_client();
    }

    if let Some(path) = options.ca_cert.as_deref() {
        let client = s2n_rustls::Client::builder()
            .with_certificate(Path::new(path))
            .map_err(|e| {
                ErrorMessage::new(format!(
                    "--ca-cert の CA 証明書の読み込みに失敗しました ({path}): {e}"
                ))
            })?
            .with_application_protocols([ALPN].iter())
            .map_err(|e| ErrorMessage::new(format!("ALPN の設定に失敗しました: {e}")))?
            .build()
            .map_err(|e| ErrorMessage::new(format!("TLS クライアントの構築に失敗しました: {e}")))?;
        return Ok(client);
    }

    // 既定は WebPKI のルート証明書で検証する。s2n-quic の rustls provider は
    // 信頼するルート証明書が無い ClientConfig を組み立てられないため、rustls の
    // ClientConfig を直接作って渡す
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|e| ErrorMessage::new(format!("TLS バージョンの設定に失敗しました: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(s2n_rustls::Client::from(config))
}

/// 証明書検証をスキップする TLS クライアントを構築する
///
/// `--insecure` 指定時の開発用経路である。検証をしない旨は呼び出し側で警告ログに出す。
fn insecure_tls_client() -> Result<s2n_rustls::Client> {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|e| ErrorMessage::new(format!("TLS バージョンの設定に失敗しました: {e}")))?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(NoVerifier))
    .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(s2n_rustls::Client::from(config))
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
}
