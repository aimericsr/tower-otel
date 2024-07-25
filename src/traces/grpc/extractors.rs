use http::{HeaderMap, HeaderValue, Request, Response, Uri, Version};
use opentelemetry::{propagation::Injector, trace::Status};
use tonic::transport::server::TcpConnectInfo;
use tracing::{field::Empty, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub fn extract_otel_info_from_req<B>(req: &Request<B>) -> Span {
    let (service, method) = extract_service_method(req.uri());

    let (local_addr, local_port, remote_addr, remote_port) = extract_client_server_conn_info(&req);

    // don't trace reflection services
    let span = match service.eq("grpc.reflection.v1.ServerReflection") {
        true => tracing::Span::none(),
        false => {
            let span = tracing::span!(tracing::Level::INFO, "OTEL GRPC",
                // network.peer.address = server_adress,
                // network.peer.port = server_port,
                network.transport = "tcp",
                "network.type" = "ipv4",
                rpc.system = "grpc",
                server.address = local_addr,
                server.port = local_port,
                client.address =  remote_addr,
                client.port = remote_port,
                rpc.method = method,
                rpc.service = service,
                otel.name = format!("{service}/{method}"),
                otel.kind = ?opentelemetry::trace::SpanKind::Server,
                otel.status_code = ?Status::default(),
                otel.status_message = Empty,
            );
            for (header_name, header_value) in req.headers().iter() {
                if let Ok(attribute_value) = header_value.to_str() {
                    let attribute_name = format!("http.request.header.{}", header_name);
                    span.set_attribute(attribute_name, attribute_value.to_owned());
                }
            }
            span
        }
    };

    span
}

fn extract_service_method(uri: &Uri) -> (&str, &str) {
    let path = uri.path();
    let mut parts = path.split('/').filter(|x| !x.is_empty());
    let service = parts.next().unwrap_or_default();
    let method = parts.next().unwrap_or_default();
    (service, method)
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

fn extract_client_server_conn_info<B>(
    req: &Request<B>,
) -> (Option<String>, Option<u16>, Option<String>, Option<u16>) {
    let local_addr = req
        .extensions()
        .get::<TcpConnectInfo>()
        .and_then(|info| info.local_addr)
        .map(|addr| addr.ip().to_string());

    let local_port = req
        .extensions()
        .get::<TcpConnectInfo>()
        .and_then(|info| info.local_addr)
        .map(|addr| addr.port());

    let remote_addr = req
        .extensions()
        .get::<TcpConnectInfo>()
        .and_then(|info| info.remote_addr)
        .map(|addr| addr.ip().to_string());

    let remote_port = req
        .extensions()
        .get::<TcpConnectInfo>()
        .and_then(|info| info.remote_addr)
        .map(|addr| addr.port());

    (local_addr, local_port, remote_addr, remote_port)
}
