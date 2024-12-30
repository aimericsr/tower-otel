use axum::body::HttpBody;
use axum::extract::MatchedPath;
use http::{Request, Response};
use opentelemetry::{
    metrics::{Counter, Histogram, Meter, UpDownCounter},
    KeyValue,
};
use pin_project::pin_project;
use std::{
    error::Error,
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Instant,
};
use tower::{BoxError, Layer, Service};

/// Tower layer to add OTEL metrics instrumentation to your axum app
/// It extract informations from the incoming HTTP request to create metrics
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#http-server)
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

#[derive(Debug, Clone)]
pub struct OtelMetrics<S> {
    metric: Metric,
    inner: S,
}

impl<S> OtelMetrics<S> {
    pub fn new(inner: S, meter: Meter) -> Self {
        let req_duration = meter
            .f64_histogram("http.server.request.duration")
            .with_unit("s")
            .with_description("The HTTP request latencies in seconds.")
            .with_boundaries(HTTP_REQ_DURATION_HISTOGRAM_BUCKETS.to_vec())
            .build();

        let req_active = meter
            .i64_up_down_counter("http.server.active_requests")
            .with_description("The number of active HTTP requests.")
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

        let requests_total = meter
            .u64_counter("requests")
            .with_description(
                "How many HTTP requests processed, partitioned by status code and HTTP method.",
            )
            .build();

        let metric = Metric {
            req_duration,
            req_active,
            req_size,
            res_size,
            requests_total,
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
    S: Service<Request<B>, Response = Response<B2>>,
    S::Error: Into<BoxError>,
    S::Future: Send + 'static,
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

#[pin_project]
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

impl<F, B, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<B>, E>>,
    E: Into<BoxError>,
    B: HttpBody,
{
    type Output = Result<Response<B>, E>;

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

        // TO DO : dynamicly extract required fields
        let labels = [
            //KeyValue::new("error.type", "http"),
            KeyValue::new("http.request.method", this.method.clone()),
            KeyValue::new("http.response.status_code", status),
            KeyValue::new("http.route", this.path.clone()),
            //KeyValue::new("url.schema", "http"),
            //KeyValue::new("network.protocol.version", "1.0"),
            KeyValue::new("server.address", this.host.clone()),
            //KeyValue::new("server.port", "unknown"),
        ];

        this.metric.req_duration.record(latency, &labels);

        this.metric.req_size.record(*this.req_size, &labels);

        this.metric.res_size.record(res_size, &labels);

        this.metric.requests_total.add(1, &labels);

        Poll::Ready(Ok(response))
    }
}

const HTTP_REQ_DURATION_HISTOGRAM_BUCKETS: &[f64] = &[
    0.0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

const KB: f64 = 1024.0;
const MB: f64 = 1024.0 * KB;

const HTTP_REQ_SIZE_HISTOGRAM_BUCKETS: &[f64] = &[
    1.0 * KB,   // 1 KB
    2.0 * KB,   // 2 KB
    5.0 * KB,   // 5 KB
    10.0 * KB,  // 10 KB
    100.0 * KB, // 100 KB
    500.0 * KB, // 500 KB
    1.0 * MB,   // 1 MB
    2.5 * MB,   // 2 MB
    5.0 * MB,   // 5 MB
    10.0 * MB,  // 10 MB
];

fn compute_approximate_request_size<T>(req: &Request<T>) -> usize {
    let mut s = 0;
    s += req.uri().path().len();
    s += req.method().as_str().len();

    req.headers().iter().for_each(|(k, v)| {
        s += k.as_str().len();
        s += v.as_bytes().len();
    });

    s += req.uri().host().map(|h| h.len()).unwrap_or(0);

    s += req
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .map(|v| v.to_str().unwrap().parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    s
}

//https://github.com/linkerd/linkerd2-proxy/blob/005dd472b9f7ae40aeb909f760a1b12cd8a93813/linkerd/trace-context/src/service.rs
