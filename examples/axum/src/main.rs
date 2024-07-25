use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::{http::StatusCode, routing::post, Router};
use core::fmt;
use core::fmt::{Display, Formatter};
use http_manager::HttpManager;
use opentelemetry::KeyValue;
use opentelemetry_otlp::ExportConfig;
use opentelemetry_otlp::Protocol;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_resource_detectors::{
    HostResourceDetector, OsResourceDetector, ProcessResourceDetector,
};
use opentelemetry_sdk::metrics::reader::DefaultAggregationSelector;
use opentelemetry_sdk::metrics::reader::DefaultTemporalitySelector;
use opentelemetry_sdk::resource::{
    EnvResourceDetector, SdkProvidedResourceDetector, TelemetryResourceDetector,
};
use opentelemetry_sdk::trace::{self, RandomIdGenerator, Sampler, SpanLimits, TracerProvider};
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::trace::{SERVICE_NAME, SERVICE_NAMESPACE, SERVICE_VERSION};
use opentelemetry_semantic_conventions::SCHEMA_URL;
use serde::Deserialize;
use std::net::SocketAddr;
use std::{error::Error, time::Duration};
use tokio::net::TcpListener;
use tower_otel::metrics::service::OtelMetricsLayer;
use tower_otel::traces::helper::get_current_otel_trace_id;
use tower_otel::traces::http::service::{OtelError, OtelLoggerLayer};
use tracing::{error, info, instrument, span};
use tracing_log::LogTracer;
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, EnvFilter};

mod http_manager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let env_filter = EnvFilter::builder().try_from_env()?;

    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let detectors_ressources = Resource::from_detectors(
        Duration::from_millis(10),
        vec![
            Box::<EnvResourceDetector>::default(),
            Box::new(TelemetryResourceDetector),
            Box::new(OsResourceDetector),
            Box::new(ProcessResourceDetector),
            Box::<HostResourceDetector>::default(),
        ],
    );

    let default_ressources = Resource::new(vec![
        KeyValue::new("service.schema.url", SCHEMA_URL),
        KeyValue::new(SERVICE_NAME, "axum-api"),
        KeyValue::new(SERVICE_VERSION, "v0.0.1"),
        KeyValue::new(SERVICE_NAMESPACE, "products"),
    ]);

    let ressources = detectors_ressources.merge(&default_ressources);

    let otel_trace_subscriber_layer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint("http://localhost:4317")
                .with_timeout(Duration::from_secs(3)),
        )
        .with_trace_config(
            trace::config()
                .with_sampler(Sampler::AlwaysOn)
                .with_id_generator(RandomIdGenerator::default())
                .with_max_events_per_span(32)
                .with_max_attributes_per_span(32)
                .with_max_links_per_span(32)
                .with_max_attributes_per_event(32)
                .with_max_attributes_per_link(32)
                .with_resource(ressources),
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;

    let otel_trace_subscriber_layer =
        tracing_opentelemetry::layer().with_tracer(otel_trace_subscriber_layer);

    // let json_layer = tracing_subscriber::fmt::layer()
    //     .json()
    //     .with_span_events(FmtSpan::NONE);

    // let stdout_provider = TracerProvider::builder()
    //     .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
    //     .build();
    // let stdout_tracer = stdout_provider.tracer("readme_example");
    // let otel_stdout = tracing_opentelemetry::layer().with_tracer(stdout_tracer);

    let telemetry_subscriber = tracing_subscriber::Registry::default()
        .with(env_filter)
        .with(otel_trace_subscriber_layer);
    //.with(json_layer)
    //.with(otel_stdout)
    tracing::subscriber::set_global_default(telemetry_subscriber)?;
    LogTracer::init()?;

    // let meter = opentelemetry_otlp::new_pipeline()
    //     .metrics(opentelemetry_sdk::runtime::Tokio)
    //     .with_exporter(
    //         opentelemetry_otlp::new_exporter()
    //             .tonic()
    //             .with_export_config(ExportConfig {
    //                 endpoint: "http://localhost:4317".to_string(),
    //                 timeout: Duration::from_secs(3),
    //                 protocol: Protocol::Grpc,
    //             }),
    //         // can also config it using with_* functions like the tracing part above.
    //     )
    //     .with_resource(Resource::new(vec![KeyValue::new(
    //         "service.name",
    //         "example",
    //     )]))
    //     .with_period(Duration::from_secs(3))
    //     .with_timeout(Duration::from_secs(10))
    //     .with_aggregation_selector(DefaultAggregationSelector::new())
    //     .with_temporality_selector(DefaultTemporalitySelector::new())
    //     .build();

    let addr: SocketAddr = ([127, 0, 0, 1], 3000).into();
    let listener = TcpListener::bind(addr).await?;

    let http_manager = HttpManager::new("https://google.com".into());
    let app_state = AppState { http_manager };

    let app = Router::new()
        .route("/api/v1/hello/:name", post(root))
        .layer(OtelLoggerLayer::default())
        .layer(OtelMetricsLayer::default())
        .with_state(app_state);

    info!("Axum listening on {:?}", listener.local_addr()?);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    http_manager: HttpManager,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    page: usize,
    per_page: usize,
}

async fn root(
    Path(_name): Path<String>,
    Query(_pagination): Query<Pagination>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    log::info!("Log recod");
    tracing::info!("Tracing recod");
    state.http_manager.health_check().await.unwrap();
    redis_call().await;
    error!("aaaaa");
    format!("Hello, {:?}!", get_current_otel_trace_id())
    //CustomError::Main
}

#[instrument(fields(error = tracing::field::Empty))]
async fn redis_call() {
    let current_span = tracing::Span::current();
    let error = CustomError::Main;
    //error!(error = &error as &dyn Error);
    current_span.record("error", &error as &dyn Error);
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[derive(Debug)]
enum CustomError {
    Main,
}

impl Display for CustomError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Main Display from Axum")
    }
}
impl Error for CustomError {}

impl IntoResponse for CustomError {
    fn into_response(self) -> Response {
        let current_span = tracing::Span::current();
        //current_span.record(field, value)

        error!("aaaaa");

        error!(error = &self as &dyn Error);

        dbg!(current_span);
        let mut response =
            (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong").into_response();
        let error: OtelError = OtelError {
            types: self.to_string(),
            messages: "je suis un message d'erreur".into(),
        };
        //response.extensions_mut().insert(error);
        response
    }
}
