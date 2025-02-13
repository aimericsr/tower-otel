//! # OTEL Tracing for Rust Web Services
//!
//! This crate provides OpenTelemetry (OTEL) instrumentation for common Rust web services,
//! enabling the generation of metrics and traces using the Rust OpenTelemetry SDK.
//!
//! ## Overview
//! The following Rust crates are supported with OpenTelemetry instrumentation:
//!
//! | Crate     | Role             | Signals Supported  |
//! |----------|----------------|-------------------|
//! | [axum](https://docs.rs/axum/latest/axum/)   | Server          | Traces / Metrics  |
//! | [tonic](https://docs.rs/tonic/latest/tonic/)  | Server & Client | Traces / Metrics  |
//! | [tonic](https://docs.rs/reqwest/latest/reqwest/)| Client          | Traces / Metrics  |
//! | [tonic](https://docs.rs/tokio/latest/tokio/)  | Runtime         | Metrics          |
//!
//! ## Implementation Details
//! This project integrates with the `tracing` ecosystem, which is the primary
//! tracing library in Rust. Instead of directly interfacing with the OpenTelemetry SDK,
//! traces are exported using `tracing-opentelemetry`, ensuring seamless interoperability
//! with existing Rust projects using `tracing`.
//!
//! ## Inspiration
//! This project follows a similar approach to the OpenTelemetry Go instrumentation for common services:
//! [opentelemetry-go-contrib](https://github.com/open-telemetry/opentelemetry-go-contrib/tree/main/instrumentation).

pub mod axum;
mod helper;
pub mod reqwest;
pub mod tonic;

use opentelemetry::trace::TraceContextExt;
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
