use http_1::HeaderMap;

const REQUEST_ID_HEADER: &str = "x-request-id";
const TRACEPARENT_HEADER: &str = "traceparent";

pub fn extract_trace_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_traceparent_trace_id)
        .or_else(|| {
            headers
                .get(REQUEST_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .and_then(normalize_trace_id)
        })
        .or_else(current_span_trace_id)
}

fn parse_traceparent_trace_id(traceparent: &str) -> Option<String> {
    let mut parts = traceparent.split('-');
    let version = parts.next()?;
    let trace_id = parts.next()?;
    let parent_id = parts.next()?;
    let flags = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !is_hex_len(version, 2) {
        return None;
    }
    if !is_hex_len(flags, 2) {
        return None;
    }
    // Parent id must be 16 lowercase/uppercase hex and not all zero.
    if !is_hex_len(parent_id, 16) || parent_id.as_bytes().iter().all(|b| *b == b'0') {
        return None;
    }
    normalize_trace_id(trace_id)
}

fn normalize_trace_id(value: &str) -> Option<String> {
    let trace_id = value.trim().to_ascii_lowercase();
    if trace_id.len() != 32 {
        return None;
    }
    if !trace_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if trace_id.as_bytes().iter().all(|b| *b == b'0') {
        return None;
    }
    Some(trace_id)
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(feature = "trace_id")]
fn current_span_trace_id() -> Option<String> {
    use opentelemetry::trace::TraceContextExt;
    use tracing::Span;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let cx = Span::current().context();
    let span = cx.span();
    let span_ctx = span.span_context();
    if span_ctx.is_valid() {
        Some(span_ctx.trace_id().to_string())
    } else {
        None
    }
}

#[cfg(not(feature = "trace_id"))]
fn current_span_trace_id() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_traceparent_returns_pure_trace_id() {
        let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let parsed = parse_traceparent_trace_id(traceparent);
        assert_eq!(parsed.as_deref(), Some("4bf92f3577b34da6a3ce929d0e0e4736"));
    }

    #[test]
    fn test_parse_traceparent_rejects_invalid_parent_id_or_flags() {
        let bad_parent = "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01";
        assert!(parse_traceparent_trace_id(bad_parent).is_none());

        let bad_flags = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-zz";
        assert!(parse_traceparent_trace_id(bad_flags).is_none());
    }

    #[test]
    fn test_normalize_trace_id_rejects_invalid_input() {
        assert!(normalize_trace_id("not-a-trace-id").is_none());
        assert!(normalize_trace_id("00000000000000000000000000000000").is_none());
        assert!(normalize_trace_id("abcd").is_none());
    }
}
