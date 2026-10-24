use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicCancelOptions, BasicConsumeOptions, BasicQosOptions, BasicRejectOptions,
    ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::FieldTable;
use lapin::{Connection, ConnectionProperties, ExchangeKind};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::events::amqp_url;

pub const EVENTS_EXCHANGE: &str = "thecargo.events";
const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct ConsumerSpec {
    pub queue: String,
    pub routing_keys: Vec<String>,
    pub exchange: String,
    pub requeue_failed: bool,
    pub requeue_delay: Duration,
    pub prefetch: u16,
}

impl ConsumerSpec {
    pub fn new(queue: &str, routing_keys: &[&str]) -> Self {
        Self {
            queue: queue.to_owned(),
            routing_keys: routing_keys.iter().map(|key| (*key).to_owned()).collect(),
            exchange: EVENTS_EXCHANGE.to_owned(),
            requeue_failed: false,
            requeue_delay: Duration::ZERO,
            prefetch: 10,
        }
    }

    #[must_use]
    pub fn requeue_failed(mut self, delay: Duration) -> Self {
        self.requeue_failed = true;
        self.requeue_delay = delay;
        self
    }
}

pub trait Handler: Send + Sync + 'static {
    fn handle(&self, routing_key: &str, body: Value) -> impl Future<Output = anyhow::Result<()>> + Send;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ack,
    Requeue,
}

// The Python consumer's contract: success acks; a failure is sent back once when the queue asked
// for that and the message is not already a redelivery, and is otherwise acked and dropped.
pub fn outcome(result: &anyhow::Result<()>, requeue_failed: bool, redelivered: bool) -> Outcome {
    if result.is_err() && requeue_failed && !redelivered { Outcome::Requeue } else { Outcome::Ack }
}

pub async fn run<H: Handler>(url: SecretString, spec: ConsumerSpec, handler: Arc<H>, cancel: CancellationToken) {
    loop {
        match consume(&url, &spec, &handler, &cancel).await {
            Ok(()) if cancel.is_cancelled() => break,
            Ok(()) => tracing::warn!(queue = %spec.queue, "consumer stream ended; reconnecting"),
            Err(err) => tracing::warn!(queue = %spec.queue, error = %err, "consumer lost its connection; reconnecting"),
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(RECONNECT_DELAY) => {}
        }
    }
    tracing::info!(queue = %spec.queue, "consumer stopped");
}

async fn consume<H: Handler>(
    url: &SecretString,
    spec: &ConsumerSpec,
    handler: &Arc<H>,
    cancel: &CancellationToken,
) -> Result<(), lapin::Error> {
    let connection = Connection::connect(&amqp_url(url.expose_secret()), ConnectionProperties::default()).await?;
    let channel = connection.create_channel().await?;
    channel.basic_qos(spec.prefetch, BasicQosOptions::default()).await?;
    channel
        .exchange_declare(
            spec.exchange.as_str().into(),
            ExchangeKind::Topic,
            ExchangeDeclareOptions { durable: true, ..Default::default() },
            FieldTable::default(),
        )
        .await?;
    channel
        .queue_declare(
            spec.queue.as_str().into(),
            QueueDeclareOptions { durable: true, ..Default::default() },
            FieldTable::default(),
        )
        .await?;
    for key in &spec.routing_keys {
        channel
            .queue_bind(
                spec.queue.as_str().into(),
                spec.exchange.as_str().into(),
                key.as_str().into(),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await?;
    }
    let tag = format!("{}.consumer", spec.queue);
    let mut deliveries = channel
        .basic_consume(
            spec.queue.as_str().into(),
            tag.as_str().into(),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;
    tracing::info!(queue = %spec.queue, keys = ?spec.routing_keys, "consumer started");

    let mut in_flight = JoinSet::new();
    let result = loop {
        tokio::select! {
            () = cancel.cancelled() => break Ok(()),
            Some(done) = in_flight.join_next(), if !in_flight.is_empty() => report(done, &spec.queue),
            next = deliveries.next() => match next {
                Some(Ok(delivery)) => {
                    in_flight.spawn(process(Arc::clone(handler), delivery, spec.clone(), cancel.clone()));
                }
                Some(Err(err)) => break Err(err),
                None => break Ok(()),
            },
        }
    };

    if let Err(err) = channel.basic_cancel(tag.as_str().into(), BasicCancelOptions::default()).await {
        tracing::debug!(queue = %spec.queue, error = %err, "consumer cancel failed");
    }
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while let Some(done) = in_flight.join_next().await {
            report(done, &spec.queue);
        }
    })
    .await;
    if drained.is_err() {
        tracing::warn!(queue = %spec.queue, left = in_flight.len(), "in-flight messages did not finish before shutdown");
    }
    if let Err(err) = connection.close(200, "shutdown".into()).await {
        tracing::debug!(queue = %spec.queue, error = %err, "consumer connection close failed");
    }
    result
}

fn report(done: Result<(), tokio::task::JoinError>, queue: &str) {
    if let Err(err) = done
        && err.is_panic()
    {
        tracing::error!(queue, "event handler panicked; the message was left unacknowledged");
    }
}

async fn process<H: Handler>(handler: Arc<H>, delivery: Delivery, spec: ConsumerSpec, cancel: CancellationToken) {
    let routing_key = delivery.routing_key.as_str().to_owned();
    let result = match serde_json::from_slice::<Value>(&delivery.data) {
        Ok(body) => handler.handle(&routing_key, body).await,
        Err(err) => Err(anyhow::Error::new(err).context("the message body is not JSON")),
    };
    if let Err(err) = &result {
        tracing::error!(queue = %spec.queue, routing_key, error = format!("{err:#}"), "failed to process message");
    }

    let settled = match outcome(&result, spec.requeue_failed, delivery.redelivered) {
        Outcome::Ack => delivery.ack(BasicAckOptions::default()).await,
        Outcome::Requeue => {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = tokio::time::sleep(spec.requeue_delay) => {}
            }
            delivery.reject(BasicRejectOptions { requeue: true }).await
        }
    };
    if let Err(err) = settled {
        tracing::warn!(queue = %spec.queue, routing_key, error = %err, "could not settle a message; the broker will redeliver it");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_requeued_once_only_when_the_queue_asked() {
        let failed: anyhow::Result<()> = Err(anyhow::anyhow!("boom"));
        assert_eq!(outcome(&Ok(()), true, false), Outcome::Ack);
        assert_eq!(outcome(&failed, true, false), Outcome::Requeue);
        assert_eq!(outcome(&failed, true, true), Outcome::Ack);
        assert_eq!(outcome(&failed, false, false), Outcome::Ack);
    }
}
