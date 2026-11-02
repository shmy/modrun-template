use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use config::TrustedProxies;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};

/// 运行时状态需提供可信代理列表，供 [`ClientIp`] 提取（避免绑定具体 `WebState`）。
pub trait HasTrustedProxies {
    fn trusted_proxies(&self) -> &TrustedProxies;
}

#[derive(Copy, Clone, Debug)]
pub struct ClientIp(pub IpAddr);

impl ClientIp {
    /// 限流等场景用的稳定 cache key。
    pub fn rate_limit_key(self) -> String {
        self.0.to_string()
    }
}

/// 从请求部件解析客户端 IP（与 [`ClientIp`] 提取器、`TRUSTED_PROXIES` 语义一致）。
pub fn client_ip_from_parts(parts: &Parts, trusted: &TrustedProxies) -> IpAddr {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    resolve_client_ip(peer, &parts.headers, trusted)
}

impl<S> FromRequestParts<S> for ClientIp
where
    S: HasTrustedProxies + Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(client_ip_from_parts(
            parts,
            state.trusted_proxies(),
        )))
    }
}

/// 按「信任边界」从右往左解析代理链。
///
/// 仅当直连对端落在 `TRUSTED_PROXIES` 内才解析代理头（否则直接返回对端地址，
/// 防止任意客户端伪造）。解析时**从右往左**逐跳跳过可信代理，取第一个非可信地址：
/// 反代通常只在右侧**追加**地址而不会清洗左侧，因此最左值可被客户端完全控制。
///
/// 取最左值会导致限流被绕过：攻击者只需每次带不同的 `X-Forwarded-For: 1.2.3.4`
/// 即可获得无限配额（实测伪造头可让 `RATE_LIMIT_AUTH_*` 完全失效）。
pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &TrustedProxies,
) -> IpAddr {
    let peer = peer.unwrap_or(IpAddr::from([0, 0, 0, 0]));
    if trusted.is_empty() || !trusted.contains(peer) {
        return peer;
    }

    // 链式头优先于单值头：若反代误把客户端可控的 `X-Real-IP` 原样转发，
    // 而 `X-Forwarded-For` 才是可信追加链，取错头会让限流键被伪造。
    for name in TRUSTED_IP_HEADERS {
        let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) else {
            continue;
        };
        if let Some(ip) = client_ip_from_forwarded_chain(value, trusted) {
            return ip;
        }
    }
    peer
}

/// 代理头白名单（按优先级）。
///
/// 链式头（`X-Forwarded-For` / `Forwarded`）优先：可做从右往左信任扫描。
/// 单值头（`X-Real-IP` / `CF-Connecting-IP`）仅在没有可用链式头时回退。
const TRUSTED_IP_HEADERS: [&str; 6] = [
    "x-forwarded-for",
    "forwarded",
    "x-forwarded",
    "forwarded-for",
    "x-real-ip",
    "cf-connecting-ip",
];

/// 从右往左扫描转发链，返回第一个**非可信**地址。
///
/// 例：`TRUSTED_PROXIES=127.0.0.1,10.0.0.0/8`
/// - `"1.2.3.4"` → `1.2.3.4`
/// - `"1.2.3.4, 10.0.0.5"` → `1.2.3.4`（右端是内网代理，跳过）
/// - `"9.9.9.9, 1.2.3.4"` → `1.2.3.4`（右端即首个非可信，客户端伪造的左端 9.9.9.9 被忽略）
///
/// 整条链均为可信地址时返回 `None`，由调用方回退到 `peer` / 下一头——
/// 若退回链上某个代理 IP，所有用户会共用一个限流键。
fn client_ip_from_forwarded_chain(value: &str, trusted: &TrustedProxies) -> Option<IpAddr> {
    // 右 → 左
    for segment in value.split(',').rev() {
        let segment = segment.trim();
        // `Forwarded` 头形态：`for=1.2.3.4;proto=https`，取 `for=` 段。
        let raw = segment
            .split(';')
            .find_map(|kv| {
                let kv = kv.trim();
                kv.strip_prefix("for=")
                    .or_else(|| kv.strip_prefix("for=\""))
            })
            .unwrap_or(segment)
            .trim_matches('"');

        let Ok(ip) = parse_forwarded_addr(raw) else {
            continue;
        };
        // 跳过可信代理（含直连对端自身可能被重复写入的情况）。
        if trusted.contains(ip) {
            continue;
        }
        return Some(ip);
    }
    None
}

/// 解析转发链中的单个地址，兼容三种常见写法：
/// - 裸 IPv4 `1.2.3.4`
/// - 带端口 `1.2.3.4:5678`
/// - 方括号 IPv6 `[2001:db8::1]:443` / 裸 IPv6 `2001:db8::1`
fn parse_forwarded_addr(raw: &str) -> Result<IpAddr, ()> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return Err(());
    }
    // `[v6]:port` —— 只取括号内，避免被 IPv6 的冒号误切。
    if let Some(rest) = raw.strip_prefix('[') {
        let inner = rest.split(']').next().unwrap_or("");
        return inner.parse::<IpAddr>().map_err(|_| ());
    }
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Ok(ip);
    }
    // `v4:port`：仅当冒号后是纯数字时才当作端口（IPv6 裸写法含多个冒号）。
    if let Some((host, port)) = raw.rsplit_once(':')
        && port.chars().all(|c| c.is_ascii_digit())
        && let Ok(ip) = host.parse::<IpAddr>()
    {
        return Ok(ip);
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn trusted_local() -> TrustedProxies {
        TrustedProxies::parse("127.0.0.1")
    }

    fn trusted_local_and_private() -> TrustedProxies {
        TrustedProxies::parse("127.0.0.1,10.0.0.0/8")
    }

    fn headers_with_xff(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
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
    fn uses_rightmost_untrusted_hop() {
        // 右端 10.0.0.1 是可信代理 → 跳过；再往左是非可信客户端。
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers_with_xff("198.51.100.20, 10.0.0.1"),
            &trusted_local_and_private(),
        );
        assert_eq!(ip, "198.51.100.20".parse::<IpAddr>().unwrap());
    }

    /// 核心回归：客户端在左端伪造任意地址不得影响限流键。
    ///
    /// 反代只追加右侧地址，左端是客户端自己写的。修复前取最左值，
    /// 攻击者每次换一个即可无限绕过 `RATE_LIMIT_AUTH_*`。
    #[test]
    fn leftmost_spoofed_entry_is_ignored() {
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers_with_xff("9.9.9.9, 1.2.3.4"),
            &trusted_local(),
        );
        assert_eq!(
            ip,
            "1.2.3.4".parse::<IpAddr>().unwrap(),
            "must use the hop appended by the trusted proxy, not the client-supplied leftmost value"
        );
    }

    #[test]
    fn uses_x_real_ip_from_trusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "198.51.100.20".parse().unwrap());
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers,
            &trusted_local(),
        );
        assert_eq!(ip, "198.51.100.20".parse::<IpAddr>().unwrap());
    }

    /// 同时存在链式头与单值头时，以 `X-Forwarded-For` 为准，忽略可被客户端
    /// 污染的 `X-Real-IP`。
    #[test]
    fn prefers_xff_chain_over_spoofed_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "9.9.9.9".parse().unwrap());
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers,
            &trusted_local(),
        );
        assert_eq!(ip, "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    /// 整条转发链都是可信代理时，回退到直连 peer，避免所有用户共用代理 IP 限流键。
    #[test]
    fn all_trusted_chain_falls_back_to_peer() {
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers_with_xff("10.0.0.5, 10.0.0.1"),
            &trusted_local_and_private(),
        );
        assert_eq!(ip, "127.0.0.1".parse::<IpAddr>().unwrap());
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

    /// `Forwarded`（RFC 7239）形态：`for=1.2.3.4;proto=https`。
    #[test]
    fn parses_rfc7239_forwarded_header() {
        let mut headers = HeaderMap::new();
        headers.insert("forwarded", "for=1.2.3.4;proto=https".parse().unwrap());
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers,
            &trusted_local(),
        );
        assert_eq!(ip, "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    /// IPv6 在转发链中常写作 `[::1]:port`。
    #[test]
    fn parses_bracketed_ipv6_with_port() {
        let ip = resolve_client_ip(
            Some("127.0.0.1".parse().unwrap()),
            &headers_with_xff("[2001:db8::1]:443"),
            &trusted_local(),
        );
        assert_eq!(ip, "2001:db8::1".parse::<IpAddr>().unwrap());
    }
}
