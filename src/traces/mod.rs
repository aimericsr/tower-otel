pub mod axum;
pub mod reqwest;
pub mod tonic;

use http::HeaderMap;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_http::HeaderInjector;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Extract the current OTEL trace id
///
/// This can be used to report the trace id to clients
/// to better trace further problems for specifics requests encounterd by your API consumers
pub fn get_current_otel_trace_id() -> Option<String> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_context = span.span_context();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

/// Inject OTEL information into the response
///
/// Can be use to propagat OTEL trace id between distributed services
pub(crate) fn inject_trace_id(headers: &mut http::HeaderMap) {
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
pub(crate) fn update_span_with_request_headers(headers: &HeaderMap) {
    let span = tracing::Span::current();

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
pub(crate) fn update_span_with_response_headers(headers: &HeaderMap) {
    let span = tracing::Span::current();

    for (header_name, header_value) in headers.iter() {
        if let Ok(attribute_value) = header_value.to_str() {
            let attribute_name = format!("http.response.header.{}", header_name);
            span.set_attribute(attribute_name, attribute_value.to_owned());
        }
    }
}
