use super::extractors::{extract_otel_info_from_req, update_span_on_error, update_span_on_success};
use http::{Request, Response};
use opentelemetry::propagation::Extractor;
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

/// Layer to add OTEL instrumentation to your axum app
#[derive(Default, Debug, Clone)]
pub struct OtelLoggerLayer;

impl<S> Layer<S> for OtelLoggerLayer {
    type Service = OtelLogger<S>;
    fn layer(&self, inner: S) -> Self::Service {
        OtelLogger { inner }
    }
}

/// This service extract informations from the incoming HTTP request to create a span according to the
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
    S: Service<Request<B>, Response = Response<B2>, Error = BoxError> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let span = extract_otel_info_from_req(&req);

        let extractor = HttpHeaderExtractorHeaderExtractor(req.headers());
        let context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&extractor)
        });
        span.set_parent(context);

        // Do we need to do this ?
        // Or entering the span only in the poll function is OK ?
        // let future = {
        //     let _enter = span.enter();
        //     self.inner.call(req)
        // };
        ResponseFuture {
            //inner: future,
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

impl<Fut, ResBody> Future for ResponseFuture<Fut>
where
    Fut: Future<Output = Result<Response<ResBody>, BoxError>>,
{
    type Output = Result<Response<ResBody>, BoxError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.span.enter();

        let result = match this.inner.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };

        match result {
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

/// Set the parent trace if it is set in http headers, otherwise the span created is the root one
struct HttpHeaderExtractorHeaderExtractor<'a>(pub &'a http::HeaderMap);

impl<'a> Extractor for HttpHeaderExtractorHeaderExtractor<'a> {
    /// Get a value for a key from the `HeaderMap`. If the value is not valid ASCII, returns None.
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    /// Collect all the keys from the `HeaderMap`.
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(http::HeaderName::as_str).collect()
    }
}
