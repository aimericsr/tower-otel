use super::extractors::{extract_otel_info_from_req, update_span_on_error, update_span_on_success};
use http::{Request, Response};
use opentelemetry::propagation::Extractor;
use opentelemetry_http::HeaderExtractor;
use pin_project::pin_project;
use std::{
    error::Error,
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::BoxError;
use tower::{Layer, Service};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Tower layer to add OTEL traces instrumentation to your axum app
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

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let span = extract_otel_info_from_req(&req);

        let context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(req.headers()))
        });
        span.set_parent(context);

        ResponseFuture {
            inner: self.inner.call(req),
            span,
        }
    }
}

#[pin_project]
pub struct ResponseFuture<F> {
    #[pin]
    pub inner: F,
    pub span: Span,
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

        let result = match this.inner.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };

        match result.map_err(Into::into) {
            Ok(mut req) => {
                if req.status().is_server_error() {
                    // let error = req
                    //     .extensions()
                    //     .get::<OtelError>()
                    //     .expect("The OtelError should have been added to the http extensions");

                    update_span_on_error(&mut req, this.span);
                    Poll::Ready(Ok(req))
                } else {
                    update_span_on_success(&mut req, this.span);
                    Poll::Ready(Ok(req))
                }
            }
            Err(err) => {
                unreachable!("The error variant sould never been reached as Axum require all of his services to be Infaillable : {err}");
            }
        }
    }
}

/// This Error struct need to be inserted into the response extensions where you handle you app error
/// so that the OTEL span can access the error type and message
#[derive(Debug, Clone)]
pub struct OtelError {
    pub types: String,
    pub messages: String,
}
