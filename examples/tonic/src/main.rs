use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_resource_detectors::{
    HostResourceDetector, OsResourceDetector, ProcessResourceDetector,
};
use opentelemetry_sdk::resource::{EnvResourceDetector, TelemetryResourceDetector};
use opentelemetry_sdk::trace::{self, RandomIdGenerator, Sampler};
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::trace::{SERVICE_NAME, SERVICE_NAMESPACE, SERVICE_VERSION};
use opentelemetry_semantic_conventions::SCHEMA_URL;
use std::sync::{Arc, RwLock};
use std::{error::Error, time::Duration};
use tonic_health::server::HealthReporter;
use tower_otel::traces::grpc::service::OtelLoggerLayer;
use tracing_log::LogTracer;
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, EnvFilter};

mod grpc_manager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Tracing
    let env_filter = EnvFilter::builder().try_from_env()?;

    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let detectors_ressources = Resource::from_detectors(
        Duration::from_millis(10),
        vec![
            Box::<EnvResourceDetector>::default(),
            Box::new(TelemetryResourceDetector),
            Box::<HostResourceDetector>::default(),
            Box::new(OsResourceDetector),
            Box::new(ProcessResourceDetector),
        ],
    );

    let default_ressources = Resource::new(vec![
        KeyValue::new("service.schema.url", SCHEMA_URL),
        KeyValue::new(SERVICE_NAME, "tonic-api"),
        KeyValue::new(SERVICE_NAMESPACE, "web-api"),
        KeyValue::new(SERVICE_VERSION, "v0.0.1"),
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

    let telemetry_subscriber = tracing_subscriber::Registry::default()
        .with(env_filter)
        .with(otel_trace_subscriber_layer);

    tracing::subscriber::set_global_default(telemetry_subscriber)?;
    LogTracer::init()?;

    // Grpc server
    let addr = "127.0.0.1:50051".parse()?;
    let greeter = GreeterService::default();

    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_not_serving::<GreeterServer<GreeterService>>()
        .await;

    tokio::spawn(twiddle_service_status(health_reporter.clone()));

    let svc = GreeterServer::with_interceptor(greeter, intercept);

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(hello_world::FILE_DESCRIPTOR_SET)
        .build()?;

    Server::builder()
        .accept_http1(true)
        .layer(tower_http::cors::CorsLayer::permissive())
        .timeout(Duration::from_secs(1))
        .layer(OtelLoggerLayer::default())
        .add_service(reflection)
        .add_service(health_service)
        .add_service(svc)
        .serve(addr)
        .await?;

    Ok(())
}

async fn twiddle_service_status(mut reporter: HealthReporter) {
    let mut iter = 0u64;
    loop {
        iter += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;

        if iter % 2 == 0 {
            reporter
                .set_serving::<GreeterServer<GreeterService>>()
                .await;
        } else {
            reporter
                .set_not_serving::<GreeterServer<GreeterService>>()
                .await;
        };
    }
}

fn intercept(mut req: Request<()>) -> Result<Request<()>, Status> {
    Ok(req)
}

pub mod hello_world {
    tonic::include_proto!("helloworld");

    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("helloworld_descriptor");
}

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use tonic::{transport::Server, Request, Response, Status};

#[derive(Debug, Default)]
pub struct GreeterService {
    state: Arc<RwLock<u64>>,
}

impl GreeterService {
    fn increment_counter(&self) {
        let mut count = self.state.write().unwrap();
        *count += 1;
    }

    fn get_counter(&self) -> u64 {
        *self.state.read().unwrap()
    }
}

#[tonic::async_trait]
impl Greeter for GreeterService {
    #[tracing::instrument]
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        tracing::info!("hello from server");
        self.increment_counter();
        println!("Count: {:?}", self.get_counter());

        let input = request.get_ref();

        if input.name == "" {
            return Err(tonic::Status::invalid_argument(
                "The name should not be empty",
            ));
        }

        let reply = HelloReply {
            message: format!("Hello {}!", input.name),
        };

        Ok(Response::new(reply))
    }
}
