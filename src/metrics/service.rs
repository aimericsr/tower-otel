use http::{Request, Response};
use opentelemetry::metrics::{Counter, Histogram, UpDownCounter};
use pin_project::pin_project;
use std::{
    error::Error,
    fmt::Debug,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tower::{Layer, Service};

/// Layer to add metrics
#[derive(Default, Debug, Clone)]
pub struct OtelMetricsLayer;

impl<S> Layer<S> for OtelMetricsLayer {
    type Service = OtelMetrics<S>;
    fn layer(&self, inner: S) -> Self::Service {
        OtelMetrics { inner }
    }
}

#[derive(Debug, Clone)]
pub struct OtelMetrics<S> {
    inner: S,
}

impl<S> OtelMetrics<S> {
    pub fn new(inner: S) -> Self {
        OtelMetrics { inner }
    }
}

impl<S, B, B2> Service<Request<B>> for OtelMetrics<S>
where
    S: Service<Request<B>, Response = Response<B2>> + Clone + Send + 'static,
    S::Error: Error + 'static,
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
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        // let metric = Metric {
        //     requests_total
        // }
        ResponseFuture {
            //inner: future,
            inner: self.inner.call(req),
            metric: Metric {},
        }
    }
}

#[pin_project]
pub struct ResponseFuture<F> {
    #[pin]
    pub inner: F,
    pub metric: Metric,
}

impl<Fut, ResBody, E> Future for ResponseFuture<Fut>
where
    Fut: Future<Output = Result<Response<ResBody>, E>>,
    E: std::error::Error + 'static,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        let result = match this.inner.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };

        //dbg!("metrics service polled");

        Poll::Ready(result)
    }
}

/// the metrics we used in the middleware
#[derive(Clone)]
pub struct Metric {
    //pub requests_total: Counter<u64>,
    // pub req_duration: Histogram<f64>,

    // pub req_size: Histogram<u64>,

    // pub res_size: Histogram<u64>,

    // pub req_active: UpDownCounter<i64>,
}

const HTTP_REQ_DURATION_HISTOGRAM_BUCKETS: &[f64] = &[
    0.0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];
