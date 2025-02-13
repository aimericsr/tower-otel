use http::HeaderMap;
use opentelemetry_http::HeaderInjector;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Inject OTEL information into the response
///
/// Can be use to propagat OTEL trace id between distributed services
pub fn inject_trace_id(headers: &mut http::HeaderMap) {
    let context = tracing::Span::current().context();

    let mut injector = HeaderInjector(headers);
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut injector);
    });
}

/// Update span with response headers
///
/// But it use tracing_opentelemetry::span_ext::Span::set_attribute
/// so that we can set attribute that were not here at the span creation
/// it is bypassing tracing API and will not show in logs other that OTEL
pub fn update_span_with_request_headers(headers: &HeaderMap, span: &Span) {
    for (header_name, header_value) in headers.iter() {
        if let Ok(attribute_value) = header_value.to_str() {
            let attribute_name = format!("http.request.header.{}", header_name);
            span.set_attribute(attribute_name, attribute_value.to_owned());
        }
    }
}

/// Update span with response headers
///
/// But it use tracing_opentelemetry::span_ext::Span::set_attribute
/// so that we can set attribute that were not here at the span creation
/// it is bypassing tracing API and will not show in logs other that OTEL
pub fn update_span_with_response_headers(headers: &HeaderMap, span: &Span) {
    for (header_name, header_value) in headers.iter() {
        if let Ok(attribute_value) = header_value.to_str() {
            let attribute_name = format!("http.response.header.{}", header_name);
            span.set_attribute(attribute_name, attribute_value.to_owned());
        }
    }
}

/// Update default span with custom values
///
/// But it use tracing_opentelemetry::span_ext::Span::set_attribute
/// so that we can set attribute that were not here at the span creation
/// it is bypassing tracing API and will not show in logs other that OTEL
pub fn update_span_with_custom_attributes(
    attributes: Vec<(&'static str, &'static str)>,
    span: &Span,
) {
    for (key, value) in attributes {
        span.set_attribute(key, value);
    }
}

pub const HTTP_REQ_DURATION_HISTOGRAM_BUCKETS: &[f64] = &[
    0.0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

const KB: f64 = 1024.0;
const MB: f64 = 1024.0 * KB;
pub const HTTP_REQ_SIZE_HISTOGRAM_BUCKETS: &[f64] = &[
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

pub fn compute_approximate_request_size<T>(req: &http::Request<T>) -> usize {
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

pub fn compute_approximate_request_body_size<T>(req: &http::Request<T>) -> usize {
    let mut s = 0;

    s += req
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .map(|v| v.to_str().unwrap().parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    s
}

pub fn compute_approximate_response_size<T>(res: &http::Response<T>) -> usize {
    let mut s = 0;
    // s += res.uri().path().len();
    // s += res.method().as_str().len();

    res.headers().iter().for_each(|(k, v)| {
        s += k.as_str().len();
        s += v.as_bytes().len();
    });

    //s += res.uri().host().map(|h| h.len()).unwrap_or(0);

    s += res
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .map(|v| v.to_str().unwrap().parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    s
}
pub fn compute_approximate_response_body_size<T>(res: &http::Response<T>) -> usize {
    let mut s = 0;

    s += res
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .map(|v| v.to_str().unwrap().parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    s
}
