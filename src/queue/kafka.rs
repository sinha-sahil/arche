use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rdkafka::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer, ConsumerContext, Rebalance, StreamConsumer};
use rdkafka::message::{Header, Message, OwnedHeaders, OwnedMessage};
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use tokio_stream::{Stream, StreamExt};

use crate::error::AppError;

pub use crate::config::kafka::{
    KafkaConnectionConfig, KafkaConnectionConfigBuilder, KafkaConsumerConfig,
    KafkaConsumerConfigBuilder, KafkaProducerConfig, KafkaProducerConfigBuilder,
};
pub use rdkafka::consumer::CommitMode;

const DEFAULT_SOCKET_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MESSAGE_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_SESSION_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_AUTO_COMMIT_INTERVAL_MS: u64 = 5_000;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityProtocol {
    #[default]
    Plaintext,
    Ssl,
    SaslPlaintext,
    SaslSsl,
}

impl SecurityProtocol {
    fn as_librdkafka_str(self) -> &'static str {
        match self {
            Self::Plaintext => "plaintext",
            Self::Ssl => "ssl",
            Self::SaslPlaintext => "sasl_plaintext",
            Self::SaslSsl => "sasl_ssl",
        }
    }

    fn requires_sasl(self) -> bool {
        matches!(self, Self::SaslPlaintext | Self::SaslSsl)
    }
}

impl FromStr for SecurityProtocol {
    type Err = AppError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "plaintext" => Ok(Self::Plaintext),
            "ssl" => Ok(Self::Ssl),
            "sasl_plaintext" => Ok(Self::SaslPlaintext),
            "sasl_ssl" => Ok(Self::SaslSsl),
            other => Err(AppError::internal_error(
                format!("Unknown security_protocol value: {other}"),
                None,
            )),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaslMechanism {
    Plain,
    ScramSha256,
    ScramSha512,
}

impl SaslMechanism {
    fn as_librdkafka_str(self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::ScramSha256 => "SCRAM-SHA-256",
            Self::ScramSha512 => "SCRAM-SHA-512",
        }
    }
}

impl FromStr for SaslMechanism {
    type Err = AppError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "PLAIN" => Ok(Self::Plain),
            "SCRAM-SHA-256" => Ok(Self::ScramSha256),
            "SCRAM-SHA-512" => Ok(Self::ScramSha512),
            other => Err(AppError::internal_error(
                format!("Unknown sasl_mechanism value: {other}"),
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

fn config_error(field: &str, reason: &str) -> AppError {
    AppError::internal_error(format!("Config error [kafka/{field}]: {reason}"), None)
}

fn lock_error() -> AppError {
    AppError::internal_error(
        "Lock poisoned".to_string(),
        Some("A thread panicked while holding the Kafka rebalance lock".to_string()),
    )
}

fn require_non_empty_list(list: Option<Vec<String>>, field: &str) -> Result<Vec<String>, AppError> {
    let list = list
        .filter(|l| !l.is_empty())
        .ok_or_else(|| config_error(field, "at least one value is required"))?;
    if list.iter().any(String::is_empty) {
        return Err(config_error(field, "values must not be empty strings"));
    }
    Ok(list)
}

fn require_non_empty_string(value: Option<String>, field: &str) -> Result<String, AppError> {
    value
        .filter(|v| !v.is_empty())
        .ok_or_else(|| config_error(field, "value is required and must not be empty"))
}

fn socket_timeout_ms(connection: &KafkaConnectionConfig) -> u64 {
    connection
        .socket_timeout_ms
        .unwrap_or(DEFAULT_SOCKET_TIMEOUT_MS)
}

pub fn build_client_config(connection: &KafkaConnectionConfig) -> Result<ClientConfig, AppError> {
    client_config_with_defaults(connection, &[])
}

fn client_config_with_defaults(
    connection: &KafkaConnectionConfig,
    role_defaults: &[(&str, String)],
) -> Result<ClientConfig, AppError> {
    let brokers = require_non_empty_list(connection.brokers.clone(), "brokers")?;

    let mut client_config = ClientConfig::new();
    client_config
        .set("bootstrap.servers", brokers.join(","))
        .set(
            "socket.timeout.ms",
            socket_timeout_ms(connection).to_string(),
        );

    if let Some(protocol) = connection.security_protocol {
        client_config.set("security.protocol", protocol.as_librdkafka_str());

        if protocol.requires_sasl() {
            if connection.sasl_mechanism.is_none() {
                return Err(config_error(
                    "sasl_mechanism",
                    "required when security_protocol is SaslPlaintext or SaslSsl",
                ));
            }
            require_non_empty_string(connection.sasl_username.clone(), "sasl_username")?;
            require_non_empty_string(connection.sasl_password.clone(), "sasl_password")?;
        }
    }

    if let Some(mechanism) = connection.sasl_mechanism {
        client_config.set("sasl.mechanism", mechanism.as_librdkafka_str());
    }
    if let Some(username) = &connection.sasl_username {
        client_config.set("sasl.username", username);
    }
    if let Some(password) = &connection.sasl_password {
        client_config.set("sasl.password", password);
    }
    if let Some(path) = &connection.ssl_ca_location {
        client_config.set("ssl.ca.location", path);
    }
    if let Some(path) = &connection.ssl_certificate_location {
        client_config.set("ssl.certificate.location", path);
    }
    if let Some(path) = &connection.ssl_key_location {
        client_config.set("ssl.key.location", path);
    }
    if let Some(password) = &connection.ssl_key_password {
        client_config.set("ssl.key.password", password);
    }

    for (key, value) in role_defaults {
        client_config.set(*key, value);
    }

    for (key, value) in connection.extra_options.iter().flatten() {
        client_config.set(key, value);
    }

    Ok(client_config)
}

pub async fn test_kafka(
    config: impl Into<Option<KafkaConnectionConfig>>,
) -> Result<bool, AppError> {
    let config = config.into().unwrap_or_default();
    let timeout = Duration::from_millis(socket_timeout_ms(&config));

    let consumer: BaseConsumer = build_client_config(&config)?
        .create()
        .map_err(|e| dependency_error("test_kafka: create client", e))?;

    tokio::task::spawn_blocking(move || consumer.fetch_metadata(None, timeout).map(|_| true))
        .await
        .map_err(|e| AppError::internal_error(format!("test_kafka: task join error: {e}"), None))?
        .map_err(|e| dependency_error("test_kafka: fetch_metadata", e))
}

pub struct KafkaProducer {
    producer: FutureProducer,
    topic: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
}

#[derive(Debug, Clone, Default)]
pub struct OutboundMessage {
    pub topic: Option<String>,
    pub key: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub headers: Vec<(String, Vec<u8>)>,
    pub timestamp_ms: Option<i64>,
    pub partition: Option<i32>,
}

impl OutboundMessage {
    pub fn new(payload: impl Into<Vec<u8>>) -> Self {
        Self {
            payload: Some(payload.into()),
            ..Self::default()
        }
    }

    pub fn tombstone() -> Self {
        Self::default()
    }

    pub fn json(value: &serde_json::Value) -> Result<Self, AppError> {
        let payload = serde_json::to_vec(value).map_err(|e| {
            AppError::internal_error(format!("Failed to serialize JSON payload: {e}"), None)
        })?;
        Ok(Self::new(payload))
    }

    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn header(mut self, key: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn headers<I, K, V>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<Vec<u8>>,
    {
        self.headers
            .extend(headers.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    pub fn timestamp_ms(mut self, timestamp_ms: i64) -> Self {
        self.timestamp_ms = Some(timestamp_ms);
        self
    }

    pub fn partition(mut self, partition: i32) -> Self {
        self.partition = Some(partition);
        self
    }
}

fn producer_client_config(config: &KafkaProducerConfig) -> Result<ClientConfig, AppError> {
    let message_timeout_ms = config
        .message_timeout_ms
        .unwrap_or(DEFAULT_MESSAGE_TIMEOUT_MS);

    client_config_with_defaults(
        &config.connection,
        &[
            ("message.timeout.ms", message_timeout_ms.to_string()),
            ("enable.idempotence", "true".to_string()),
        ],
    )
}

pub async fn get_kafka_producer(
    config: impl Into<Option<KafkaProducerConfig>>,
) -> Result<KafkaProducer, AppError> {
    let config = config.into().unwrap_or_default();

    let client_config = producer_client_config(&config)?;
    let topic = require_non_empty_string(config.topic, "topic")?;

    let producer: FutureProducer = client_config
        .create()
        .map_err(|e| dependency_error("get_kafka_producer: create", e))?;

    tracing::info!(topic = %topic, "Kafka producer initialized");

    Ok(KafkaProducer { producer, topic })
}

impl KafkaProducer {
    pub async fn send(&self, message: OutboundMessage) -> Result<DeliveryReport, AppError> {
        let topic = message.topic.as_deref().unwrap_or(&self.topic);
        if topic.is_empty() {
            return Err(config_error("message.topic", "must not be empty"));
        }

        let mut headers = OwnedHeaders::new_with_capacity(message.headers.len());
        for (key, value) in &message.headers {
            headers = headers.insert(Header {
                key,
                value: Some(value.as_slice()),
            });
        }

        let mut record = FutureRecord::<String, Vec<u8>>::to(topic).headers(headers);
        if let Some(key) = &message.key {
            record = record.key(key);
        }
        if let Some(payload) = &message.payload {
            record = record.payload(payload);
        }
        if let Some(timestamp_ms) = message.timestamp_ms {
            record = record.timestamp(timestamp_ms);
        }
        if let Some(partition) = message.partition {
            record = record.partition(partition);
        }

        self.dispatch(record, "send").await
    }

    pub async fn send_all(
        &self,
        messages: Vec<OutboundMessage>,
    ) -> Vec<Result<DeliveryReport, AppError>> {
        futures::future::join_all(messages.into_iter().map(|m| self.send(m))).await
    }

    pub async fn flush(&self, timeout: Duration) -> Result<(), AppError> {
        self.producer
            .flush(timeout)
            .map_err(|e| dependency_error("flush", e))
    }

    pub fn in_flight(&self) -> i32 {
        self.producer.in_flight_count()
    }

    async fn dispatch<K, P>(
        &self,
        record: FutureRecord<'_, K, P>,
        op: &'static str,
    ) -> Result<DeliveryReport, AppError>
    where
        K: rdkafka::message::ToBytes + ?Sized,
        P: rdkafka::message::ToBytes + ?Sized,
    {
        let topic = record.topic.to_string();
        let delivery = self
            .producer
            .send_result(record)
            .map_err(|(e, _)| dependency_error(op, e))?;
        match delivery.await {
            Ok(Ok(delivery)) => Ok(DeliveryReport {
                topic,
                partition: delivery.partition,
                offset: delivery.offset,
            }),
            Ok(Err((e, _))) => Err(dependency_error(op, e)),
            Err(_) => Err(AppError::internal_error(
                format!("{op}: delivery future canceled"),
                None,
            )),
        }
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
pub struct KafkaRawMessage {
    raw: OwnedMessage,
}

impl KafkaRawMessage {
    pub fn key(&self) -> Option<&[u8]> {
        self.raw.key()
    }

    pub fn payload(&self) -> Option<&[u8]> {
        self.raw.payload()
    }

    pub fn topic(&self) -> &str {
        self.raw.topic()
    }

    pub fn partition(&self) -> i32 {
        self.raw.partition()
    }

    pub fn offset(&self) -> i64 {
        self.raw.offset()
    }

    pub fn headers(&self) -> Option<&OwnedHeaders> {
        self.raw.headers()
    }

    pub fn into_inner(self) -> OwnedMessage {
        self.raw
    }
}

#[derive(Debug, Clone)]
pub struct KafkaMessage {
    pub key: Option<String>,
    pub value: serde_json::Value,
    raw: OwnedMessage,
}

impl KafkaMessage {
    pub fn topic(&self) -> &str {
        self.raw.topic()
    }

    pub fn partition(&self) -> i32 {
        self.raw.partition()
    }

    pub fn offset(&self) -> i64 {
        self.raw.offset()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KafkaConsumeError {
    #[error("{0}")]
    Consumer(AppError),
    #[error("Kafka: failed to decode message at {}[{}]@{}: {reason}", message.topic(), message.partition(), message.offset())]
    Decode {
        reason: String,
        message: Box<KafkaRawMessage>,
    },
}

impl From<KafkaConsumeError> for AppError {
    fn from(err: KafkaConsumeError) -> Self {
        match err {
            KafkaConsumeError::Consumer(e) => e,
            decode @ KafkaConsumeError::Decode { .. } => {
                AppError::internal_error(decode.to_string(), None)
            }
        }
    }
}

pub struct KafkaConsumer {
    consumer: StreamConsumer<KafkaConsumerContext>,
    partition_reassignment_in_progress: Arc<RwLock<bool>>,
}

fn consumer_client_config(
    config: &KafkaConsumerConfig,
    group_id: &str,
) -> Result<ClientConfig, AppError> {
    let session_timeout_ms = config
        .session_timeout_ms
        .unwrap_or(DEFAULT_SESSION_TIMEOUT_MS);
    let auto_offset_reset = config.auto_offset_reset.unwrap_or_default();
    let auto_commit = config.auto_commit.unwrap_or(false);
    let auto_commit_interval_ms = config
        .auto_commit_interval_ms
        .unwrap_or(DEFAULT_AUTO_COMMIT_INTERVAL_MS);

    client_config_with_defaults(
        &config.connection,
        &[
            ("group.id", group_id.to_string()),
            ("enable.auto.commit", auto_commit.to_string()),
            (
                "auto.commit.interval.ms",
                auto_commit_interval_ms.to_string(),
            ),
            (
                "auto.offset.reset",
                auto_offset_reset.as_librdkafka_str().to_string(),
            ),
            ("session.timeout.ms", session_timeout_ms.to_string()),
        ],
    )
}

pub async fn get_kafka_consumer(
    config: impl Into<Option<KafkaConsumerConfig>>,
) -> Result<KafkaConsumer, AppError> {
    let config = config.into().unwrap_or_default();

    let topics = require_non_empty_list(config.topics.clone(), "topics")?;
    let group_id = require_non_empty_string(config.group_id.clone(), "group_id")?;

    let client_config = consumer_client_config(&config, &group_id)?;

    let partition_reassignment_in_progress = Arc::new(RwLock::new(false));
    let context = KafkaConsumerContext {
        partition_reassignment_in_progress: partition_reassignment_in_progress.clone(),
    };

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
    pub fn raw_messages(&self) -> impl Stream<Item = Result<KafkaRawMessage, AppError>> + '_ {
        self.consumer.stream().map(|message_result| {
            message_result
                .map(|message| KafkaRawMessage {
                    raw: message.detach(),
                })
                .map_err(|e| {
                    tracing::error!(error = %e, "Kafka: consumer error");
                    dependency_error("raw_messages", e)
                })
        })
    }

    pub fn messages(&self) -> impl Stream<Item = Result<KafkaMessage, KafkaConsumeError>> + '_ {
        self.raw_messages().map(|result| match result {
            Ok(message) => Self::decode_json_message(message),
            Err(e) => Err(KafkaConsumeError::Consumer(e)),
        })
    }

    fn decode_json_message(message: KafkaRawMessage) -> Result<KafkaMessage, KafkaConsumeError> {
        let key = match message.key().map(std::str::from_utf8) {
            None => None,
            Some(Ok(k)) => Some(k.to_string()),
            Some(Err(e)) => {
                return Err(KafkaConsumeError::Decode {
                    reason: format!("key is not valid UTF-8: {e}"),
                    message: Box::new(message),
                });
            }
        };

        let value = match message.payload().map(serde_json::from_slice) {
            None => serde_json::Value::Null,
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                return Err(KafkaConsumeError::Decode {
                    reason: format!("payload is not valid JSON: {e}"),
                    message: Box::new(message),
                });
            }
        };

        Ok(KafkaMessage {
            key,
            value,
            raw: message.raw,
        })
    }

    pub fn commit(&self, message: &KafkaMessage, mode: CommitMode) -> Result<(), AppError> {
        self.commit_owned(&message.raw, mode)
    }

    pub fn commit_raw(&self, message: &KafkaRawMessage, mode: CommitMode) -> Result<(), AppError> {
        self.commit_owned(&message.raw, mode)
    }

    fn commit_owned(&self, raw: &OwnedMessage, mode: CommitMode) -> Result<(), AppError> {
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(
            raw.topic(),
            raw.partition(),
            Offset::Offset(raw.offset().saturating_add(1)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_config_error(err: AppError, field: &str) {
        match err {
            AppError::InternalError { error, .. } => {
                assert!(
                    error.contains(&format!("[kafka/{field}]")),
                    "unexpected error: {error}"
                );
            }
            other => panic!("expected InternalError, got {other:?}"),
        }
    }

    #[test]
    fn enums_parse_case_insensitively() {
        assert_eq!(
            "LATEST".parse::<AutoOffsetReset>().ok(),
            Some(AutoOffsetReset::Latest)
        );
        assert_eq!(
            "sasl_ssl".parse::<SecurityProtocol>().ok(),
            Some(SecurityProtocol::SaslSsl)
        );
        assert_eq!(
            "scram-sha-512".parse::<SaslMechanism>().ok(),
            Some(SaslMechanism::ScramSha512)
        );
        assert!("nope".parse::<AutoOffsetReset>().is_err());
        assert!("nope".parse::<SecurityProtocol>().is_err());
        assert!("nope".parse::<SaslMechanism>().is_err());
    }

    #[test]
    fn require_helpers_reject_missing_and_empty() {
        assert_config_error(
            require_non_empty_list(None, "brokers").unwrap_err(),
            "brokers",
        );
        assert_config_error(
            require_non_empty_list(Some(vec![]), "brokers").unwrap_err(),
            "brokers",
        );
        assert_config_error(
            require_non_empty_list(Some(vec!["a".into(), String::new()]), "topics").unwrap_err(),
            "topics",
        );
        assert_eq!(
            require_non_empty_list(Some(vec!["a".into()]), "brokers").ok(),
            Some(vec!["a".to_string()])
        );

        assert_config_error(
            require_non_empty_string(None, "topic").unwrap_err(),
            "topic",
        );
        assert_config_error(
            require_non_empty_string(Some(String::new()), "group_id").unwrap_err(),
            "group_id",
        );
        assert_eq!(
            require_non_empty_string(Some("x".into()), "topic").ok(),
            Some("x".to_string())
        );
    }

    #[test]
    fn client_config_sets_defaults_and_security_fields() {
        let connection = KafkaConnectionConfig::builder()
            .brokers(["a:9092", "b:9092"])
            .security_protocol(SecurityProtocol::SaslSsl)
            .sasl_mechanism(SaslMechanism::ScramSha512)
            .sasl_username("user")
            .sasl_password("pass")
            .ssl_ca_location("/ca.pem")
            .build();

        let config = build_client_config(&connection).expect("valid config");
        assert_eq!(config.get("bootstrap.servers"), Some("a:9092,b:9092"));
        assert_eq!(config.get("socket.timeout.ms"), Some("5000"));
        assert_eq!(config.get("security.protocol"), Some("sasl_ssl"));
        assert_eq!(config.get("sasl.mechanism"), Some("SCRAM-SHA-512"));
        assert_eq!(config.get("sasl.username"), Some("user"));
        assert_eq!(config.get("sasl.password"), Some("pass"));
        assert_eq!(config.get("ssl.ca.location"), Some("/ca.pem"));
    }

    #[test]
    fn client_config_requires_sasl_fields_for_sasl_protocols() {
        let base = || {
            KafkaConnectionConfig::builder()
                .broker("a:9092")
                .security_protocol(SecurityProtocol::SaslPlaintext)
        };

        assert_config_error(
            build_client_config(&base().build()).unwrap_err(),
            "sasl_mechanism",
        );
        assert_config_error(
            build_client_config(&base().sasl_mechanism(SaslMechanism::Plain).build()).unwrap_err(),
            "sasl_username",
        );
        assert_config_error(
            build_client_config(
                &base()
                    .sasl_mechanism(SaslMechanism::Plain)
                    .sasl_username("u")
                    .build(),
            )
            .unwrap_err(),
            "sasl_password",
        );
        assert!(
            build_client_config(
                &base()
                    .sasl_mechanism(SaslMechanism::Plain)
                    .sasl_username("u")
                    .sasl_password("p")
                    .build(),
            )
            .is_ok()
        );
    }

    #[test]
    fn client_config_plain_ssl_does_not_require_sasl() {
        let connection = KafkaConnectionConfig::builder()
            .broker("a:9092")
            .security_protocol(SecurityProtocol::Ssl)
            .build();
        let config = build_client_config(&connection).expect("valid config");
        assert_eq!(config.get("security.protocol"), Some("ssl"));
        assert_eq!(config.get("sasl.mechanism"), None);
    }

    #[test]
    fn extra_options_override_typed_fields() {
        let connection = KafkaConnectionConfig::builder()
            .broker("a:9092")
            .socket_timeout_ms(1_000)
            .extra_option("socket.timeout.ms", "9000")
            .extra_option("client.id", "svc")
            .build();
        let config = build_client_config(&connection).expect("valid config");
        assert_eq!(config.get("socket.timeout.ms"), Some("9000"));
        assert_eq!(config.get("client.id"), Some("svc"));
    }

    #[test]
    fn role_defaults_sit_between_typed_fields_and_extra_options() {
        let connection = KafkaConnectionConfig::builder()
            .broker("a:9092")
            .extra_option("enable.idempotence", "false")
            .build();
        let config = client_config_with_defaults(
            &connection,
            &[
                ("enable.idempotence", "true".to_string()),
                ("acks", "all".to_string()),
            ],
        )
        .expect("valid config");
        assert_eq!(config.get("acks"), Some("all")); // default kept
        assert_eq!(config.get("enable.idempotence"), Some("false")); // user override wins
    }

    #[test]
    fn outbound_message_builder() {
        let message = OutboundMessage::json(&serde_json::json!({"a": 1}))
            .expect("serializable")
            .topic("orders")
            .key("k1")
            .header("trace-id", "abc")
            .headers([("h2", vec![1u8, 2])])
            .timestamp_ms(1_700_000_000_000)
            .partition(2);
        assert_eq!(message.topic.as_deref(), Some("orders"));
        assert_eq!(message.key.as_deref(), Some("k1"));
        assert_eq!(message.payload.as_deref(), Some(br#"{"a":1}"#.as_slice()));
        assert_eq!(message.headers.len(), 2);
        assert_eq!(message.timestamp_ms, Some(1_700_000_000_000));
        assert_eq!(message.partition, Some(2));

        let tombstone = OutboundMessage::tombstone().key("k1");
        assert!(tombstone.payload.is_none());
        assert!(tombstone.topic.is_none()); // falls back to the producer's topic
    }

    #[test]
    fn producer_defaults_are_overridable_by_extra_options() {
        let config = KafkaProducerConfig::builder()
            .broker("a:9092")
            .topic("orders")
            .build();
        let defaults = producer_client_config(&config).expect("valid config");
        assert_eq!(defaults.get("enable.idempotence"), Some("true"));
        assert_eq!(defaults.get("message.timeout.ms"), Some("5000"));

        let overridden = KafkaProducerConfig::builder()
            .broker("a:9092")
            .topic("orders")
            .extra_options_from_str("enable.idempotence=false,message.timeout.ms=60000")
            .build();
        let overridden = producer_client_config(&overridden).expect("valid config");
        assert_eq!(overridden.get("enable.idempotence"), Some("false"));
        assert_eq!(overridden.get("message.timeout.ms"), Some("60000"));
    }

    #[test]
    fn consumer_defaults_are_overridable_by_extra_options() {
        let config = KafkaConsumerConfig::builder()
            .broker("a:9092")
            .topic("orders")
            .group_id("svc")
            .build();
        let defaults = consumer_client_config(&config, "svc").expect("valid config");
        assert_eq!(defaults.get("enable.auto.commit"), Some("false"));
        assert_eq!(defaults.get("auto.offset.reset"), Some("earliest"));
        assert_eq!(defaults.get("session.timeout.ms"), Some("30000"));

        let overridden = KafkaConsumerConfig::builder()
            .broker("a:9092")
            .topic("orders")
            .group_id("svc")
            .extra_options_from_str("enable.auto.commit=true,session.timeout.ms=45000")
            .build();
        let overridden = consumer_client_config(&overridden, "svc").expect("valid config");
        assert_eq!(overridden.get("enable.auto.commit"), Some("true"));
        assert_eq!(overridden.get("session.timeout.ms"), Some("45000"));
    }

    #[test]
    fn missing_brokers_is_config_error() {
        assert_config_error(
            build_client_config(&KafkaConnectionConfig::default()).unwrap_err(),
            "brokers",
        );
    }
}
