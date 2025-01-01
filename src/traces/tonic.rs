use super::extractors::{extract_otel_info_from_req, update_span_on_error, update_span_on_success};
use http::{Request, Response};
use opentelemetry_http::HeaderExtractor;
use pin_project_lite::pin_project;
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
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

        let response = match this.inner.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(response) => response,
        };

        match response.map_err(Into::into) {
            Ok(mut response) => {
                if response.status().is_server_error() {
                    // let error = req
                    //     .extensions()
                    //     .get::<OtelError>()
                    //     .expect("The OtelError should have been added to the http extensions");

                    update_span_on_error(&mut response, this.span);
                    Poll::Ready(Ok(response))
                } else {
                    update_span_on_success(&mut response, this.span);
                    Poll::Ready(Ok(response))
                }
            }
            Err(err) => {
                unreachable!("The error variant sould never been reached as Axum require all of his services to be Infaillable : {err}");
            }
        }
    }
}
