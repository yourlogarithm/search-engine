use anyhow::Context;
use config::{Config, Environment};
use proto::{
    messaging_server::{Messaging, MessagingServer},
    PublishRequest, Url,
};
use queue::Queue;
use rand::seq::IteratorRandom;
use redis::{aio::MultiplexedConnection, AsyncCommands, RedisError};
use serde::Deserialize;
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc::error::SendError, Mutex};
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tonic::{transport::Server, Request, Response, Status};
use utils::redis::Key;

mod queue;

mod proto {
    tonic::include_proto!("messaging");
}

const COOLDOWN: i64 = 5;

struct MessagingService {
    redis_client: redis::Client,
    queues: Arc<Mutex<HashMap<String, Queue>>>,
    notifier: Arc<tokio::sync::Notify>,
}

#[tonic::async_trait]
impl Messaging for MessagingService {
    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<Url, Status>> + Send + 'static>>;

    async fn publish_urls(
        &self,
        request: Request<proto::PublishRequest>,
    ) -> Result<Response<()>, Status> {
        let PublishRequest { payloads } = request.into_inner();
        let mut map = self.queues.lock().await;
        tracing::debug!("Publishing {} URLs", payloads.len());
        for payload in payloads {
            tracing::info!(domain = %payload.queue, url = %payload.message, "Added");
            let entry = map.entry(payload.queue).or_default();
            entry.add(payload.message);
            self.notifier.notify_one();
        }
        Ok(Response::new(()))
    }

    async fn subscribe(&self, _: Request<()>) -> Result<Response<Self::SubscribeStream>, Status> {
        tracing::debug!("New subscription request received");
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let queues = self.queues.clone();
        let redis_client = self.redis_client.clone();
        let notifier = self.notifier.clone();
        tokio::spawn(async move {
            loop {
                notifier.notified().await;
                if let Err(e) = consume(&queues, &tx, &redis_client).await {
                    tracing::error!(error = %e.error(), "Failed to consume");
                    if let Some((domain, url)) = e.resend() {
                        tracing::warn!(domain = %domain, url = %url, "Resending URL due to consumption failure");
                        let mut map = queues.lock().await;
                        map.entry(domain).or_default().add(url);
                        notifier.notify_one();
                    }
                }
            }
        });

        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(output_stream) as Self::SubscribeStream
        ))
    }
}

enum ConsumeErrorKind {
    EmptyError(anyhow::Error),
    SendError {
        domain: String,
        error: SendError<Result<Url, Status>>,
    },
}

impl ConsumeErrorKind {
    fn resend(self) -> Option<(String, String)> {
        match self {
            ConsumeErrorKind::EmptyError(_) => None,
            ConsumeErrorKind::SendError { domain, error } => Some((domain, error.0.unwrap().url)),
        }
    }

    fn error(&self) -> String {
        match self {
            ConsumeErrorKind::EmptyError(e) => e.to_string(),
            ConsumeErrorKind::SendError { .. } => "Receiver dropped".to_string(),
        }
    }
}

async fn consume(
    queues: &Mutex<HashMap<String, Queue>>,
    tx: &tokio::sync::mpsc::Sender<Result<Url, Status>>,
    redis_client: &redis::Client,
) -> anyhow::Result<(), ConsumeErrorKind> {
    let mut map = queues.lock().await;
    let keys: Vec<_> = map
        .keys()
        .map(|name| Key::Cooldown(name.as_str()))
        .collect();
    tracing::debug!(key_count = keys.len(), "Checking cooldown for queues");
    let mut conn = redis_client
        .get_multiplexed_tokio_connection()
        .await
        .context("redis get_multiplexed_tokio_connection")
        .map_err(ConsumeErrorKind::EmptyError)?;
    let values: Vec<Option<u8>> = conn
        .mget(&keys)
        .await
        .context("redis mget")
        .map_err(ConsumeErrorKind::EmptyError)?;
    let entry = {
        let mut rng = rand::thread_rng();
        map.iter_mut()
            .zip(values)
            .filter(|(_, value)| value.is_none())
            .map(|(queue, _)| queue)
            .filter(|(_, queue)| !queue.is_empty())
            .choose(&mut rng)
    };
    if let Some((domain, queue)) = entry {
        let domain = domain.to_owned();
        let Some(url) = queue.pop() else {
            tracing::info!(domain = %domain, "Queue emptied, removing domain");
            map.remove(&domain);
            return Ok(());
        };
        if queue.is_empty() {
            tracing::info!(domain = %domain, "Queue emptied, removing domain");
            map.remove(&domain);
        }
        drop(map);
        if let Err(e) = cooldown(&domain, &mut conn).await {
            tracing::error!(error = %e, domain = %domain, "Failed to set cooldown");
        }
        tracing::info!(domain = %domain, url = %url, "Sending");
        tx.send(Ok(Url { url: url.clone() }))
            .await
            .map_err(|error| ConsumeErrorKind::SendError { domain, error })?;
        Ok(())
    } else {
        tracing::debug!("No eligible URLs to consume");
        Ok(())
    }
}

#[derive(Deserialize)]
struct AppConfig {
    pub redis_uri: String,
}

async fn serve() {
    tracing::debug!("Setting up health router");
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<MessagingServer<MessagingService>>()
        .await;

    tracing::debug!("Loading environment variables");
    let env = Environment::default().ignore_empty(true);

    tracing::debug!("Building AppConfig");
    let config = Config::builder()
        .add_source(env)
        .build()
        .expect("Failed to build configuration");

    let app_config: AppConfig = config
        .try_deserialize()
        .expect("Failed to deserialize configuration");

    let service = MessagingService {
        queues: Arc::new(Mutex::new(HashMap::new())),
        redis_client: redis::Client::open(app_config.redis_uri).unwrap(),
        notifier: Arc::new(tokio::sync::Notify::new()),
    };

    let addr = "0.0.0.0:50051".parse().unwrap();
    tracing::info!(address = %addr, "Server listening at {addr}");
    Server::builder()
        .add_service(health_service)
        .add_service(MessagingServer::new(service))
        .serve(addr)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "Server failed to start");
        });
}

async fn cooldown(domain: &str, conn: &mut MultiplexedConnection) -> Result<(), RedisError> {
    let key = Key::Cooldown(domain);
    tracing::debug!(domain = %domain, "Setting cooldown");
    redis::pipe()
        .atomic()
        .set(&key, 1)
        .expire(&key, COOLDOWN)
        .query_async(conn)
        .await
}

#[tokio::main]
async fn main() {
    utils::start(env!("CARGO_PKG_NAME"), Box::pin(serve())).await;
}
