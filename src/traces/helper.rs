use opentelemetry::trace::TraceContextExt;
use std::error::Error;
use tracing::{error, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Extract the current OTEL trace id. This can be used to report the trace id to clients
/// to better trace further problems for specifics requests encounterd by your API consumers
pub fn get_current_otel_trace_id() -> Option<String> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_context = span.span_context();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

// /// Record the provided Error as an event with the Opentelemetry conventions
// pub fn record_exeption_as_event(error: &dyn Error) {
//     error!(
//         exception.escaped = false,
//         "exception.type" = error.to_string(),
//         exception.message = error.to_string(),
//         exception.stacktrace = error.source(),
//     );
// }

// /// Record the provided Error inside the span with the Opentelemetry conventions
// pub fn record_exeption_for_span(span: &Span, error: &dyn Error) {
//     span.record("exception.escaped", false);
//     span.record("exception.type", error.to_string());
//     span.record("exception.message", error.to_string());
//     span.record("exception.stacktrace", error.source());
// }
