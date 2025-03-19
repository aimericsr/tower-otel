use crate::helper::{
    inject_trace_id, update_span_with_custom_attributes, update_span_with_response_headers,
};
use core::fmt;
use extractor::extract_otel_info_from_req;
use http::{request::Parts, Request, Response};
use opentelemetry_http::HeaderExtractor;
use pin_project_lite::pin_project;
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tower::{BoxError, Layer, Service};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub type SpanAttributes = fn(&Parts) -> Vec<(&'static str, &'static str)>;
pub type Filter = fn(&Parts) -> bool;

/// Add OTEL traces instrumentation to your reqwest client
/// It extract informations from all outgoing HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-client)
#[derive(Clone)]
pub struct OtelLoggerLayer {
    span_attributes: SpanAttributes,
    is_recorded: Filter,
}
impl OtelLoggerLayer {
    /// Choose to record or not the incoming HTTP request based on his [`http::request::Parts`].
    pub fn with_filter(self, is_recorded: Filter) -> Self {
        OtelLoggerLayer {
            span_attributes: self.span_attributes,
            is_recorded,
        }
    }

    /// Choose to record additional fields in the root span
    /// for the incoming HTTP request based on his [`http::request::Parts`].
    ///
    /// <div class="warning"> The current Tracing API does not allow to record fields after the span creation.
    ///
    /// To avoid this problem, fields provided into this function are recorded using
    /// [`tracing_opentelemetry::OpenTelemetrySpanExt::set_attribute`]
    /// Then, this fields will only be available for the [`tracing_opentelemetry`] layer and not other layers from the tracing. </div>
    pub fn with_span_attributes(self, span_attributes: SpanAttributes) -> Self {
        OtelLoggerLayer {
            span_attributes,
            is_recorded: self.is_recorded,
        }
    }
}

impl Default for OtelLoggerLayer {
    fn default() -> Self {
        Self {
            span_attributes: |_req: &Parts| Vec::new(),
            is_recorded: |_req: &Parts| true,
        }
    }
}

impl fmt::Debug for OtelLoggerLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OtelLoggerLayer")
            .field("span_attributes", &"<closure>")
            .field("is_recorded", &"<closure>")
            .finish()
    }
}

impl<S> Layer<S> for OtelLoggerLayer {
    type Service = OtelLogger<S>;
    fn layer(&self, inner: S) -> Self::Service {
        OtelLogger {
            inner,
            span_attributes: self.span_attributes.clone(),
            is_recorded: self.is_recorded.clone(),
        }
    }
}

/// Add OTEL traces instrumentation to your axum app.
/// It extract informations from the incoming HTTP request to create a span according to the
/// [OTEL specification](https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-server)
#[derive(Clone)]
pub struct OtelLogger<S> {
    inner: S,
    span_attributes: SpanAttributes,
    is_recorded: Filter,
}

impl<S> fmt::Debug for OtelLogger<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OtelLoggerLayer")
            .field("span_attributes", &"<closure>")
            .field("is_recorded", &"<closure>")
            .finish()
    }
}

impl<S> OtelLogger<S> {
    pub fn new(inner: S, span_attributes: SpanAttributes, is_recorded: Filter) -> Self {
        OtelLogger {
            inner,
            span_attributes,
            is_recorded,
        }
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
        let (parts, body) = request.into_parts();
        let cloned_parts = parts.clone();
        let request = http::Request::from_parts(parts, body);

        let span = match (self.is_recorded)(&cloned_parts) {
            true => {
                let span = extract_otel_info_from_req(&request);
                let extra_attributes = (self.span_attributes)(&cloned_parts);
                update_span_with_custom_attributes(extra_attributes, &span);
                span
            }
            false => tracing::Span::none(),
        };

        let context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        span.set_parent(context);

        // The span is only enter inside the poll function of ResponseFuture.
        // It should not be a problem as future need to be polled to make work
        // And the time to constructing the future should be fairly fast
        ResponseFuture {
            inner: self.inner.call(request),
            span,
        }
    }
}

pin_project! {
    /// [`OtelLogger`] response future
    ///
    /// [`OtelLogger`]: crate::traces::reqwest::OtelLogger
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
                update_span_with_response_headers(response.headers(), this.span);
                inject_trace_id(response.headers_mut());
                // let response_size = compute_approximate_response_size(&response);
                // let response_body_size = compute_approximate_response_body_size(&response);
                // this.span.record("http.response.size", response_size);
                // this.span
                //     .record("http.response.body.size", response_body_size);
                this.span
                    .record("http.response.status_code", response.status().as_str());
                Poll::Ready(Ok(response))
            }
            Err(err) => {
                unreachable!("The error variant sould never been reached as Axum require all of his services to be Infaillable : {err:?}");
            }
        }
    }
}

pub mod extractor {
    use http::{header::USER_AGENT, Request};
    use hyper_util::client::legacy::connect::HttpInfo;
    use opentelemetry::trace::Status;
    use tracing::{field::Empty, Span};

    use crate::helper::update_span_with_request_headers;

    pub fn extract_otel_info_from_req<B>(req: &Request<B>) -> Span {
        let method = req.method().as_str();

        let (local_addr, local_port, remote_addr, remote_port) =
            extract_client_server_conn_info(req);
        let span_name = format!("{method} / {:?}", req.uri());

        let user_agent = req.headers().get(USER_AGENT).map(|v| v.to_str().unwrap());

        // let http_request_size = compute_approximate_request_size(req);
        // let http_request_body_size = compute_approximate_request_body_size(req);

        let url_full = req.uri().to_string();
        let http_version = req.version();

        let span = tracing::span!(tracing::Level::INFO, "OTEL HTTP",
            network.peer.address = remote_addr,
            network.peer.port = remote_port,
            network.protocol.name = "http",
            network.protocol.version = ?http_version,
            network.transport = "tcp",
            server.address = local_addr,
            server.port = local_port,
            http.request.method = method,
            // http.request.method_original = method,
            http.request.resend_count = 0,
            // http.request.header.random_key = Empty,
            // http.request.size = http_request_size, // Experimental
            // http.request.body.size = http_request_body_size, // Experimental
            url.full = url_full,
            url.shchema = "http",
            // url.template = Empty, // Experimental
            user_agent.original = user_agent,
            // user_agent.synthetic.type = user_agent, // Experimental
            // http.response.header.random_key = Empty,
            // http.response.size = Empty, // Experimental
            // http.response.body.size = Empty, // Experimental
            http.response.status_code = Empty,
            error.type = Empty,
            otel.name = span_name,
            otel.kind = ?opentelemetry::trace::SpanKind::Client,
            otel.status_code = ?Status::Unset,
            otel.status_message = Empty,
        );
        update_span_with_request_headers(req.headers(), &span);

        span
    }

    fn extract_client_server_conn_info<B>(
        req: &Request<B>,
    ) -> (Option<String>, Option<u16>, Option<String>, Option<u16>) {
        let local_addr = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.local_addr().ip().to_string());

        let local_port = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.local_addr().port());

        let remote_addr = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.remote_addr().ip().to_string());

        let remote_port = req
            .extensions()
            .get::<HttpInfo>()
            .map(|info| info.remote_addr().port());

        (local_addr, local_port, remote_addr, remote_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;
    use opentelemetry::trace::{SpanId, SpanKind, Status, TracerProvider};
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::InMemorySpanExporter};
    use tracing::subscriber::DefaultGuard;
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    fn init_test_tracer() -> (InMemorySpanExporter, DefaultGuard) {
        let exporter = InMemorySpanExporter::default();

        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("test-tracer");
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = Registry::default().with(telemetry);
        let _guard = tracing::subscriber::set_default(subscriber);

        (exporter, _guard)
    }

    #[tokio::test]
    async fn test_with_span_id() {
        let (exporter, _guard) = init_test_tracer();

        // TO DO : hide Arc impl details into the new methods ?
        // As Fn is a trait, we can't simply pass a Fn but hide it behind some kind of pointer (&, Box, Arc ...)
        // So is it possible to simplify the signature ? Can't use & as we have lifetime issue(because Service is 'static)
        // into the arc, Box is possible but is the same as using an Arc for the caller
        let logger = OtelLoggerLayer::default()
            .with_filter(|_req: &Parts| true)
            .with_span_attributes(|_req: &Parts| {
                let mut vec: Vec<(&'static str, &'static str)> = Vec::new();
                vec.push(("my_value", "here"));
                vec
            });

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "traceparent",
            reqwest::header::HeaderValue::from_static(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        );
        let client = reqwest::Client::builder()
            //.connector_layer(logger)
            .default_headers(headers)
            .user_agent("test-bot")
            .build()
            .unwrap();

        let uri = "https://testdns.fr";
        let response = client.get(uri).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let spans = exporter.get_finished_spans().unwrap();
        assert!(spans.len() == 1, "Only 1 span recorded");
        let span = &spans[0];
        assert_eq!(span.name, "GET https://testdns.fr");
        assert_eq!(
            span.parent_span_id,
            SpanId::from_hex("00f067aa0ba902b7").unwrap()
        );
        assert_eq!(span.span_kind, SpanKind::Client);
        assert_eq!(span.status, Status::Unset);
        assert_eq!(span.attributes.len(), 25);
    }
}
