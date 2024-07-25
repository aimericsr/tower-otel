use std::time::Duration;

use http::header::USER_AGENT;
use http::{Extensions, HeaderValue, Method};
use hyper::header;
use reqwest::Error;
use reqwest::{Client, StatusCode, Url};
use reqwest::{Request, Response};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Result as ResultReqwest};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use reqwest_tracing::{
    default_on_request_end, reqwest_otel_span, ReqwestOtelSpanBackend, TracingMiddleware,
};
use thiserror::Error;
use tower::limit::{ConcurrencyLimit, RateLimit};
use tower::util::{BoxCloneService, ServiceFn};
use tower::{Service, ServiceBuilder, ServiceExt};
use tower_otel::traces::http::service::{OtelLogger, OtelLoggerLayer};
use tower_reqwest::HttpClientLayer;
use tracing::{error, info};
use tracing::{Level, Span};
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Http error: {0}")]
    Main(#[from] reqwest_middleware::Error),

    #[error("Http error: {0}")]
    Main2(#[from] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct HttpManager {
    //client: ClientWithMiddleware,
    //client: ConcurrencyLimit<Client>,
    //client: ConcurrencyLimit<ServiceFn<T>>,
    client: Client,
    //client: T,
    //client:
    // BoxCloneService<http::Request<reqwest::Body>, http::Response<reqwest::Body>, anyhow::Error>,
    base_url: String,
}

//http.request.heades.all = tracing::field::Empty,

impl HttpManager {
    pub fn new(base_url: String) -> Self {
        //let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("Rust/1.0"),
        );
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .default_headers(headers)
            .build()
            .unwrap();

        // let svc = ServiceBuilder::new()
        //     .concurrency_limit(500)
        //     .layer(OtelLoggerLayer::default())
        //     //.rate_limit(100, Duration::new(10, 0))
        //     .service(client);

        //let client = make_client(client);

        // let client = ClientBuilder::new(client)
        //     .with(TracingMiddleware::<OtelTrace>::new())
        //     .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        //     .build();

        info!("HTTP manager initialized");
        Self { client, base_url }
    }

    pub async fn health_check(&self) -> Result<bool, HttpError> {
        let mut req = Request::new(
            reqwest::Method::POST,
            Url::parse(&format!("{}", &self.base_url)).unwrap(),
        );

        //let response = self.client.ready_and().await?.call(req).await?;

        //self.client.

        //self.client.call(req);

        // match response {
        //     Ok(response) => Ok(response.status() == StatusCode::OK),
        //     //Err(error) => Err(error.into()),
        //     Err(error) => Err(HttpError::Main2(error)),
        // }
        Ok(true)
    }
}

fn make_client(client: reqwest::Client) -> HttpClient {
    ServiceBuilder::new()
        // Make client compatible with the `tower-http` layers.
        .layer(HttpClientLayer)
        .service(client)
        .map_err(anyhow::Error::from)
        .boxed_clone()
}

type HttpClient = tower::util::BoxCloneService<
    http::Request<reqwest::Body>,
    http::Response<reqwest::Body>,
    anyhow::Error,
>;

struct RequestRetries {
    count: u64,
}

pub struct OtelTrace;

impl ReqwestOtelSpanBackend for OtelTrace {
    fn on_request_start(req: &Request, extension: &mut Extensions) -> Span {
        let otel_span_name = format!("{:#?} {:#?}", req.method(), req.url().path())
            .trim()
            .to_owned();
        reqwest_otel_span!(
            level = Level::INFO,
            name = otel_span_name,
            req,
            url.full = req.url().to_string(),
            http.request.resend_count = tracing::field::Empty,
        )
    }

    fn on_request_end(span: &Span, outcome: &ResultReqwest<Response>, extension: &mut Extensions) {
        // This would be someting like this
        //let retries = extension.get::<RequestRetries>().unwrap().count;
        //span.record("http.request.resend_count", retries);
        default_on_request_end(span, outcome);
    }
}
