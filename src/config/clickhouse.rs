use crate::database::clickhouse::Compression;

#[derive(Debug, Clone, Default)]
pub struct ClickHouseConfig {
    pub hosts: Option<Vec<String>>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub secure: Option<bool>,
    pub max_pool_size: Option<u32>,
    pub connection_timeout_ms: Option<u64>,
    pub request_timeout_ms: Option<u64>,
    pub compression: Option<Compression>,
    pub allow_select_star: Option<bool>,
}

impl ClickHouseConfig {
    pub fn builder() -> ClickHouseConfigBuilder {
        ClickHouseConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClickHouseConfigBuilder {
    hosts: Option<Vec<String>>,
    port: Option<u16>,
    database: Option<String>,
    username: Option<String>,
    password: Option<String>,
    secure: Option<bool>,
    max_pool_size: Option<u32>,
    connection_timeout_ms: Option<u64>,
    request_timeout_ms: Option<u64>,
    compression: Option<Compression>,
    allow_select_star: Option<bool>,
}

impl ClickHouseConfigBuilder {
    pub fn host(mut self, host: impl Into<String>) -> Self {
        let host = host.into();
        match self.hosts.as_mut() {
            Some(existing) => existing.push(host),
            None => self.hosts = Some(vec![host]),
        }
        self
    }

    pub fn hosts<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.hosts = Some(hosts.into_iter().map(Into::into).collect());
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    pub fn username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = Some(secure);
        self
    }

    pub fn max_pool_size(mut self, max_pool_size: u32) -> Self {
        self.max_pool_size = Some(max_pool_size);
        self
    }

    pub fn connection_timeout_ms(mut self, ms: u64) -> Self {
        self.connection_timeout_ms = Some(ms);
        self
    }

    pub fn request_timeout_ms(mut self, ms: u64) -> Self {
        self.request_timeout_ms = Some(ms);
        self
    }

    pub fn compression(mut self, compression: Compression) -> Self {
        self.compression = Some(compression);
        self
    }

    pub fn allow_select_star(mut self, allow: bool) -> Self {
        self.allow_select_star = Some(allow);
        self
    }

    pub fn build(self) -> ClickHouseConfig {
        ClickHouseConfig {
            hosts: self.hosts,
            port: self.port,
            database: self.database,
            username: self.username,
            password: self.password,
            secure: self.secure,
            max_pool_size: self.max_pool_size,
            connection_timeout_ms: self.connection_timeout_ms,
            request_timeout_ms: self.request_timeout_ms,
            compression: self.compression,
            allow_select_star: self.allow_select_star,
        }
    }
}
