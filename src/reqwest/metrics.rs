use crate::helper::{
    compute_approximate_request_size, HTTP_REQ_DURATION_HISTOGRAM_BUCKETS,
    HTTP_REQ_SIZE_HISTOGRAM_BUCKETS,
};
use axum::body::HttpBody;
use axum::extract::MatchedPath;
use http::{Request, Response};
use opentelemetry::{
    metrics::{Counter, Histogram, Meter, UpDownCounter},
    KeyValue,
};
use pin_project_lite::pin_project;
use std::{
    error::Error,
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Instant,
};
use tower::{Layer, Service};

/// Add OTEL metrics instrumentation to your axum app
/// It extract informations from the incoming HTTP request to create metrics
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#http-client)
#[derive(Debug, Clone)]
pub struct OtelMetricsLayer {
    meter: Meter,
}

impl OtelMetricsLayer {
    pub const fn new(meter: Meter) -> Self {
        OtelMetricsLayer { meter }
    }
}

impl<S> Layer<S> for OtelMetricsLayer {
    type Service = OtelMetrics<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OtelMetrics::new(inner, self.meter.clone())
    }
}

/// Add OTEL metrics instrumentation to your axum app
/// It extract informations from the incoming HTTP request to create metrics
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#http-client)
#[derive(Debug, Clone)]
pub struct OtelMetrics<S> {
    metric: Metric,
    inner: S,
}

impl<S> OtelMetrics<S> {
    pub fn new(inner: S, meter: Meter) -> Self {
        let requests_total = meter
            .u64_counter("requests")
            .with_description(
                "How many HTTP requests processed, partitioned by status code and HTTP method.",
            )
            .build();

        let req_duration = meter
            .f64_histogram("http.server.request.duration")
            .with_unit("s")
            .with_description("The HTTP request latencies in seconds.")
            .with_boundaries(HTTP_REQ_DURATION_HISTOGRAM_BUCKETS.to_vec())
            .build();

        let req_size = meter
            .u64_histogram("http.server.request.size")
            .with_unit("By")
            .with_description("The HTTP request sizes in bytes.")
            .with_boundaries(HTTP_REQ_SIZE_HISTOGRAM_BUCKETS.to_vec())
            .build();

        let res_size = meter
            .u64_histogram("http.server.response.size")
            .with_unit("By")
            .with_description("The HTTP reponse sizes in bytes.")
            .with_boundaries(HTTP_REQ_SIZE_HISTOGRAM_BUCKETS.to_vec())
            .build();

        let req_active = meter
            .i64_up_down_counter("http.server.active_requests")
            .with_description("The number of active HTTP requests.")
            .build();

        let metric = Metric {
            requests_total,
            req_duration,
            req_size,
            res_size,
            req_active,
        };

        OtelMetrics { inner, metric }
    }
}

/// The metrics we used in the middleware
#[derive(Debug, Clone)]
pub struct Metric {
    // Otel spec
    pub req_duration: Histogram<f64>,

    pub req_active: UpDownCounter<i64>,

    pub req_size: Histogram<u64>,

    pub res_size: Histogram<u64>,

    // Custom spec
    pub requests_total: Counter<u64>,
}

impl<S, B, B2> Service<Request<B>> for OtelMetrics<S>
where
    S: Service<Request<B>, Response = Response<B2>> + Clone + Send + 'static,
    S::Error: Error + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
    B2: HttpBody,
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
        self.metric.req_active.add(
            1,
            &[KeyValue::new(
                "http.request.method",
                req.method().as_str().to_string(),
            )],
        );

        let start = Instant::now();
        let method = req.method().clone().to_string();
        let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
            matched_path.as_str().to_owned()
        } else {
            "".to_owned()
        };

        let host = req
            .headers()
            .get(http::header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        let req_size = compute_approximate_request_size(&req);

        ResponseFuture {
            inner: self.inner.call(req),
            metric: self.metric.clone(),
            start,
            method,
            path,
            host,
            req_size: req_size as u64,
        }
    }
}

pin_project! {
    /// [`OtelMetrics`] response future
    ///
    /// [`OtelMetrics`]: crate::metrics::reqwest::OtelMetrics
    #[derive(Debug)]
    pub struct ResponseFuture<F> {
        #[pin]
        inner: F,
        metric: Metric,
        start: Instant,
        path: String,
        method: String,
        host: String,
        req_size: u64,
    }
}

impl<Fut, ResBody, E> Future for ResponseFuture<Fut>
where
    Fut: Future<Output = Result<Response<ResBody>, E>>,
    E: std::error::Error + 'static,
    ResBody: HttpBody,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        let response = ready!(this.inner.poll(cx))?;

        this.metric.req_active.add(
            -1,
            &[KeyValue::new("http.request.method", this.method.clone())],
        );

        let latency = this.start.elapsed().as_secs_f64();
        let status = response.status().as_u16().to_string();

        let res_size = response.body().size_hint().upper().unwrap_or(0);

        let labels = [
            KeyValue::new("http.request.method", this.method.clone()),
            KeyValue::new("http.route", this.path.clone()),
            KeyValue::new("http.response.status_code", status),
            KeyValue::new("server.address", this.host.clone()),
        ];

        this.metric.requests_total.add(1, &labels);

        this.metric.req_size.record(*this.req_size, &labels);

        this.metric.res_size.record(res_size, &labels);

        this.metric.req_duration.record(latency, &labels);

        Poll::Ready(Ok(response))
    }
}
