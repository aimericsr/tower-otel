use crate::helper::{inject_trace_id, update_span_with_response_headers};
use extractor::extract_otel_info_from_req;
use http::{Request, Response};
use opentelemetry_http::HeaderExtractor;
use pin_project_lite::pin_project;
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tower::BoxError;
use tower::{Layer, Service};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Add OTEL traces instrumentation to your axum app
/// It extract informations from the incoming HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/rpc/grpc/)
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
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/rpc/grpc/)
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

        ResponseFuture {
            inner: self.inner.call(request),
            span,
        }
    }
}

pin_project! {
    /// [`OtelLogger`] response future
    ///
    /// [`OtelLogger`]: crate::traces::tonic::OtelLogger
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
                Poll::Ready(Ok(response))
            }
            Err(err) => {
                unreachable!("The error variant sould never been reached as Axum require all of his services to be Infaillable : {err}");
            }
        }
    }
}

pub mod extractor {
    use http::{Request, Uri};
    use opentelemetry::trace::Status;
    use tonic::transport::server::TcpConnectInfo;
    use tracing::{field::Empty, Span};
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    pub fn extract_otel_info_from_req<B>(req: &Request<B>) -> Span {
        let (service, method) = extract_service_method(req.uri());

        let (local_addr, local_port, remote_addr, remote_port) =
            extract_client_server_conn_info(&req);

        // don't trace reflection services
        let span = match service.eq("grpc.reflection.v1.ServerReflection") {
            true => tracing::Span::none(),
            false => {
                let span = tracing::span!(tracing::Level::INFO, "OTEL GRPC",
                    // network.peer.address = server_adress,
                    // network.peer.port = server_port,
                    // network.transport = "tcp",
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
}
