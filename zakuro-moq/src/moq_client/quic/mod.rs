//! ネイティブ QUIC のバックエンド選択
//!
//! QUIC 直接接続の実装は Cargo feature で選ぶ。次の 3 つからちょうど 1 つだけを
//! 有効にすること。
//!
//! - `quic-tobi` (既定): tobi-quic
//! - `quic-s2n`: s2n-quic
//! - `quic-ngtcp2`: ngtcp2 (shiguredo_ngtcp2_tokio)
//!
//! WebTransport over HTTP/3 は tobi-webtransport を直接使うため、この選択の対象外で
//! ある。そのため `tobi_quic` はバックエンドに関わらず常に依存に含まれる。

use std::net::SocketAddr;

#[cfg(feature = "quic-tobi")]
pub(super) mod tobi;

#[cfg(feature = "quic-s2n")]
pub(super) mod s2n;

#[cfg(feature = "quic-ngtcp2")]
pub(super) mod ngtcp2;

// ネイティブ QUIC のバックエンドが 2 つ以上、または 0 個の場合はビルドを止める
#[cfg(not(any(
    all(
        feature = "quic-tobi",
        not(feature = "quic-s2n"),
        not(feature = "quic-ngtcp2")
    ),
    all(
        feature = "quic-s2n",
        not(feature = "quic-tobi"),
        not(feature = "quic-ngtcp2")
    ),
    all(
        feature = "quic-ngtcp2",
        not(feature = "quic-tobi"),
        not(feature = "quic-s2n")
    ),
)))]
compile_error!(
    "exactly one native QUIC backend must be selected: quic-tobi / quic-s2n / quic-ngtcp2"
);

/// アドレスファミリに合わせたローカルバインドアドレスを返す
///
/// 1 つの UDP ソケットは別のアドレスファミリ宛に送信できないため、接続先の
/// 名前解決結果に含まれるファミリごとにソケットを作る。
pub(crate) fn local_bind_addr(is_ipv6: bool) -> SocketAddr {
    if is_ipv6 {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0))
    } else {
        SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0))
    }
}
