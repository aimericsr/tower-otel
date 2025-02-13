use crate::helper::{
    compute_approximate_response_body_size, compute_approximate_response_size, inject_trace_id,
    update_span_with_response_headers,
};
use extractor::extract_otel_info_from_req;
use http::{Request, Response};
use hyper_util::client::legacy::connect::HttpInfo;
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

/// Add OTEL traces instrumentation to your reqwest client
/// It extract informations from all outgoing HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-client)
#[derive(Default, Debug, Clone)]
pub struct OtelLoggerLayer;

impl<S> Layer<S> for OtelLoggerLayer {
    type Service = OtelLogger<S>;
    fn layer(&self, inner: S) -> Self::Service {
        OtelLogger { inner }
    }
}

/// Add OTEL traces instrumentation to your reqwest client
/// It extract informations from all outgoing HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-client)
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
        let http_info = request.extensions().get::<HttpInfo>();
        dbg!(http_info);

        let span = extract_otel_info_from_req(&request);

        let context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        span.set_parent(context);

        ResponseFuture {
            inner: self.inner.call(request),
            span,
        }
    }
}

pin_project! {
    /// [`OtelLogger`] response future
    ///
    /// [`OtelLogger`]: crate::traces::reqwest::OtelLogger
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
                update_span_with_response_headers(response.headers(), &this.span);
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

pub mod extractor {
    use axum::extract::MatchedPath;
    use http::{header::USER_AGENT, Request};
    use hyper_util::client::legacy::connect::HttpInfo;
    use opentelemetry::trace::Status;
    use tracing::{field::Empty, Span};

    use crate::helper::{
        compute_approximate_request_body_size, compute_approximate_request_size,
        update_span_with_request_headers,
    };

    pub fn extract_otel_info_from_req<B>(req: &Request<B>) -> Span {
        let route = req
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| "", |mp| mp.as_str());

        let method = req.method().as_str();

        let (local_addr, local_port, remote_addr, remote_port) =
            extract_client_server_conn_info(&req);
        let span_name = format!("{method}/{route}");

        let user_agent = req.headers().get(USER_AGENT).map(|v| v.to_str().unwrap());

        let http_request_size = compute_approximate_request_size(req);
        let http_request_body_size = compute_approximate_request_body_size(req);

        let url_full = req.uri().to_string();
        let http_version = req.version();

        let span = tracing::span!(tracing::Level::INFO, "OTEL HTTP",
            network.peer.address = remote_addr,
            network.peer.port = remote_port,
            network.protocol.version = ?http_version,
            network.transport = "tcp",
            server.address = local_addr,
            server.port = local_port,
            http.request.method = method,
            http.request.method_original = method,
            http.request.resend_count = 0,
            // http.request.header.random_key = Empty,
            http.request.size = http_request_size,
            http.request.body.size = http_request_body_size,
            url.full = url_full,
            url.shchema = "http",
            url.template = Empty,
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
        update_span_with_request_headers(req.headers(), &span);

        span
    }

    fn extract_client_server_conn_info<B>(
        req: &Request<B>,
    ) -> (Option<String>, Option<u16>, Option<String>, Option<u16>) {
        let local_addr = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.local_addr().ip().to_string());

        let local_port = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.local_addr().port());

        let remote_addr = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.remote_addr().ip().to_string());

        let remote_port = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.remote_addr().port());

        (local_addr, local_port, remote_addr, remote_port)
    }
}
