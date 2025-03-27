use crate::helper::{update_span_with_custom_attributes, update_span_with_response_headers};
use core::fmt;
use extract::extract_otel_info_from_req;
use http::{request::Parts, Request, Response};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use pin_project_lite::pin_project;
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tower::{BoxError, Layer, Service};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub type SpanAttributes = fn(&Parts) -> Vec<(&'static str, &'static str)>;
pub type Filter = fn(&Parts) -> bool;

/// Add OTEL traces instrumentation to your axum app.
/// It extract informations from the incoming HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-server)
///
/// Fields populated by default:
///
/// - network.protocol.name
/// - network.protocol.version
/// - network.transport
/// - server.address
/// - server.port
/// - client.address
/// - client.port
/// - http.request.method
/// - http.route
/// - url.schema
/// - url.path
/// - url.query
/// - user_agent.original
/// - http.response.status_code
/// - error.type
#[derive(Clone)]
pub struct OtelLoggerLayer {
    span_attributes: SpanAttributes,
    is_recorded: Filter,
}

impl OtelLoggerLayer {
    /// Choose to record or not the incoming HTTP request based on his [`http::request::Parts`].
    pub fn with_filter(self, is_recorded: Filter) -> Self {
        OtelLoggerLayer {
            is_recorded,
            ..self
        }
    }

    /// Choose to record additional fields in the root span
    /// for the incoming HTTP request based on his [`http::request::Parts`].
    ///
    /// <div class="warning"> The current Tracing API does not allow to record fields after the span creation.
    ///
    /// To avoid this problem, fields provided into this function are recorded using
    /// [`tracing_opentelemetry::OpenTelemetrySpanExt::set_attribute`]
    /// Then, this fields will only be available for the [`tracing_opentelemetry`] layer and not other layers from the tracing. </div>
    pub fn with_span_attributes(self, span_attributes: SpanAttributes) -> Self {
        OtelLoggerLayer {
            span_attributes,
            ..self
        }
    }
}

impl Default for OtelLoggerLayer {
    fn default() -> Self {
        Self {
            span_attributes: |_req: &Parts| Vec::new(),
            is_recorded: |_req: &Parts| true,
        }
    }
}

impl fmt::Debug for OtelLoggerLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OtelLoggerLayer")
            .field("span_attributes", &"<closure>")
            .field("is_recorded", &"<closure>")
            .finish()
    }
}

impl<S> Layer<S> for OtelLoggerLayer {
    type Service = OtelLogger<S>;
    fn layer(&self, inner: S) -> Self::Service {
        OtelLogger {
            inner,
            span_attributes: self.span_attributes.clone(),
            is_recorded: self.is_recorded.clone(),
        }
    }
}

/// Add OTEL traces instrumentation to your axum app.
/// It extract informations from the incoming HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-server)
#[derive(Clone)]
pub struct OtelLogger<S> {
    inner: S,
    span_attributes: SpanAttributes,
    is_recorded: Filter,
}

impl<S> fmt::Debug for OtelLogger<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OtelLoggerLayer")
            .field("span_attributes", &"<closure>")
            .field("is_recorded", &"<closure>")
            .finish()
    }
}

impl<S> OtelLogger<S> {
    pub fn new(inner: S, span_attributes: SpanAttributes, is_recorded: Filter) -> Self {
        OtelLogger {
            inner,
            span_attributes,
            is_recorded,
        }
    }
}

impl<S, B, B2> Service<Request<B>> for OtelLogger<S>
where
    S: Service<Request<B>, Response = Response<B2>>,
    S::Error: Into<BoxError>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let (parts, body) = request.into_parts();
        let cloned_parts = parts.clone();
        let request = http::Request::from_parts(parts, body);

        let span = match (self.is_recorded)(&cloned_parts) {
            true => {
                let span = extract_otel_info_from_req(&request);
                let extra_attributes = (self.span_attributes)(&cloned_parts);
                update_span_with_custom_attributes(extra_attributes, &span);
                span
            }
            false => tracing::Span::none(),
        };

        let context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });

        span.set_parent(context);

        // The span is only enter inside the poll function of ResponseFuture.
        // It should not be a problem as future need to be polled to make it advanced
        // And the time to constructing the future should be fairly fast
        ResponseFuture {
            inner: self.inner.call(request),
            span,
        }
    }
}

pin_project! {
    /// [`OtelLogger`] response future
    ///
    /// [`OtelLogger`]: crate::traces::axum::OtelLogger
    #[derive(Debug)]
    pub struct ResponseFuture<F> {
        #[pin]
        inner: F,
        span: Span,
    }
}

impl<F, B, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<B>, E>>,
    E: Into<BoxError>,
{
    type Output = Result<Response<B>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.span.enter();

        let response = ready!(this.inner.poll(cx));

        match response.map_err(Into::into) {
            Ok(mut response) => {
                // let response_size = compute_approximate_response_size(&response);
                // let response_body_size = compute_approximate_response_body_size(&response);
                // this.span.record("http.response.size", response_size);
                // this.span
                //     .record("http.response.body.size", response_body_size);
                update_span_with_response_headers(response.headers(), this.span);
                this.span
                    .record("http.response.status_code", response.status().as_str());
                // TO DO : How to get root error with more context ?
                if response.status().is_server_error() {
                    this.span.record("error.type", response.status().as_str());
                }

                let context = tracing::Span::current().context();
                let mut injector = HeaderInjector(response.headers_mut());
                opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.inject_context(&context, &mut injector);
                });

                Poll::Ready(Ok(response))
            }
            Err(err) => {
                unreachable!("The error variant sould never been reached as Axum require all of his services to be Infaillable : {err:?}");
            }
        }
    }
}

mod extract {
    use crate::helper::update_span_with_request_headers;
    use axum::extract::{ConnectInfo, MatchedPath, OriginalUri};
    use http::{header::USER_AGENT, Request};
    use opentelemetry::trace::Status;
    use std::net::{SocketAddr, ToSocketAddrs};
    use tracing::{field::Empty, Span};

    pub fn extract_otel_info_from_req<B>(req: &Request<B>) -> Span {
        let path = req.extensions().get::<OriginalUri>().unwrap().path();
        let query = req.extensions().get::<OriginalUri>().unwrap().query();
        let http_version = format!("{:?}", req.version()).replace("HTTP/", "");

        // If not matched path have been found, default to the path
        let route = req
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| path, |mp| mp.as_str());

        let method = req.method().as_str();
        let span_name = format!("{method} {route}");

        let (client_addr, client_port) = extract_client_conn_info(req);
        let (server_addr, server_port) = extract_server_conn_info(req);

        let user_agent = req.headers().get(USER_AGENT).map(|v| v.to_str().unwrap());

        // let http_request_size = compute_approximate_request_size(req);
        // let http_request_body_size = compute_approximate_request_body_size(req);

        let span = tracing::span!(tracing::Level::INFO, "OTEL HTTP",
            // network.peer.address = remote_addr,
            // network.peer.port = remote_port,
            // network.local.address = local_addr,
            // network.local.port = local_port,
            network.protocol.name = "http",
            network.protocol.version = ?http_version,
            network.transport = "tcp",
            server.address = server_addr,
            server.port = server_port,
            client.address = client_addr,
            client.port = client_port,
            http.request.method = method,
            // http.request.method_original = method,
            // http.request.header.random_key = Empty,
            // http.request.size = http_request_size, // Experimental
            // http.request.body.size = http_request_body_size, // Experimental
            http.route = route,
            url.scheme = "http",
            url.path = path,
            url.query = query,
            user_agent.original = user_agent,
            //user_agent.synthetic.type = user_agent, // Experimental
            // http.response.header.random_key = Empty,
            // http.response.size = Empty, // Experimental
            // http.response.body.size = Empty, // Experimental
            http.response.status_code = Empty,
            error.type = Empty,
            // Special Fields use by tracing-opentelemetry
            otel.name = span_name,
            otel.kind = ?opentelemetry::trace::SpanKind::Server,
            otel.status_code = ?Status::Unset,
            otel.status_message = Empty,
        );
        update_span_with_request_headers(req.headers(), &span);

        span
    }

    fn extract_client_conn_info<B>(req: &Request<B>) -> (Option<String>, Option<u16>) {
        let client_addr = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.ip().to_string());

        let client_port = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.port());

        (client_addr, client_port)
    }

    /// Follow OTLP [docs](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#setting-serveraddress-and-serverport-attributes)
    /// for trying to find the most intersting informations based on HTTP headers
    fn extract_server_conn_info<B>(req: &Request<B>) -> (Option<String>, Option<u16>) {
        let headers = req.headers();

        if let Some(forwarded) = headers.get("forwarded") {
            if let Some(result) = parse_forwarded_header(forwarded) {
                return result;
            }
        }

        if let Some(x_forwarded_host) = headers.get("x-forwarded-host") {
            if let Some(result) = parse_host_header(x_forwarded_host) {
                return result;
            }
        }

        if let Some(authority) = headers.get(":authority") {
            if let Some(result) = parse_host_header(authority) {
                return result;
            }
        }

        if let Some(host) = headers.get("host") {
            if let Some(result) = parse_host_header(host) {
                return result;
            }
        }

        (None, None)
    }

    fn parse_forwarded_header(value: &http::HeaderValue) -> Option<(Option<String>, Option<u16>)> {
        let value = value.to_str().ok()?;
        for part in value.split(';') {
            if part.trim_start().starts_with("host=") {
                let host = part.trim_start_matches("host=").trim_matches('"');
                return Some(parse_host_string(host));
            }
        }
        None
    }

    fn parse_host_header(value: &http::HeaderValue) -> Option<(Option<String>, Option<u16>)> {
        let value = value.to_str().ok()?;
        Some(parse_host_string(value))
    }

    fn parse_host_string(host: &str) -> (Option<String>, Option<u16>) {
        if let Ok(addr) = format!("{}:80", host).to_socket_addrs() {
            if let Some(socket_addr) = addr.into_iter().next() {
                return (Some(socket_addr.ip().to_string()), Some(socket_addr.port()));
            }
        }
        let mut parts = host.splitn(2, ':');
        let address = parts.next().map(String::from);
        let port = parts.next().and_then(|p| p.parse().ok());
        (address, port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::{routing::get, Router};
    use http::{HeaderValue, StatusCode};
    use opentelemetry::propagation::TextMapCompositePropagator;
    use opentelemetry::trace::{SpanId, SpanKind, Status, TracerProvider};
    use opentelemetry_sdk::propagation::BaggagePropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::InMemorySpanExporter};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tracing::subscriber::DefaultGuard;
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    fn init_test_tracer() -> (InMemorySpanExporter, DefaultGuard) {
        let exporter = InMemorySpanExporter::default();

        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("test-tracer");

        let baggage_propagator = BaggagePropagator::new();
        let trace_context_propagator = TraceContextPropagator::new();
        let composite_propagator = TextMapCompositePropagator::new(vec![
            Box::new(baggage_propagator),
            Box::new(trace_context_propagator),
        ]);
        opentelemetry::global::set_text_map_propagator(composite_propagator);

        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = Registry::default().with(telemetry);
        let _guard = tracing::subscriber::set_default(subscriber);

        (exporter, _guard)
    }

    fn get_trace_id_from_headers(headers: &http::HeaderMap) -> String {
        let trace_id = headers
            .get("traceparent")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        trace_id.split('-').take(2).collect::<Vec<&str>>().join("-")
    }

    #[tokio::test]
    async fn test_with_span_id() {
        let (exporter, _guard) = init_test_tracer();

        let logger = OtelLoggerLayer::default()
            .with_filter(|_req: &Parts| true)
            .with_span_attributes(|_req: &Parts| {
                let mut vec: Vec<(&'static str, &'static str)> = Vec::new();
                vec.push(("my_value", "here"));
                vec
            });

        let routes = Router::new()
            .route("/test", get(|| async { StatusCode::OK.into_response() }))
            .layer(logger);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = axum::serve(
            listener,
            routes.into_make_service_with_connect_info::<SocketAddr>(),
        );

        let _server_handle = tokio::spawn(async { server.await.unwrap() });

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "traceparent",
            reqwest::header::HeaderValue::from_static(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        );
        headers.insert(
            "baggage",
            reqwest::header::HeaderValue::from_static("user-id=12345"),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers.clone())
            .user_agent("test-bot")
            .build()
            .unwrap();

        let uri = format!("http://{}/test", addr);
        let response = client.get(uri).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Propagators
        let expected_trace_id = get_trace_id_from_headers(&headers);
        let actual_trace_id = get_trace_id_from_headers(response.headers());
        assert_eq!(actual_trace_id, expected_trace_id);
        assert_eq!(
            response.headers().get("tracestate"),
            Some(&HeaderValue::from_static(""))
        );
        assert_eq!(
            response.headers().get::<&str>("baggage"),
            Some(&HeaderValue::from_static("user-id=12345"))
        );

        // Traces
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1, "Only 1 span recorded");
        let span = &spans[0];
        assert_eq!(span.name, "GET /test");
        assert_eq!(
            span.parent_span_id,
            SpanId::from_hex("00f067aa0ba902b7").unwrap()
        );
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(span.status, Status::Unset);
        assert_eq!(span.attributes.len(), 26);
    }

    #[tokio::test]
    async fn test_without_span_id() {
        let (exporter, _guard) = init_test_tracer();

        let routes = Router::new()
            .route("/test", get(|| async { StatusCode::OK.into_response() }))
            .layer(OtelLoggerLayer::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = axum::serve(
            listener,
            routes.into_make_service_with_connect_info::<SocketAddr>(),
        );

        let _server_handle = tokio::spawn(async { server.await.unwrap() });

        let client = reqwest::Client::builder()
            .user_agent("test-bot")
            .build()
            .unwrap();

        let uri = format!("http://{}/test", addr);
        let response = client.get(uri).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Propagators
        assert!(response.headers().get("traceparent").is_some());
        assert_eq!(
            response.headers().get("tracestate"),
            Some(&HeaderValue::from_static(""))
        );
        assert_eq!(response.headers().get("baggage"), None);

        // Traces
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1, "Only 1 span recorded");
        let span = &spans[0];
        assert_eq!(span.name, "GET /test");
        assert_eq!(
            span.parent_span_id,
            SpanId::from_hex("0000000000000000").unwrap()
        );
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(span.status, Status::Unset);
        assert_eq!(span.attributes.len(), 23);
    }
}
