use std::net::IpAddr;
use std::num::NonZeroU32;

use axum::body::Body;
use axum::http::{Response, StatusCode, request::Parts};
use axum_governor::boxed::BoxedGovernorLayer;
use axum_governor::extractor::{KeyExtractor, KeyOutcome};
use axum_governor::{
    ExtractionError, GovernorConfigBuilder, GovernorLayer, Quota, RejectionReason,
};
use config::TrustedProxies;
use config::cli::Cli;
use web_error::WebError;
use web_state::extractor::client_ip::client_ip_from_parts;

#[derive(Clone)]
struct TrustedClientIp {
    trusted: TrustedProxies,
}

impl KeyExtractor for TrustedClientIp {
    type Key = IpAddr;

    fn requires_connect_info(&self) -> bool {
        true
    }

    fn extract(&self, parts: &Parts) -> Result<KeyOutcome<Self::Key>, ExtractionError> {
        Ok(KeyOutcome::new(client_ip_from_parts(parts, &self.trusted)))
    }
}

fn nonzero_u32(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}

fn quota(per_second: u32, burst: u32) -> Quota {
    Quota::requests_per_second(nonzero_u32(per_second)).burst(nonzero_u32(burst))
}

fn l10n_rate_limit_response(_reason: RejectionReason) -> Response<Body> {
    #[cfg(feature = "otel")]
    trace_kit::metrics::record_rate_limited();
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
    response
        .extensions_mut()
        .insert(WebError::L10n("rate_limited".to_string()));
    response
}

fn build_config(
    trusted: &TrustedProxies,
    per_second: u32,
    burst: u32,
) -> Result<axum_governor::GovernorConfig<IpAddr>, String> {
    GovernorConfigBuilder::default()
        .with_extractor(TrustedClientIp {
            trusted: trusted.clone(),
        })
        .expect_connect_info()
        .quota_default(quota(per_second, burst))
        .error_handler(l10n_rate_limit_response)
        .finish()
        .map_err(|err| format!("rate limit config: {err}"))
}

pub fn global_layer(cli: &Cli) -> Result<GovernorLayer<IpAddr>, String> {
    let trusted = TrustedProxies::parse(&cli.server.trusted_proxies);
    build_config(
        &trusted,
        cli.rate_limit.global_per_second,
        cli.rate_limit.global_burst,
    )
    .map(GovernorLayer::new)
}

pub fn auth_layer(cli: &Cli) -> Result<BoxedGovernorLayer, String> {
    let trusted = TrustedProxies::parse(&cli.server.trusted_proxies);
    build_config(
        &trusted,
        cli.rate_limit.auth_per_second,
        cli.rate_limit.auth_burst,
    )
    .map(BoxedGovernorLayer::from_config)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::{IpAddr, SocketAddr};

    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use config::TrustedProxies;

    use super::*;

    #[test]
    fn trusted_client_ip_ignores_spoofed_xff_without_trusted_peer() {
        let extractor = TrustedClientIp {
            trusted: TrustedProxies::parse("127.0.0.1"),
        };
        let mut request = Request::builder()
            .header("x-forwarded-for", "198.51.100.20")
            .body(())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 10], 12345))));
        let (parts, _) = request.into_parts();
        let key = extractor.extract(&parts).unwrap().key;
        assert_eq!(key, "203.0.113.10".parse::<IpAddr>().unwrap());
    }
}
