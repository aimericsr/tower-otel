use crate::{compute_approximate_response_body_size, compute_approximate_response_size};

use super::{inject_trace_id, update_span_with_response_headers};
use extract::extract_otel_info_from_req;
use http::{Request, Response};
use opentelemetry_http::HeaderExtractor;
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

/// Add OTEL traces instrumentation to your axum app
/// It extract informations from the incoming HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-server)
#[derive(Default, Debug, Clone)]
pub struct OtelLoggerLayer;

impl<S> Layer<S> for OtelLoggerLayer {
    type Service = OtelLogger<S>;
    fn layer(&self, inner: S) -> Self::Service {
        OtelLogger { inner }
    }
}

/// Add OTEL traces instrumentation to your axum app
/// It extract informations from the incoming HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-server)
#[derive(Debug, Clone)]
pub struct OtelLogger<S> {
    inner: S,
}

impl<S> OtelLogger<S> {
    pub fn new(inner: S) -> Self {
        OtelLogger { inner }
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
        let span = extract_otel_info_from_req(&request);

        let context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        span.set_parent(context);

        // The span is only enter inside the poll function of ResponseFuture.
        // It should not be a problem as future need to be polled to make work
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
                update_span_with_response_headers(response.headers());
                inject_trace_id(response.headers_mut());
                let response_size = compute_approximate_response_size(&response);
                let response_body_size = compute_approximate_response_body_size(&response);
                this.span.record("http.response.size", response_size);
                this.span
                    .record("http.response.body.size", response_body_size);
                this.span
                    .record("http.response.status_code", response.status().as_str());

                Poll::Ready(Ok(response))
            }
            Err(err) => {
                unreachable!("The error variant sould never been reached as Axum require all of his services to be Infaillable : {err:?}");
            }
        }
    }
}

mod extract {
    use axum::extract::{ConnectInfo, MatchedPath, OriginalUri};
    use http::{header::USER_AGENT, Request};
    use opentelemetry::trace::Status;
    use std::net::SocketAddr;
    use tracing::{field::Empty, Span};

    use crate::{
        compute_approximate_request_body_size, compute_approximate_request_size,
        traces::update_span_with_request_headers,
    };

    pub fn extract_otel_info_from_req<B>(req: &Request<B>) -> Span {
        let route = req
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| "", |mp| mp.as_str());
        let method = req.method().as_str();

        let path = req.extensions().get::<OriginalUri>().unwrap().path();
        let query = req.extensions().get::<OriginalUri>().unwrap().query();
        let http_version = req.version();

        let (local_addr, local_port, remote_addr, remote_port) =
            extract_client_server_conn_info(&req);
        let span_name = format!("{method} {route}");
        let user_agent = req.headers().get(USER_AGENT).map(|v| v.to_str().unwrap());

        let http_request_size = compute_approximate_request_size(req);
        let http_request_body_size = compute_approximate_request_body_size(req);

        let span = tracing::span!(tracing::Level::INFO, "OTEL HTTP",
            network.peer.address = remote_addr,
            network.peer.port = remote_port,
            network.local.address = local_addr,
            network.local.port = local_port,
            network.protocol.name = "http",
            network.protocol.version = ?http_version,
            network.transport = "tcp",
            network.type = "ipv4",
            server.address = local_addr,
            server.port = local_port,
            client.address = remote_addr,
            client.port = remote_port,
            http.request.method = method,
            http.request.method_original = method,
            // http.request.header.random_key = Empty,
            http.request.size = http_request_size,
            http.request.body.size = http_request_body_size,
            http.route = route,
            url.shchema = "http",
            url.path = path,
            url.query = query,
            user_agent.original = user_agent,
            user_agent.synthetic.type = user_agent,
            // http.response.header.random_key = Empty,
            http.response.size = Empty,
            http.response.body.size = Empty,
            http.response.status_code = Empty,
            error.type = Empty,
            otel.name = span_name,
            otel.kind = ?opentelemetry::trace::SpanKind::Server,
            otel.status_code = ?Status::Unset,
            otel.status_message = Empty,
        );
        update_span_with_request_headers(req.headers());

        span
    }

    fn extract_client_server_conn_info<B>(
        req: &Request<B>,
    ) -> (Option<String>, Option<u16>, Option<String>, Option<u16>) {
        // let local_addr = req
        //     .extensions()
        //     .get::<SocketAddr>()
        //     .and_then(|info| info.local_addr)
        //     .map(|addr| addr.ip().to_string());

        // let local_port = req
        //     .extensions()
        //     .get::<ConnectInfo>()
        //     .and_then(|info| info.local_addr)
        //     .map(|addr| addr.port());

        let local_addr = Some("unknown".to_string());

        let local_port = Some(0000);

        let remote_addr = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.ip().to_string());

        let remote_port = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.port());

        (local_addr, local_port, remote_addr, remote_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::{routing::get, Router};
    use http::StatusCode;
    use opentelemetry::trace::{SpanId, SpanKind, Status, TracerProvider as trace};
    use opentelemetry_sdk::trace::TracerProvider;
    use opentelemetry_sdk::{
        propagation::TraceContextPropagator, testing::trace::InMemorySpanExporter,
    };
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tracing::subscriber::DefaultGuard;
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    fn init_test_tracer() -> (InMemorySpanExporter, DefaultGuard) {
        let exporter = InMemorySpanExporter::default();

        let provider = TracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("test-tracer");

        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = Registry::default().with(telemetry);
        let _guard = tracing::subscriber::set_default(subscriber);
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

        (exporter, _guard)
    }

    #[tokio::test]
    async fn test_with_span_id() {
        let (exporter, _guard) = init_test_tracer();

        let routes = Router::new()
            .route("/test_ok", get(|| async { StatusCode::OK.into_response() }))
            .route(
                "/test_err",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR.into_response() }),
            )
            .layer(OtelLoggerLayer);
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
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION"),
            ))
            .build()
            .unwrap();

        // OK path
        let uri = format!("http://{}/test_ok", addr);
        let response = client.get(uri).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let spans = exporter.get_finished_spans().unwrap();
        assert!(spans.len() == 1, "Only 1 span recorded");
        let span = &spans[0];
        assert_eq!(span.name, "GET /test_ok");
        assert_eq!(
            span.parent_span_id,
            SpanId::from_hex("00f067aa0ba902b7").unwrap()
        );
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(span.status, Status::Unset);

        // ERROR path
        let uri = format!("http://{}/test_err", addr);
        let response = client.get(uri).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let spans = exporter.get_finished_spans().unwrap();
        assert!(spans.len() == 2, "Only 2 span recorded");
        let span = &spans[1];
        assert_eq!(span.name, "GET /test_err");
        assert_eq!(
            span.parent_span_id,
            SpanId::from_hex("00f067aa0ba902b7").unwrap()
        );
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(span.status, Status::Unset);

        // dbg!(&span.attributes.len());
        // for key_value in &span.attributes {
        //     let key = &key_value.key;
        //     let value = &key_value.value;
        //     if key.as_str() == "network.peer.port" {
        //         dbg!(value);
        //     }
        // }
    }

    #[tokio::test]
    async fn test_without_span_id() {
        let (exporter, _guard) = init_test_tracer();

        let routes = Router::new()
            .route("/test_ok", get(|| async { StatusCode::OK.into_response() }))
            .route(
                "/test_err",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR.into_response() }),
            )
            .layer(OtelLoggerLayer);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = axum::serve(
            listener,
            routes.into_make_service_with_connect_info::<SocketAddr>(),
        );

        let _server_handle = tokio::spawn(async { server.await.unwrap() });

        let client = reqwest::Client::builder()
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION"),
            ))
            .build()
            .unwrap();

        // OK path
        let uri = format!("http://{}/test_ok", addr);
        let response = client.get(uri).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let spans = exporter.get_finished_spans().unwrap();
        assert!(spans.len() == 1, "Only 1 span recorded");
        let span = &spans[0];
        assert_eq!(span.name, "GET /test_ok");
        assert_eq!(
            span.parent_span_id,
            SpanId::from_hex("0000000000000000").unwrap()
        );
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(span.status, Status::Unset);

        // ERROR path
        let uri = format!("http://{}/test_err", addr);
        let response = client.get(uri).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let spans = exporter.get_finished_spans().unwrap();
        assert!(spans.len() == 2, "Only 2 span recorded");
        let span = &spans[1];
        assert_eq!(span.name, "GET /test_err");
        assert_eq!(
            span.parent_span_id,
            SpanId::from_hex("0000000000000000").unwrap()
        );
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(span.status, Status::Unset);
    }
}
