use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use config::TrustedProxies;
use real::extractor::{HeaderMap, IpExtractor};

#[derive(Copy, Clone, Debug)]
pub struct ClientIp(pub IpAddr);

impl ClientIp {
    /// 限流等场景用的稳定 cache key。
    pub fn rate_limit_key(self) -> String {
        self.0.to_string()
    }
}

/// 从请求部件解析客户端 IP（与 `ClientIp` 提取器、`TRUSTED_PROXIES` 语义一致）。
pub fn client_ip_from_parts(parts: &Parts, trusted: &TrustedProxies) -> IpAddr {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    resolve_client_ip(peer, &header_map_from_parts(parts), trusted)
}

impl FromRequestParts<crate::WebState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::WebState,
    ) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(client_ip_from_parts(
            parts,
            &state.trusted_proxies,
        )))
    }
}

/// real 会读取的代理头白名单（与 `real` 默认集合对齐）；只拷贝这些头，
/// 避免每请求把全部 header 复制成 String。
const CLIENT_IP_HEADERS: [&str; 6] = [
    "x-real-ip",
    "cf-connecting-ip",
    "x-forwarded-for",
    "x-forwarded",
    "forwarded-for",
    "forwarded",
];

fn header_map_from_parts(parts: &Parts) -> HeaderMap {
    let mut map = HeaderMap::new();
    for name in CLIENT_IP_HEADERS {
        if let Some(value) = parts.headers.get(name).and_then(|v| v.to_str().ok()) {
            map.insert(name.to_string(), value.to_string());
        }
    }
    map
}

static IP_EXTRACTOR: OnceLock<IpExtractor> = OnceLock::new();

fn ip_extractor() -> &'static IpExtractor {
    IP_EXTRACTOR.get_or_init(|| {
        IpExtractor::new()
            .with_headers(CLIENT_IP_HEADERS.map(str::to_string).to_vec())
            .trust_private_ips(true)
            .use_first_forwarded(true)
    })
}

/// 仅当直连对端在可信代理列表内时，才用 `real` 从代理头解析客户端 IP。
pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &TrustedProxies,
) -> IpAddr {
    let peer = peer.unwrap_or(IpAddr::from([0, 0, 0, 0]));
    if trusted.is_empty() || !trusted.contains(peer) {
        return peer;
    }
    ip_extractor()
        .extract(headers, Some(peer.to_string()))
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted_local() -> TrustedProxies {
        TrustedProxies::parse("127.0.0.1")
    }

    fn headers_with_xff(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for".to_string(), value.to_string());
        headers
    }

    #[test]
    fn ignores_forwarded_headers_without_trusted_peer() {
        let ip = resolve_client_ip(
            Some("203.0.113.10".parse().unwrap()),
            &headers_with_xff("198.51.100.20"),
            &trusted_local(),
        );
        assert_eq!(ip, "203.0.113.10".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn ignores_spoofed_headers_from_untrusted_peer() {
        let ip = resolve_client_ip(
            Some("198.51.100.20".parse().unwrap()),
            &headers_with_xff("1.2.3.4"),
            &trusted_local(),
        );
        assert_eq!(ip, "198.51.100.20".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn uses_leftmost_x_forwarded_for_from_trusted_peer() {
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers_with_xff("198.51.100.20, 10.0.0.1"),
            &trusted_local(),
        );
        assert_eq!(ip, "198.51.100.20".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn uses_x_real_ip_from_trusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip".to_string(), "198.51.100.20".to_string());
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers,
            &trusted_local(),
        );
        assert_eq!(ip, "198.51.100.20".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn falls_back_to_peer_when_headers_missing() {
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &HeaderMap::new(),
            &trusted_local(),
        );
        assert_eq!(ip, "127.0.0.1".parse::<IpAddr>().unwrap());
    }
}
