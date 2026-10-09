//! Sora から通知された ICE サーバーの URL を、指定したアドレスファミリに絞り込む
//!
//! Sora は TURN の URL をホスト名で通知する。ホスト名が A と AAAA の両方を持つ場合、
//! libwebrtc はローカルネットワークごとに IPv4 と IPv6 の relay 候補を作る。両方の経路で
//! メディアが届くと ICE の経路選択が「最後にデータが届いた方」へ切り替わり続けて発振し、
//! パケット損失と再送を増やしたすえに接続が切断される。
//!
//! `--sora-ice-address-family` を指定すると、通知された URL のホスト部を指定した
//! アドレスファミリのアドレスに解決してリテラルに置き換える。指定したファミリの
//! アドレスを持たない URL は使わない。未指定の場合は通知された URL をそのまま使う
//! (従来の動作)。
//!
//! `turns:` / `stuns:` (TLS) の URL は、ホスト部をアドレスに置き換えると TLS の証明書検証
//! (ホスト名一致) が通らなくなるため使わない。UDP / TCP の TURN はそのまま使える。

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};

use shiguredo_webrtc::{IceServer, rtc_log_info, rtc_log_warning};

/// ICE サーバーの URL に使うアドレスファミリ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IceAddressFamily {
    /// IPv4 のアドレスだけを使う
    Ipv4,
    /// IPv6 のアドレスだけを使う
    Ipv6,
}

impl IceAddressFamily {
    /// `--sora-ice-address-family` の値を解析する
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "ipv4" => Some(Self::Ipv4),
            "ipv6" => Some(Self::Ipv6),
            _ => None,
        }
    }

    /// ログに出す名前
    fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }

    /// このファミリのアドレスかどうか
    fn accepts(self, addr: IpAddr) -> bool {
        matches!(
            (self, addr),
            (Self::Ipv4, IpAddr::V4(_)) | (Self::Ipv6, IpAddr::V6(_))
        )
    }
}

/// ICE サーバーの URL (`turn:` / `turns:` / `stun:` / `stuns:`)
#[derive(Debug)]
struct IceServerUrl<'a> {
    /// スキーム (`turn` / `turns` / `stun` / `stuns`)
    scheme: &'a str,
    /// スキームとホスト部の間 (`//` が付く形式をそのまま残すため)
    prefix: &'a str,
    /// ホスト部 (IPv6 リテラルのブラケットは外したもの)
    host: &'a str,
    /// ポート番号の指定 (書き換え時にそのまま使うため文字列で保持する)
    port: Option<&'a str>,
    /// ポートの後ろ (`?transport=udp` など)
    params: &'a str,
}

impl<'a> IceServerUrl<'a> {
    /// URL を解析する。ICE サーバーの URL として解釈できない場合は None を返す
    fn parse(url: &'a str) -> Option<Self> {
        let (scheme, rest) = url.split_once(':')?;
        if !matches!(scheme, "turn" | "turns" | "stun" | "stuns") {
            return None;
        }
        let (prefix, rest) = match rest.strip_prefix("//") {
            Some(rest) => ("//", rest),
            None => ("", rest),
        };
        let (host, rest) = if let Some(rest) = rest.strip_prefix('[') {
            // IPv6 リテラル (`turn:[2001:db8::1]:3478?transport=udp`)
            let (host, rest) = rest.split_once(']')?;
            (host, rest)
        } else {
            let end = rest.find([':', '?']).unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        if host.is_empty() || host.contains('/') {
            return None;
        }
        let (port, params) = match rest.strip_prefix(':') {
            Some(rest) => {
                let end = rest.find('?').unwrap_or(rest.len());
                (Some(&rest[..end]), &rest[end..])
            }
            None => (None, rest),
        };
        if let Some(port) = port
            && port.parse::<u16>().is_err()
        {
            return None;
        }
        if !params.is_empty() && !params.starts_with('?') {
            return None;
        }
        Some(Self {
            scheme,
            prefix,
            host,
            port,
            params,
        })
    }

    /// ホスト部の解決に使うポート (指定が無ければスキームの既定値)
    fn port_or_default(&self) -> u16 {
        if let Some(port) = self.port
            && let Ok(port) = port.parse::<u16>()
        {
            return port;
        }
        match self.scheme {
            "turns" | "stuns" => 5349,
            _ => 3478,
        }
    }

    /// ホスト部を指定したアドレスに置き換えた URL を組み立てる
    fn with_host(&self, addr: IpAddr) -> String {
        let host = match addr {
            IpAddr::V4(addr) => addr.to_string(),
            IpAddr::V6(addr) => format!("[{addr}]"),
        };
        let port = match self.port {
            Some(port) => format!(":{port}"),
            None => String::new(),
        };
        format!(
            "{}:{}{}{}{}",
            self.scheme, self.prefix, host, port, self.params
        )
    }
}

/// ICE サーバーの URL を 1 つ選ぶ
///
/// `resolved` は URL のホスト部を解決した結果 (ホスト部が IP リテラルならその 1 件)。
/// 指定したファミリのアドレスが無い場合と、TLS の URL は None を返し、その URL は使わない。
pub(crate) fn select_url(
    url: &str,
    family: IceAddressFamily,
    resolved: &[IpAddr],
) -> Option<String> {
    let parsed = IceServerUrl::parse(url)?;
    // TLS の URL はホスト名で証明書を検証するため、アドレスに置き換えると接続できない
    if matches!(parsed.scheme, "turns" | "stuns") {
        return None;
    }
    let addr = resolved
        .iter()
        .copied()
        .find(|addr| family.accepts(*addr))?;
    Some(parsed.with_host(addr))
}

/// ホスト部を解決してから [`select_url`] を適用する
fn select_url_with_resolve(url: &str, family: IceAddressFamily) -> Option<String> {
    let parsed = IceServerUrl::parse(url)?;
    let resolved = resolve_host(parsed.host, parsed.port_or_default());
    select_url(url, family, &resolved)
}

/// ホスト名の解決結果 (ホスト名, ポート) ごとのキャッシュ
type HostAddressCache = HashMap<(String, u16), Vec<IpAddr>>;

/// ホスト部を解決する
///
/// IP リテラルはそのまま返す。解決結果はプロセス内でキャッシュする (仮想クライアントごとに
/// 同じホスト名を解決し直さないため)。解決に失敗した場合は空のリストを返し、キャッシュしない。
fn resolve_host(host: &str, port: u16) -> Vec<IpAddr> {
    if let Ok(addr) = host.parse::<IpAddr>() {
        return vec![addr];
    }
    static CACHE: OnceLock<Mutex<HostAddressCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (host.to_string(), port);
    if let Ok(cache) = cache.lock()
        && let Some(addrs) = cache.get(&key)
    {
        return addrs.clone();
    }
    let addrs: Vec<IpAddr> = (host, port)
        .to_socket_addrs()
        .map(|iter| iter.map(|addr| addr.ip()).collect())
        .unwrap_or_default();
    if addrs.is_empty() {
        rtc_log_warning!("Failed to resolve ICE server host '{host}'");
        return addrs;
    }
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, addrs.clone());
    }
    addrs
}

/// Sora から通知された ICE サーバーの URL を、指定したアドレスファミリに絞って追加する
///
/// `sora_sdk::SoraConnectionBuilder::ice_server_url_configurer` に渡す。
/// 選んだ URL と使わなかった URL は、最初の 1 回だけログに出す (接続ごとに出さない)。
pub(crate) fn configure_ice_server_urls(
    server_entry: &mut IceServer,
    urls: &[String],
    family: IceAddressFamily,
) {
    static LOGGED: OnceLock<()> = OnceLock::new();
    let log_once = LOGGED.set(()).is_ok();
    let mut selected: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for url in urls {
        match select_url_with_resolve(url, family) {
            Some(selected_url) => {
                if log_once {
                    selected.push(selected_url.clone());
                }
                server_entry.add_url(&selected_url);
            }
            None => {
                if log_once {
                    dropped.push(url.clone());
                }
            }
        }
    }
    if log_once {
        rtc_log_info!(
            "ICE server URLs for address family {}: using [{}], dropping [{}]",
            family.as_str(),
            selected.join(", "),
            dropped.join(", "),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// IPv4 のアドレスを作る
    fn v4(value: &str) -> IpAddr {
        IpAddr::V4(
            value
                .parse::<Ipv4Addr>()
                .expect("IPv4 として解析できること"),
        )
    }

    /// IPv6 のアドレスを作る
    fn v6(value: &str) -> IpAddr {
        IpAddr::V6(
            value
                .parse::<Ipv6Addr>()
                .expect("IPv6 として解析できること"),
        )
    }

    #[test]
    fn test_parse_ice_address_family() {
        assert_eq!(
            IceAddressFamily::parse("ipv4"),
            Some(IceAddressFamily::Ipv4)
        );
        assert_eq!(
            IceAddressFamily::parse("ipv6"),
            Some(IceAddressFamily::Ipv6)
        );
        assert_eq!(IceAddressFamily::parse("IPv4"), None);
        assert_eq!(IceAddressFamily::parse("4"), None);
        assert_eq!(IceAddressFamily::parse(""), None);
    }

    #[test]
    fn test_select_url_rewrites_hostname_to_ipv4_literal() {
        // ホスト名が A と AAAA の両方を持つ場合でも、指定したファミリのアドレスに置き換えること
        let addr = v4("203.0.113.5");
        assert_eq!(
            select_url(
                "turn:example.com:3478?transport=udp",
                IceAddressFamily::Ipv4,
                &[addr]
            ),
            Some("turn:203.0.113.5:3478?transport=udp".to_string()),
        );
    }

    #[test]
    fn test_select_url_rewrites_hostname_to_ipv6_literal() {
        // IPv6 はブラケットで囲んで組み立てること
        let addr = v6("2001:db8::1");
        assert_eq!(
            select_url(
                "turn:example.com:3478?transport=udp",
                IceAddressFamily::Ipv6,
                &[addr]
            ),
            Some("turn:[2001:db8::1]:3478?transport=udp".to_string()),
        );
    }

    #[test]
    fn test_select_url_picks_requested_family_from_dual_stack() {
        // A と AAAA の両方がある場合は、指定したファミリのアドレスを選ぶこと
        let addrs = [v6("2001:db8::1"), v4("203.0.113.5")];
        assert_eq!(
            select_url("turn:example.com:3478", IceAddressFamily::Ipv4, &addrs),
            Some("turn:203.0.113.5:3478".to_string()),
        );
        assert_eq!(
            select_url("turn:example.com:3478", IceAddressFamily::Ipv6, &addrs),
            Some("turn:[2001:db8::1]:3478".to_string()),
        );
    }

    #[test]
    fn test_select_url_drops_url_without_requested_family() {
        // 指定したファミリのアドレスが無い場合は使わないこと
        assert_eq!(
            select_url(
                "turn:example.com:3478",
                IceAddressFamily::Ipv4,
                &[v6("2001:db8::1")]
            ),
            None,
        );
        assert_eq!(
            select_url(
                "turn:example.com:3478",
                IceAddressFamily::Ipv6,
                &[v4("203.0.113.5")]
            ),
            None,
        );
        assert_eq!(
            select_url("turn:example.com:3478", IceAddressFamily::Ipv4, &[]),
            None,
        );
    }

    #[test]
    fn test_select_url_keeps_port_and_params() {
        // ポートとパラメータをそのまま残すこと
        assert_eq!(
            select_url(
                "turn:example.com:3478?transport=tcp",
                IceAddressFamily::Ipv4,
                &[v4("203.0.113.5")]
            ),
            Some("turn:203.0.113.5:3478?transport=tcp".to_string()),
        );
        // ポート省略時はポートを付けないこと
        assert_eq!(
            select_url(
                "stun:example.com",
                IceAddressFamily::Ipv4,
                &[v4("203.0.113.5")]
            ),
            Some("stun:203.0.113.5".to_string()),
        );
        // `//` 付きの形式もそのまま残すこと
        assert_eq!(
            select_url(
                "turn://example.com:3478?transport=udp",
                IceAddressFamily::Ipv4,
                &[v4("203.0.113.5")]
            ),
            Some("turn://203.0.113.5:3478?transport=udp".to_string()),
        );
    }

    #[test]
    fn test_select_url_drops_tls_url() {
        // TLS の URL はホスト名の置き換えと両立しないため使わないこと
        assert_eq!(
            select_url(
                "turns:example.com:443?transport=tcp",
                IceAddressFamily::Ipv4,
                &[v4("203.0.113.5")]
            ),
            None,
        );
        assert_eq!(
            select_url(
                "stuns:example.com:5349",
                IceAddressFamily::Ipv4,
                &[v4("203.0.113.5")]
            ),
            None,
        );
    }

    #[test]
    fn test_select_url_rejects_unsupported_url() {
        // ICE サーバーの URL として解釈できない場合は使わないこと
        for url in [
            "",
            "https://example.com",
            "turn:",
            "turn:example.com:not-a-port",
            "turn:example.com/3478",
            "turn:[2001:db8::1:3478",
        ] {
            assert_eq!(
                select_url(url, IceAddressFamily::Ipv4, &[v4("203.0.113.5")]),
                None,
                "解釈できない URL は使わないこと: {url}",
            );
        }
    }

    #[test]
    fn test_select_url_accepts_ipv6_literal_host() {
        // ホスト部が IPv6 リテラルの場合も解析できること
        assert_eq!(
            select_url(
                "turn:[2001:db8::1]:3478?transport=udp",
                IceAddressFamily::Ipv6,
                &[v6("2001:db8::1")]
            ),
            Some("turn:[2001:db8::1]:3478?transport=udp".to_string()),
        );
        assert_eq!(
            select_url(
                "turn:[2001:db8::1]:3478",
                IceAddressFamily::Ipv4,
                &[v6("2001:db8::1")]
            ),
            None,
        );
    }

    #[test]
    fn test_resolve_host_returns_loopback_for_localhost() {
        // ホスト名の解決はシステムのリゾルバを使う (IP リテラルは解決しない)
        let addrs = resolve_host("localhost", 3478);
        assert!(
            addrs.iter().any(|addr| addr.is_loopback()),
            "localhost はループバックアドレスに解決されること: {addrs:?}",
        );
        assert_eq!(
            resolve_host("203.0.113.5", 3478),
            vec![v4("203.0.113.5")],
            "IP リテラルはそのまま返すこと",
        );
    }

    #[test]
    fn test_select_url_with_resolve_rewrites_hostname() {
        // ホスト名を解決する経路でも、指定したファミリのアドレスに置き換えること
        // (`localhost` は /etc/hosts で解決できるため、外部の DNS に依存しない)
        let resolved = resolve_host("localhost", 3478);
        if let Some(addr) = resolved.iter().find(|addr| addr.is_ipv4()) {
            assert_eq!(
                select_url_with_resolve(
                    "turn:localhost:3478?transport=udp",
                    IceAddressFamily::Ipv4
                ),
                Some(format!("turn:{addr}:3478?transport=udp")),
            );
        }
    }
}
