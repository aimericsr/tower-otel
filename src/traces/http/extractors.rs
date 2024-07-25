use axum::extract::{ConnectInfo, MatchedPath};
use http::{HeaderMap, HeaderValue, Request, Response, Version};
use opentelemetry::{propagation::Injector, trace::Status};
use std::net::SocketAddr;
use tracing::{field::Empty, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::service::OtelError;

pub fn extract_otel_info_from_req<B>(req: &Request<B>) -> Span {
    let request_method = req.method().as_str();
    let (server_adress, server_port) = extract_server_address_and_port(&req);

    let user_agent = req
        .headers()
        .get(http::header::USER_AGENT)
        .and_then(|h| h.to_str().ok());

    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or(req.uri().path(), |mp| mp.as_str());

    let (client_adress, client_port) = extract_client_address_and_port(&req);
    let _network_protocol_version = get_http_version(&req);

    let span = tracing::span!(tracing::Level::INFO, "OTEL HTTP",
        // network.local.address = server_adress,
        // network.local.port = server_port,
        // network.transport = "tcp",
        // network.protocol.version = network_protocol_version,
        // network.protocol.name = "http",
        // network.peer.address = server_adress,
        // network.peer.port = server_port,
        server.address = server_adress,
        server.port = server_port,
        client.address =  client_adress,
        client.port = client_port,
        http.request.method = request_method,
        //http.request.method_original = request_method,
        http.route = route,
        http.response.status_code = Empty,
        user_agent.original = user_agent,
        url.path = req.uri().path(),
        url.scheme = "http",
        url.query = req.uri().query(),
        otel.name = format!("{request_method} {route}"),
        otel.kind = ?opentelemetry::trace::SpanKind::Server,
        otel.status_code = ?Status::default(),
        otel.status_message = Empty,
        //"error.type" = Empty,
        // "exception.type" = Empty,
        // exception.message = Empty,
        // exception.escaped = Empty,
        // exception.stacktrace = Empty,
    );

    for (header_name, header_value) in req.headers().iter() {
        if let Ok(attribute_value) = header_value.to_str() {
            let attribute_name = format!("http.request.header.{}", header_name);
            span.set_attribute(attribute_name, attribute_value.to_owned());
        }
    }

    span
}

pub fn update_span_on_success<B>(req: &mut Response<B>, span: &Span) {
    span.record("http.response.status_code", req.status().as_u16());
    span.record("otel.status_code", "OK");
    update_span_with_headers(req.headers(), span);
    inject_trace_id(req.headers_mut());
}

pub fn update_span_on_error<B>(req: &mut Response<B>, span: &Span) {
    span.record("http.response.status_code", req.status().as_u16());
    span.record("otel.status_code", "ERROR");
    update_span_with_headers(req.headers(), span);
    inject_trace_id(req.headers_mut());
    //span.record("error.type", err.types.clone());
    // span.record("exception.type", err.types.clone());
    // span.record("exception.message", err.messages.clone());
    // span.record("exception.escaped", false);
    // span.record("exception.stacktrace", "je suis une stacktrace");
}

fn inject_trace_id(headers: &mut http::HeaderMap) {
    let context = tracing::Span::current().context();

    let mut injector = HeaderInjector(headers);
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut injector);
    });
}

pub struct HeaderInjector<'a>(pub &'a mut http::HeaderMap);

impl<'a> Injector for HeaderInjector<'a> {
    /// Set a key and value in the `HeaderMap`. Does nothing if the key or value are not valid inputs.
    fn set(&mut self, key: &str, value: String) {
        if let Ok(name) = http::header::HeaderName::from_bytes(key.as_bytes()) {
            if let Ok(val) = http::header::HeaderValue::from_str(&value) {
                self.0.insert(name, val);
            }
        }
    }
}

/// set_attribute is used instead of record so that we can set attribute that were not here at the span creation
fn update_span_with_headers(headers: &HeaderMap<HeaderValue>, span: &Span) {
    for (header_name, header_value) in headers.iter() {
        if let Ok(attribute_value) = header_value.to_str() {
            let attribute_name = format!("http.response.header.{}", header_name);
            span.set_attribute(attribute_name, attribute_value.to_owned());
        }
    }
}

/// Extract the server host and port following the (OTEL)[https://opentelemetry.io/docs/specs/semconv/http/http-spans/#setting-serveraddress-and-serverport-attributes]
/// Only IPV4 is supported for the moment.
fn extract_server_address_and_port<B>(req: &Request<B>) -> (Option<String>, Option<String>) {
    // Try to get the Forwarded header
    if let Some(forwarded) = req.headers().get(http::header::FORWARDED) {
        if let Ok(forwarded_str) = forwarded.to_str() {
            return parse_forwarded_header(forwarded_str);
        }
    }

    // Try to get the X-Forwarded-Host header
    if let Some(x_forwarded_host) = req.headers().get("x-forwarded-host") {
        if let Ok(x_forwarded_host_str) = x_forwarded_host.to_str() {
            return split_host_and_port(x_forwarded_host_str);
        }
    }

    // Try to get the :authority pseudo header for HTTP2/3
    if let Some(authority) = req.uri().authority() {
        return (
            Some(authority.host().into()),
            authority.port().map(|p| p.to_string()),
        );
    }

    // Default to the Host header
    if let Some(host) = req.headers().get(http::header::HOST) {
        if let Ok(host_str) = host.to_str() {
            return split_host_and_port(host_str);
        }
    }

    (None, None)
}

fn parse_forwarded_header(header_value: &str) -> (Option<String>, Option<String>) {
    if let Some(start) = header_value.find("for=") {
        let start = start + 4;
        let end = header_value[start..]
            .find([';', ','])
            .map_or(header_value.len(), |i| start + i);
        let for_value = &header_value[start..end].trim();
        return split_host_and_port(for_value);
    }
    (None, None)
}

fn split_host_and_port(host: &str) -> (Option<String>, Option<String>) {
    if let Some((address, port)) = host.split_once(':') {
        (Some(address.to_string()), Some(port.to_string()))
    } else {
        (Some(host.to_string()), None)
    }
}

fn extract_client_address_and_port<B>(req: &Request<B>) -> (Option<String>, Option<String>) {
    let client_adress = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .and_then(|x| Some(x.ip().to_string()));
    let client_port = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .and_then(|x| Some(x.port().to_string()));
    (client_adress, client_port)
}

pub fn get_http_version<B>(req: &Request<B>) -> Option<&'static str> {
    match req.version() {
        Version::HTTP_09 => Some("0.9"),
        Version::HTTP_10 => Some("1.0"),
        Version::HTTP_11 => Some("1.1"),
        Version::HTTP_2 => Some("2"),
        Version::HTTP_3 => Some("3"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;

    fn create_request_with_header(header_name: &str, header_value: &str) -> Request<()> {
        Request::builder()
            .header(header_name, header_value)
            .body(())
            .unwrap()
    }

    #[test]
    fn test_forwarded_header_basic() {
        let req = create_request_with_header("Forwarded", "for=192.0.2.1");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("192.0.2.1".to_string()));
        assert_eq!(port, None);
    }

    #[test]
    fn test_forwarded_header_with_port() {
        let req = create_request_with_header("Forwarded", "for=192.0.2.1:8080");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("192.0.2.1".to_string()));
        assert_eq!(port, Some("8080".to_string()));
    }

    #[test]
    fn test_forwarded_header_multiple_values() {
        let req =
            create_request_with_header("Forwarded", "for=192.0.2.60;proto=http;by=203.0.113.43");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("192.0.2.60".to_string()));
        assert_eq!(port, None);
    }

    #[test]
    fn test_forwarded_header_multiple_proxies() {
        let req = create_request_with_header("Forwarded", "for=192.0.2.43, for=198.51.100.17");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("192.0.2.43".to_string()));
        assert_eq!(port, None);
    }

    #[test]
    fn test_x_forwarded_host_header() {
        let req = create_request_with_header("X-Forwarded-Host", "example.com:443");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("example.com".to_string()));
        assert_eq!(port, Some("443".to_string()));
    }

    #[test]
    fn test_x_forwarded_host_header_no_port() {
        let req = create_request_with_header("X-Forwarded-Host", "example.com");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("example.com".to_string()));
        assert_eq!(port, None);
    }

    #[test]
    fn test_authority_header_with_port() {
        let req = Request::builder()
            .uri("http://example.com:443")
            .body(())
            .unwrap();
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("example.com".to_string()));
        assert_eq!(port, Some("443".to_string()));
    }

    #[test]
    fn test_authority_header_no_port() {
        let req = Request::builder()
            .uri("http://example.com")
            .body(())
            .unwrap();
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("example.com".to_string()));
        assert_eq!(port, None);
    }

    #[test]
    fn test_host_header_with_port() {
        let req = create_request_with_header("Host", "example.com:80");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("example.com".to_string()));
        assert_eq!(port, Some("80".to_string()));
    }

    #[test]
    fn test_host_header_no_port() {
        let req = create_request_with_header("Host", "example.com");
        let (address, port) = extract_server_address_and_port(&req);
        assert_eq!(address, Some("example.com".to_string()));
        assert_eq!(port, None);
    }
}
