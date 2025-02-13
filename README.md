# OTEL Tracing for Rust Web Services

This project provides OpenTelemetry (OTEL) instrumentation for common Rust web services, enabling the generation of metrics and traces using the Rust OpenTelemetry SDK.

## Overview
| Crate | Role | Signals Supported |
|-----------|-------|-------------------|
| `axum`   | Server | Traces / Metrics |
| `tonic`  | Server & Client | Traces / Metrics |
| `reqwest` | Client | Traces / Metrics |
| `tokio`  | Runtime | Metrics |

## Implementation Details
As `tracing` is the primary tracing library in Rust, this project uses `tracing-opentelemetry` instead of directly interfacing with the OpenTelemetry SDK for traces signals.

## Inspiration
This project follows a similar approach to the OpenTelemetry Go instrumentation for common services: [opentelemetry-go-contrib](https://github.com/open-telemetry/opentelemetry-go-contrib/tree/main/instrumentation).

