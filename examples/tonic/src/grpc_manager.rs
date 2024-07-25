use crate::hello_world::greeter_client::GreeterClient;
use crate::hello_world::HelloRequest;
use thiserror::Error;
use tonic::transport::{channel, Channel};
use tower::ServiceBuilder;
use tower_otel::traces::grpc::service::{OtelLogger, OtelLoggerLayer};
use tracing::{error, info};

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Http error: {0}")]
    Main(#[from] reqwest_middleware::Error),

    #[error("Http error: {0}")]
    Main2(#[from] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct GrpcManager {
    client: OtelLogger<Channel>,
    base_url: String,
}

impl GrpcManager {
    pub async fn new(base_url: String) -> Self {
        //let mut client = GreeterClient::connect("http://[::1]:50051").await.unwrap();

        let channel = Channel::from_static("http://127.0.0.1:50051")
            .connect()
            .await
            .unwrap();
        let channel = ServiceBuilder::new()
            .layer(OtelLoggerLayer)
            .service(channel);

        //let mut client = GreeterClient::new(channel);

        info!("GRPC manager initialized");
        Self {
            client: channel,
            base_url,
        }
    }

    pub async fn say_hello(&self) -> Result<(), Box<dyn std::error::Error>> {
        //let mut client = GreeterClient::new(sel);

        // let request = tonic::Request::new(HelloRequest {
        //     name: "Tonic".into(),
        // });

        // let response = client.say_hello(request).await?;
        Ok(())
    }
}
