use dotenvy::dotenv;
use lapin::{Connection, ConnectionProperties};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
pub struct AppState {
    pub redis_client: redis::Client,
    pub reqwest_client: reqwest::Client,
    pub amqp_channel: lapin::Channel,
}

impl AppState {
    pub async fn new() -> Self {
        dotenv().ok();
        tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or("INFO".into()))
            .with(tracing_subscriber::fmt::layer().without_time())
            .init();

        let redis_client = redis::Client::open(env!("REDIS_URI")).unwrap();

        let reqwest_client = reqwest::Client::builder().build().unwrap();

        let options = ConnectionProperties::default()
            .with_executor(tokio_executor_trait::Tokio::current())
            .with_reactor(tokio_reactor_trait::Tokio);

        let connection = Connection::connect(env!("AMQP_URI"), options)
            .await
            .unwrap();
        let amqp_channel = connection.create_channel().await.unwrap();

        Self {
            redis_client,
            reqwest_client,
            amqp_channel,
        }
    }
}
