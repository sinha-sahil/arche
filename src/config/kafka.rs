use std::collections::HashMap;

use crate::queue::kafka::AutoOffsetReset;

#[derive(Debug, Clone, Default)]
pub struct KafkaConnectionConfig {
    pub brokers: Option<Vec<String>>,
    pub socket_timeout_ms: Option<u64>,
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

    pub fn build(self) -> KafkaConnectionConfig {
        KafkaConnectionConfig {
            brokers: self.brokers,
            socket_timeout_ms: self.socket_timeout_ms,
            extra_options: self.extra_options,
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
