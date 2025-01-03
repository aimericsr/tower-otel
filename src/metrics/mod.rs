pub mod axum;
pub mod reqwest;
pub mod tonic;

use http::Request;

pub(crate) const HTTP_REQ_DURATION_HISTOGRAM_BUCKETS: &[f64] = &[
    0.0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

const KB: f64 = 1024.0;
const MB: f64 = 1024.0 * KB;
pub(crate) const HTTP_REQ_SIZE_HISTOGRAM_BUCKETS: &[f64] = &[
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

pub(crate) fn compute_approximate_request_size<T>(req: &Request<T>) -> usize {
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
