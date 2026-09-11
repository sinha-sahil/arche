use std::collections::HashMap;

use crate::queue::kafka::{AutoOffsetReset, SaslMechanism, SecurityProtocol};

#[derive(Debug, Clone, Default)]
pub struct KafkaConnectionConfig {
    pub brokers: Option<Vec<String>>,
    pub socket_timeout_ms: Option<u64>,
    pub security_protocol: Option<SecurityProtocol>,
    pub sasl_mechanism: Option<SaslMechanism>,
    pub sasl_username: Option<String>,
    pub sasl_password: Option<String>,
    pub ssl_ca_location: Option<String>,
    pub ssl_certificate_location: Option<String>,
    pub ssl_key_location: Option<String>,
    pub ssl_key_password: Option<String>,
    pub extra_options: Option<HashMap<String, String>>,
}

impl KafkaConnectionConfig {
    pub fn builder() -> KafkaConnectionConfigBuilder {
        KafkaConnectionConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct KafkaConnectionConfigBuilder {
    brokers: Option<Vec<String>>,
    socket_timeout_ms: Option<u64>,
    security_protocol: Option<SecurityProtocol>,
    sasl_mechanism: Option<SaslMechanism>,
    sasl_username: Option<String>,
    sasl_password: Option<String>,
    ssl_ca_location: Option<String>,
    ssl_certificate_location: Option<String>,
    ssl_key_location: Option<String>,
    ssl_key_password: Option<String>,
    extra_options: Option<HashMap<String, String>>,
}

impl KafkaConnectionConfigBuilder {
    pub fn broker(mut self, broker: impl Into<String>) -> Self {
        let broker = broker.into();
        match self.brokers.as_mut() {
            Some(existing) => existing.push(broker),
            None => self.brokers = Some(vec![broker]),
        }
        self
    }

    pub fn brokers<I, S>(mut self, brokers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.brokers = Some(brokers.into_iter().map(Into::into).collect());
        self
    }

    pub fn socket_timeout_ms(mut self, ms: u64) -> Self {
        self.socket_timeout_ms = Some(ms);
        self
    }

    pub fn security_protocol(mut self, protocol: SecurityProtocol) -> Self {
        self.security_protocol = Some(protocol);
        self
    }

    pub fn sasl_mechanism(mut self, mechanism: SaslMechanism) -> Self {
        self.sasl_mechanism = Some(mechanism);
        self
    }

    pub fn sasl_username(mut self, username: impl Into<String>) -> Self {
        self.sasl_username = Some(username.into());
        self
    }

    pub fn sasl_password(mut self, password: impl Into<String>) -> Self {
        self.sasl_password = Some(password.into());
        self
    }

    pub fn ssl_ca_location(mut self, path: impl Into<String>) -> Self {
        self.ssl_ca_location = Some(path.into());
        self
    }

    pub fn ssl_certificate_location(mut self, path: impl Into<String>) -> Self {
        self.ssl_certificate_location = Some(path.into());
        self
    }

    pub fn ssl_key_location(mut self, path: impl Into<String>) -> Self {
        self.ssl_key_location = Some(path.into());
        self
    }

    pub fn ssl_key_password(mut self, password: impl Into<String>) -> Self {
        self.ssl_key_password = Some(password.into());
        self
    }

    pub fn extra_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_options
            .get_or_insert_with(HashMap::new)
            .insert(key.into(), value.into());
        self
    }

    pub fn extra_options<I, K, V>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.extra_options = Some(
            options
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        );
        self
    }

    pub fn extra_options_from_str(mut self, options: &str) -> Self {
        let map = self.extra_options.get_or_insert_with(HashMap::new);
        for pair in options.split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            if let Some((key, value)) = pair.split_once('=') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        self
    }

    pub fn build(self) -> KafkaConnectionConfig {
        KafkaConnectionConfig {
            brokers: self.brokers,
            socket_timeout_ms: self.socket_timeout_ms,
            security_protocol: self.security_protocol,
            sasl_mechanism: self.sasl_mechanism,
            sasl_username: self.sasl_username,
            sasl_password: self.sasl_password,
            ssl_ca_location: self.ssl_ca_location,
            ssl_certificate_location: self.ssl_certificate_location,
            ssl_key_location: self.ssl_key_location,
            ssl_key_password: self.ssl_key_password,
            extra_options: self.extra_options,
        }
    }
}

impl From<KafkaConnectionConfig> for KafkaConnectionConfigBuilder {
    fn from(config: KafkaConnectionConfig) -> Self {
        Self {
            brokers: config.brokers,
            socket_timeout_ms: config.socket_timeout_ms,
            security_protocol: config.security_protocol,
            sasl_mechanism: config.sasl_mechanism,
            sasl_username: config.sasl_username,
            sasl_password: config.sasl_password,
            ssl_ca_location: config.ssl_ca_location,
            ssl_certificate_location: config.ssl_certificate_location,
            ssl_key_location: config.ssl_key_location,
            ssl_key_password: config.ssl_key_password,
            extra_options: config.extra_options,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KafkaProducerConfig {
    pub connection: KafkaConnectionConfig,
    pub topic: Option<String>,
    pub message_timeout_ms: Option<u64>,
}

impl KafkaProducerConfig {
    pub fn builder() -> KafkaProducerConfigBuilder {
        KafkaProducerConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct KafkaProducerConfigBuilder {
    connection: KafkaConnectionConfigBuilder,
    topic: Option<String>,
    message_timeout_ms: Option<u64>,
}

impl KafkaProducerConfigBuilder {
    pub fn connection(mut self, connection: KafkaConnectionConfig) -> Self {
        self.connection = connection.into();
        self
    }

    pub fn broker(mut self, broker: impl Into<String>) -> Self {
        self.connection = self.connection.broker(broker);
        self
    }

    pub fn brokers<I, S>(mut self, brokers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.connection = self.connection.brokers(brokers);
        self
    }

    pub fn socket_timeout_ms(mut self, ms: u64) -> Self {
        self.connection = self.connection.socket_timeout_ms(ms);
        self
    }

    pub fn extra_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.connection = self.connection.extra_option(key, value);
        self
    }

    pub fn extra_options<I, K, V>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.connection = self.connection.extra_options(options);
        self
    }

    pub fn extra_options_from_str(mut self, options: &str) -> Self {
        self.connection = self.connection.extra_options_from_str(options);
        self
    }

    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    pub fn message_timeout_ms(mut self, ms: u64) -> Self {
        self.message_timeout_ms = Some(ms);
        self
    }

    pub fn build(self) -> KafkaProducerConfig {
        KafkaProducerConfig {
            connection: self.connection.build(),
            topic: self.topic,
            message_timeout_ms: self.message_timeout_ms,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KafkaConsumerConfig {
    pub connection: KafkaConnectionConfig,
    pub topics: Option<Vec<String>>,
    pub group_id: Option<String>,
    pub session_timeout_ms: Option<u64>,
    pub auto_offset_reset: Option<AutoOffsetReset>,
    pub auto_commit: Option<bool>,
    pub auto_commit_interval_ms: Option<u64>,
}

impl KafkaConsumerConfig {
    pub fn builder() -> KafkaConsumerConfigBuilder {
        KafkaConsumerConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct KafkaConsumerConfigBuilder {
    connection: KafkaConnectionConfigBuilder,
    topics: Option<Vec<String>>,
    group_id: Option<String>,
    session_timeout_ms: Option<u64>,
    auto_offset_reset: Option<AutoOffsetReset>,
    auto_commit: Option<bool>,
    auto_commit_interval_ms: Option<u64>,
}

impl KafkaConsumerConfigBuilder {
    pub fn connection(mut self, connection: KafkaConnectionConfig) -> Self {
        self.connection = connection.into();
        self
    }

    pub fn broker(mut self, broker: impl Into<String>) -> Self {
        self.connection = self.connection.broker(broker);
        self
    }

    pub fn brokers<I, S>(mut self, brokers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.connection = self.connection.brokers(brokers);
        self
    }

    pub fn socket_timeout_ms(mut self, ms: u64) -> Self {
        self.connection = self.connection.socket_timeout_ms(ms);
        self
    }

    pub fn extra_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.connection = self.connection.extra_option(key, value);
        self
    }

    pub fn extra_options<I, K, V>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.connection = self.connection.extra_options(options);
        self
    }

    pub fn extra_options_from_str(mut self, options: &str) -> Self {
        self.connection = self.connection.extra_options_from_str(options);
        self
    }

    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        let topic = topic.into();
        match self.topics.as_mut() {
            Some(existing) => existing.push(topic),
            None => self.topics = Some(vec![topic]),
        }
        self
    }

    pub fn topics<I, S>(mut self, topics: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.topics = Some(topics.into_iter().map(Into::into).collect());
        self
    }

    pub fn group_id(mut self, group_id: impl Into<String>) -> Self {
        self.group_id = Some(group_id.into());
        self
    }

    pub fn session_timeout_ms(mut self, ms: u64) -> Self {
        self.session_timeout_ms = Some(ms);
        self
    }

    pub fn auto_offset_reset(mut self, value: AutoOffsetReset) -> Self {
        self.auto_offset_reset = Some(value);
        self
    }

    pub fn auto_commit(mut self, value: bool) -> Self {
        self.auto_commit = Some(value);
        self
    }

    pub fn auto_commit_interval_ms(mut self, ms: u64) -> Self {
        self.auto_commit_interval_ms = Some(ms);
        self
    }

    pub fn build(self) -> KafkaConsumerConfig {
        KafkaConsumerConfig {
            connection: self.connection.build(),
            topics: self.topics,
            group_id: self.group_id,
            session_timeout_ms: self.session_timeout_ms,
            auto_offset_reset: self.auto_offset_reset,
            auto_commit: self.auto_commit,
            auto_commit_interval_ms: self.auto_commit_interval_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_builder_accumulates_brokers_and_options() {
        let config = KafkaConnectionConfig::builder()
            .broker("a:9092")
            .broker("b:9092")
            .extra_option("k1", "v1")
            .extra_option("k2", "v2")
            .build();

        assert_eq!(
            config.brokers,
            Some(vec!["a:9092".to_string(), "b:9092".to_string()])
        );
        let extra = config.extra_options.unwrap_or_default();
        assert_eq!(extra.get("k1").map(String::as_str), Some("v1"));
        assert_eq!(extra.get("k2").map(String::as_str), Some("v2"));
    }

    #[test]
    fn extra_options_from_str_parses_env_style_pairs() {
        let config = KafkaConnectionConfig::builder()
            .extra_option("client.id", "old")
            .extra_options_from_str(" client.id=svc , linger.ms=20,,broken, sasl.password=a=b ")
            .build();
        let extra = config.extra_options.unwrap_or_default();
        assert_eq!(extra.get("client.id").map(String::as_str), Some("svc"));
        assert_eq!(extra.get("linger.ms").map(String::as_str), Some("20"));
        assert_eq!(extra.get("sasl.password").map(String::as_str), Some("a=b"));
        assert_eq!(extra.len(), 3);
    }

    #[test]
    fn brokers_replaces_previous_list() {
        let config = KafkaConnectionConfig::builder()
            .broker("old:9092")
            .brokers(["a:9092", "b:9092"])
            .build();
        assert_eq!(
            config.brokers,
            Some(vec!["a:9092".to_string(), "b:9092".to_string()])
        );
    }

    #[test]
    fn producer_builder_layers_on_top_of_shared_connection() {
        let connection = KafkaConnectionConfig::builder()
            .security_protocol(SecurityProtocol::SaslSsl)
            .sasl_mechanism(SaslMechanism::ScramSha512)
            .sasl_username("user")
            .sasl_password("pass")
            .ssl_ca_location("/ca.pem")
            .build();

        let config = KafkaProducerConfig::builder()
            .connection(connection)
            .broker("a:9092")
            .socket_timeout_ms(1_000)
            .topic("orders")
            .message_timeout_ms(2_000)
            .build();

        assert_eq!(config.topic.as_deref(), Some("orders"));
        assert_eq!(config.message_timeout_ms, Some(2_000));
        assert_eq!(config.connection.socket_timeout_ms, Some(1_000));
        assert_eq!(
            config.connection.security_protocol,
            Some(SecurityProtocol::SaslSsl)
        );
        assert_eq!(
            config.connection.sasl_mechanism,
            Some(SaslMechanism::ScramSha512)
        );
        assert_eq!(config.connection.sasl_username.as_deref(), Some("user"));
        assert_eq!(config.connection.sasl_password.as_deref(), Some("pass"));
        assert_eq!(
            config.connection.ssl_ca_location.as_deref(),
            Some("/ca.pem")
        );
    }

    #[test]
    fn consumer_builder_accumulates_topics_and_accepts_shared_connection() {
        let connection = KafkaConnectionConfig::builder()
            .broker("a:9092")
            .security_protocol(SecurityProtocol::Ssl)
            .build();

        let config = KafkaConsumerConfig::builder()
            .connection(connection)
            .topic("orders")
            .topic("returns")
            .group_id("svc")
            .auto_offset_reset(AutoOffsetReset::Latest)
            .auto_commit(true)
            .build();

        assert_eq!(
            config.topics,
            Some(vec!["orders".to_string(), "returns".to_string()])
        );
        assert_eq!(config.group_id.as_deref(), Some("svc"));
        assert_eq!(config.auto_offset_reset, Some(AutoOffsetReset::Latest));
        assert_eq!(config.auto_commit, Some(true));
        assert_eq!(config.connection.brokers, Some(vec!["a:9092".to_string()]));
        assert_eq!(
            config.connection.security_protocol,
            Some(SecurityProtocol::Ssl)
        );
    }
}
