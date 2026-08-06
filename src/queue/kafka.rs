use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rdkafka::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer, ConsumerContext, Rebalance, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use tokio_stream::{Stream, StreamExt};

use crate::error::AppError;

pub use crate::config::kafka::{
    KafkaConnectionConfig, KafkaConnectionConfigBuilder, KafkaConsumerConfig,
    KafkaConsumerConfigBuilder, KafkaProducerConfig, KafkaProducerConfigBuilder,
};
pub use rdkafka::consumer::CommitMode;

/// Consumer offset-reset policy, applied when a consumer group has no
/// previously committed offset for a partition (e.g. a brand-new
/// `group_id`, or offsets that expired past the broker's retention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoOffsetReset {
    /// Start from the beginning of the topic.
    #[default]
    Earliest,
    /// Start from the current end of the topic (only new messages).
    Latest,
}

impl AutoOffsetReset {
    fn as_librdkafka_str(self) -> &'static str {
        match self {
            Self::Earliest => "earliest",
            Self::Latest => "latest",
        }
    }
}

impl FromStr for AutoOffsetReset {
    type Err = AppError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "earliest" => Ok(Self::Earliest),
            "latest" => Ok(Self::Latest),
            other => Err(AppError::internal_error(
                format!("Unknown auto_offset_reset value: {other}"),
                None,
            )),
        }
    }
}

fn dependency_error(op: &'static str, err: impl std::fmt::Display) -> AppError {
    AppError::DependencyFailed {
        upstream: "kafka".into(),
        detail: format!("{op}: {err}"),
        retryable: true,
    }
}

fn config_error(field: &str) -> AppError {
    AppError::DependencyFailed {
        upstream: "kafka".into(),
        detail: format!("Kafka config field `{field}` is required and must not be empty"),
        retryable: false,
    }
}

fn lock_error() -> AppError {
    AppError::internal_error(
        "Lock poisoned".to_string(),
        Some("A thread panicked while holding the Kafka rebalance lock".to_string()),
    )
}

fn validate_key(key: &str) -> Result<(), AppError> {
    if key.is_empty() {
        return Err(AppError::bad_request(
            None,
            Some("Kafka message key must not be empty".to_string()),
            None,
        ));
    }
    Ok(())
}

fn is_empty_json_value(value: &serde_json::Value) -> bool {
    matches!(value, serde_json::Value::Null)
        || matches!(value, serde_json::Value::String(s) if s.is_empty())
}

fn validate_value(value: &serde_json::Value) -> Result<(), AppError> {
    if is_empty_json_value(value) {
        return Err(AppError::bad_request(
            None,
            Some("Kafka message value must not be null or an empty string".to_string()),
            None,
        ));
    }
    Ok(())
}

fn validate_payload(payload: &[u8]) -> Result<(), AppError> {
    if payload.is_empty() {
        return Err(AppError::bad_request(
            None,
            Some("Kafka message payload must not be empty".to_string()),
            None,
        ));
    }
    Ok(())
}

fn require_brokers(brokers: Option<Vec<String>>) -> Result<Vec<String>, AppError> {
    let brokers = brokers
        .filter(|b| !b.is_empty())
        .ok_or_else(|| config_error("brokers"))?;
    if brokers.iter().any(|b| b.is_empty()) {
        return Err(config_error("brokers"));
    }
    Ok(brokers)
}

fn require_producer_topic(topic: Option<String>) -> Result<String, AppError> {
    topic
        .filter(|t| !t.is_empty())
        .ok_or_else(|| config_error("topic"))
}

fn require_consumer_topics(topics: Option<Vec<String>>) -> Result<Vec<String>, AppError> {
    let topics = topics
        .filter(|t| !t.is_empty())
        .ok_or_else(|| config_error("topics"))?;
    if topics.iter().any(|t| t.is_empty()) {
        return Err(config_error("topics"));
    }
    Ok(topics)
}

fn require_group_id(group_id: Option<String>) -> Result<String, AppError> {
    group_id
        .filter(|g| !g.is_empty())
        .ok_or_else(|| config_error("group_id"))
}

fn apply_extra_options(
    client_config: &mut ClientConfig,
    extra_options: &Option<std::collections::HashMap<String, String>>,
) {
    for (key, value) in extra_options.iter().flatten() {
        client_config.set(key, value);
    }
}

pub async fn test_kafka(
    config: impl Into<Option<KafkaConnectionConfig>>,
) -> Result<bool, AppError> {
    let config = config.into().unwrap_or_default();
    let brokers = require_brokers(config.brokers)?;
    let socket_timeout_ms = config.socket_timeout_ms.unwrap_or(5_000);

    let mut client_config = ClientConfig::new();
    client_config
        .set("bootstrap.servers", brokers.join(","))
        .set("socket.timeout.ms", socket_timeout_ms.to_string());

    let consumer: BaseConsumer = client_config
        .create()
        .map_err(|e| dependency_error("test_kafka: create client", e))?;

    tokio::task::spawn_blocking(move || {
        consumer
            .fetch_metadata(None, Duration::from_secs(5))
            .map(|_| true)
    })
    .await
    .map_err(|e| AppError::internal_error(format!("test_kafka: task join error: {e}"), None))?
    .map_err(|e| dependency_error("test_kafka: fetch_metadata", e))
}

pub struct KafkaProducer {
    producer: FutureProducer,
    topic: String,
}

pub async fn get_kafka_producer(
    config: impl Into<Option<KafkaProducerConfig>>,
) -> Result<KafkaProducer, AppError> {
    let config = config.into().unwrap_or_default();

    let brokers = require_brokers(config.connection.brokers)?;
    let topic = require_producer_topic(config.topic)?;
    let message_timeout_ms = config.message_timeout_ms.unwrap_or(5_000);
    let socket_timeout_ms = config.connection.socket_timeout_ms.unwrap_or(5_000);

    let mut client_config = ClientConfig::new();
    client_config
        .set("bootstrap.servers", brokers.join(","))
        .set("socket.timeout.ms", socket_timeout_ms.to_string())
        .set("message.timeout.ms", message_timeout_ms.to_string());
    apply_extra_options(&mut client_config, &config.connection.extra_options);

    let producer: FutureProducer = client_config
        .create()
        .map_err(|e| dependency_error("get_kafka_producer: create", e))?;

    tracing::info!(topic = %topic, "Kafka producer initialized");

    Ok(KafkaProducer { producer, topic })
}

impl KafkaProducer {
    pub async fn send_json(&self, key: &str, value: &serde_json::Value) -> Result<(), AppError> {
        self.send_json_to_topic(&self.topic.clone(), key, value)
            .await
    }

    pub async fn send_json_to_topic(
        &self,
        topic: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), AppError> {
        validate_key(key)?;
        validate_value(value)?;

        let payload = serde_json::to_vec(value).map_err(|e| {
            AppError::internal_error(format!("Failed to serialize JSON payload: {e}"), None)
        })?;

        let record = FutureRecord::to(topic).key(key).payload(&payload);

        self.producer
            .send(record, Duration::from_secs(0))
            .await
            .map_err(|(e, _)| dependency_error("send_json_to_topic", e))?;
        Ok(())
    }

    pub async fn send_bytes(&self, key: &str, payload: &[u8]) -> Result<(), AppError> {
        validate_key(key)?;
        validate_payload(payload)?;

        let record = FutureRecord::to(&self.topic).key(key).payload(payload);

        self.producer
            .send(record, Duration::from_secs(0))
            .await
            .map_err(|(e, _)| dependency_error("send_bytes", e))?;
        Ok(())
    }
}

struct KafkaConsumerContext {
    partition_reassignment_in_progress: Arc<RwLock<bool>>,
}

impl ClientContext for KafkaConsumerContext {}

impl ConsumerContext for KafkaConsumerContext {
    fn pre_rebalance(&self, _base_consumer: &BaseConsumer<Self>, rebalance: &Rebalance<'_>) {
        match rebalance {
            Rebalance::Revoke(_) => {
                tracing::info!("Kafka: partitions revoked");
                self.set_reassignment_flag(true);
            }
            Rebalance::Assign(_) => {
                tracing::info!("Kafka: partitions assigned");
                self.set_reassignment_flag(false);
            }
            Rebalance::Error(err) => {
                tracing::error!(error = %err, "Kafka: rebalance error");
            }
        }
    }

    fn post_rebalance(&self, _base_consumer: &BaseConsumer<Self>, rebalance: &Rebalance<'_>) {
        if let Rebalance::Assign(_) = rebalance {
            tracing::info!("Kafka: rebalance completed");
        }
    }
}

impl KafkaConsumerContext {
    fn set_reassignment_flag(&self, value: bool) {
        let mut guard = self
            .partition_reassignment_in_progress
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = value;
    }
}

#[derive(Debug, Clone)]
pub struct KafkaMessage {
    pub key: String,
    pub value: serde_json::Value,
    raw: rdkafka::message::OwnedMessage,
}

pub struct KafkaConsumer {
    consumer: StreamConsumer<KafkaConsumerContext>,
    partition_reassignment_in_progress: Arc<RwLock<bool>>,
}

pub async fn get_kafka_consumer(
    config: impl Into<Option<KafkaConsumerConfig>>,
) -> Result<KafkaConsumer, AppError> {
    let config = config.into().unwrap_or_default();

    let brokers = require_brokers(config.connection.brokers)?;
    let topics = require_consumer_topics(config.topics)?;
    let group_id = require_group_id(config.group_id)?;
    let session_timeout_ms = config.session_timeout_ms.unwrap_or(30_000);
    let auto_offset_reset = config.auto_offset_reset.unwrap_or_default();
    let auto_commit = config.auto_commit.unwrap_or(false);
    let auto_commit_interval_ms = config.auto_commit_interval_ms.unwrap_or(5_000);
    let socket_timeout_ms = config.connection.socket_timeout_ms.unwrap_or(5_000);

    let partition_reassignment_in_progress = Arc::new(RwLock::new(false));
    let context = KafkaConsumerContext {
        partition_reassignment_in_progress: partition_reassignment_in_progress.clone(),
    };

    let mut client_config = ClientConfig::new();
    client_config
        .set("group.id", &group_id)
        .set("bootstrap.servers", brokers.join(","))
        .set("socket.timeout.ms", socket_timeout_ms.to_string())
        .set("enable.auto.commit", auto_commit.to_string())
        .set(
            "auto.commit.interval.ms",
            auto_commit_interval_ms.to_string(),
        )
        .set("auto.offset.reset", auto_offset_reset.as_librdkafka_str())
        .set("session.timeout.ms", session_timeout_ms.to_string());
    apply_extra_options(&mut client_config, &config.connection.extra_options);

    let consumer: StreamConsumer<KafkaConsumerContext> = client_config
        .create_with_context(context)
        .map_err(|e| dependency_error("get_kafka_consumer: create", e))?;

    let topic_refs: Vec<&str> = topics.iter().map(String::as_str).collect();
    consumer
        .subscribe(&topic_refs)
        .map_err(|e| dependency_error("get_kafka_consumer: subscribe", e))?;

    tracing::info!(topics = ?topics, group_id = %group_id, "Kafka consumer initialized");

    Ok(KafkaConsumer {
        consumer,
        partition_reassignment_in_progress,
    })
}

impl KafkaConsumer {
    pub fn messages(&self) -> impl Stream<Item = Result<KafkaMessage, AppError>> + '_ {
        self.consumer.stream().filter_map(|message_result| {
            match message_result {
                Ok(message) => {
                    let key = message
                        .key()
                        .and_then(|k| std::str::from_utf8(k).ok())
                        .unwrap_or("")
                        .to_string();

                    match message.payload().map(serde_json::from_slice::<serde_json::Value>) {
                        Some(Ok(value)) => {
                            if key.is_empty() || is_empty_json_value(&value) {
                                tracing::warn!(
                                    key = %key,
                                    value = %value,
                                    "Kafka: skipping message with empty key or null/empty value"
                                );
                                None
                            } else {
                                Some(Ok(KafkaMessage {
                                    key,
                                    value,
                                    raw: message.detach(),
                                }))
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "Kafka: failed to parse message payload as JSON");
                            None
                        }
                        None => None,
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Kafka: consumer error");
                    Some(Err(dependency_error("messages", e)))
                }
            }
        })
    }

    pub fn commit(&self, message: &KafkaMessage, mode: CommitMode) -> Result<(), AppError> {
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(
            message.raw.topic(),
            message.raw.partition(),
            Offset::Offset(message.raw.offset().saturating_add(1)),
        )
        .map_err(|e| dependency_error("commit: build offset list", e))?;

        self.consumer
            .commit(&tpl, mode)
            .map_err(|e| dependency_error("commit", e))
    }

    pub fn is_rebalancing(&self) -> Result<bool, AppError> {
        let guard = self
            .partition_reassignment_in_progress
            .read()
            .map_err(|_| lock_error())?;
        Ok(*guard)
    }
}
