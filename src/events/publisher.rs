use std::sync::Arc;

use lapin::Confirmation;
use lapin::options::{BasicPublishOptions, ConfirmSelectOptions, ExchangeDeclareOptions};
use lapin::types::FieldTable;
use lapin::{BasicProperties, Channel, Connection, ConnectionProperties, ExchangeKind};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::events::{EVENTS_EXCHANGE, amqp_url};

const PERSISTENT: u8 = 2;

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error(transparent)]
    Amqp(#[from] lapin::Error),
    #[error("the broker refused the message")]
    Nacked,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct Publisher {
    url: SecretString,
    exchange: String,
    link: Arc<Mutex<Option<(Connection, Channel)>>>,
}

impl std::fmt::Debug for Publisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Publisher").field("exchange", &self.exchange).finish_non_exhaustive()
    }
}

impl Publisher {
    pub fn new(url: SecretString) -> Self {
        Self { url, exchange: EVENTS_EXCHANGE.to_owned(), link: Arc::default() }
    }

    #[must_use]
    pub fn exchange(mut self, exchange: &str) -> Self {
        exchange.clone_into(&mut self.exchange);
        self
    }

    pub async fn connect(&self) -> Result<(), PublishError> {
        self.channel().await.map(|_| ())
    }

    async fn channel(&self) -> Result<Channel, PublishError> {
        let mut link = self.link.lock().await;
        if let Some((connection, channel)) = link.as_ref()
            && connection.status().connected()
            && channel.status().connected()
        {
            return Ok(channel.clone());
        }
        let connection =
            Connection::connect(&amqp_url(self.url.expose_secret()), ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        channel.confirm_select(ConfirmSelectOptions::default()).await?;
        channel
            .exchange_declare(
                self.exchange.as_str().into(),
                ExchangeKind::Topic,
                ExchangeDeclareOptions { durable: true, ..Default::default() },
                FieldTable::default(),
            )
            .await?;
        *link = Some((connection, channel.clone()));
        tracing::info!(exchange = %self.exchange, "publisher connected");
        Ok(channel)
    }

    async fn drop_link(&self) {
        let mut link = self.link.lock().await;
        *link = None;
    }

    pub async fn publish(&self, routing_key: &str, body: &Value) -> Result<(), PublishError> {
        let bytes = serde_json::to_vec(body)?;
        let channel = self.channel().await?;
        let properties =
            BasicProperties::default().with_content_type("application/json".into()).with_delivery_mode(PERSISTENT);
        let confirm = channel
            .basic_publish(
                self.exchange.as_str().into(),
                routing_key.into(),
                BasicPublishOptions::default(),
                &bytes,
                properties,
            )
            .await;
        let confirmation = match confirm {
            Ok(pending) => pending.await,
            Err(err) => Err(err),
        };
        match confirmation {
            Ok(Confirmation::Nack(_)) => Err(PublishError::Nacked),
            Ok(_) => Ok(()),
            Err(err) => {
                self.drop_link().await;
                Err(PublishError::Amqp(err))
            }
        }
    }

    pub async fn publish_many(
        &self,
        messages: &[(&str, Value)],
    ) -> Result<Vec<Result<(), PublishError>>, PublishError> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let channel = self.channel().await?;
        let properties =
            BasicProperties::default().with_content_type("application/json".into()).with_delivery_mode(PERSISTENT);
        let mut sent = Vec::with_capacity(messages.len());
        for (routing_key, body) in messages {
            let outcome = match serde_json::to_vec(body) {
                Ok(bytes) => channel
                    .basic_publish(
                        self.exchange.as_str().into(),
                        (*routing_key).into(),
                        BasicPublishOptions::default(),
                        &bytes,
                        properties.clone(),
                    )
                    .await
                    .map_err(PublishError::Amqp),
                Err(err) => Err(PublishError::Json(err)),
            };
            sent.push(outcome);
        }
        let mut broken = false;
        let mut outcomes = Vec::with_capacity(sent.len());
        for outcome in sent {
            let confirmed = match outcome {
                Ok(pending) => match pending.await {
                    Ok(Confirmation::Nack(_)) => Err(PublishError::Nacked),
                    Ok(_) => Ok(()),
                    Err(err) => Err(PublishError::Amqp(err)),
                },
                Err(err) => Err(err),
            };
            broken |= matches!(confirmed, Err(PublishError::Amqp(_)));
            outcomes.push(confirmed);
        }
        if broken {
            self.drop_link().await;
        }
        Ok(outcomes)
    }

    pub async fn close(&self) {
        let mut link = self.link.lock().await;
        if let Some((connection, _)) = link.take()
            && let Err(err) = connection.close(200, "shutdown".into()).await
        {
            tracing::debug!(error = %err, "publisher connection close failed");
        }
    }
}
