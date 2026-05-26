use std::str::FromStr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bb8::{ManageConnection, Pool, PooledConnection};
use clickhouse::error::Error as ChError;
use clickhouse::query::Query as ChQuery;
use clickhouse::{Client as ChClient, Compression as ChCompression};
use indexmap::IndexMap;
use regex::Regex;

use crate::config::{resolve_optional_string, resolve_with_default};
use crate::error::AppError;

pub use crate::config::clickhouse::{ClickHouseConfig, ClickHouseConfigBuilder};
pub use ::clickhouse::Row;
pub use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    #[default]
    None,
    Lz4,
}

impl FromStr for Compression {
    type Err = AppError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "off" | "false" => Ok(Self::None),
            "lz4" | "on" | "true" => Ok(Self::Lz4),
            other => Err(AppError::internal_error(
                format!("Unknown CLICKHOUSE_COMPRESSION: {other}"),
                None,
            )),
        }
    }
}

fn map_wire_error(op: &'static str, err: ChError) -> AppError {
    match err {
        ChError::Network(b) => {
            let msg = b.to_string();
            let lower = msg.to_ascii_lowercase();
            let retryable =
                !(lower.contains("decode") || lower.contains("body") || lower.contains("tls"));
            AppError::DependencyFailed {
                upstream: "clickhouse".into(),
                detail: format!("{op}: network: {msg}"),
                retryable,
            }
        }
        ChError::TimedOut => AppError::DependencyFailed {
            upstream: "clickhouse".into(),
            detail: format!("{op}: timeout"),
            retryable: true,
        },
        ChError::BadResponse(body) if body.starts_with("Code: ") => classify_ch(op, body),
        ChError::BadResponse(body) => AppError::DependencyFailed {
            upstream: "clickhouse".into(),
            detail: format!("{op}: bad response: {body}"),
            retryable: true,
        },
        ChError::RowNotFound => AppError::NotFound {
            resource: "clickhouse:row".into(),
        },
        ChError::InvalidParams(b) => {
            AppError::internal_error(format!("{op}: invalid params: {b}"), None)
        }
        ChError::Unsupported(m) => {
            AppError::internal_error(format!("{op}: clickhouse unsupported: {m}"), None)
        }
        ChError::NotEnoughData
        | ChError::SequenceMustHaveLength
        | ChError::DeserializeAnyNotSupported
        | ChError::InvalidTagEncoding(_)
        | ChError::VariantDiscriminatorIsOutOfBound(_)
        | ChError::InvalidUtf8Encoding(_) => AppError::internal_error(
            format!("{op}: row decode: {err:?}"),
            Some("ClickHouse row decode failed".into()),
        ),
        other => {
            tracing::warn!(op = op, error = %other, "Unmapped ChError variant");
            AppError::DependencyFailed {
                upstream: "clickhouse".into(),
                detail: format!("{op}: {other}"),
                retryable: false,
            }
        }
    }
}

fn classify_ch(op: &'static str, body: String) -> AppError {
    let code = body
        .strip_prefix("Code: ")
        .and_then(|s| s.split_once('.'))
        .and_then(|(n, _)| n.trim().parse::<u32>().ok())
        .unwrap_or(0);
    match code {
        62 | 60 | 47 | 53 | 41 | 27 | 117 => AppError::BadRequest {
            error_values: None,
            message: Some(format!("ClickHouse: {body}")),
            description: Some(op.to_string()),
        },
        516 | 192 | 193 => AppError::Unauthorized,
        497 => AppError::Forbidden,
        81 => AppError::NotFound {
            resource: format!("clickhouse:{body}"),
        },
        252 | 241 | 160 | 209 | 210 => AppError::DependencyFailed {
            upstream: "clickhouse".into(),
            detail: format!("{op}: {body}"),
            retryable: true,
        },
        _ => AppError::DependencyFailed {
            upstream: "clickhouse".into(),
            detail: format!("{op}: {body}"),
            retryable: false,
        },
    }
}

pub type ClickHousePool = Pool<ClickHouseConnectionManager>;

pub struct ClickHouseConnectionManager {
    base_client: ChClient,
    urls: Vec<String>,
    rr: AtomicUsize,
    allow_select_star: bool,
}

pub struct PooledClient {
    client: ChClient,
    endpoint: String,
    allow_select_star_pool_default: bool,
}

impl ManageConnection for ClickHouseConnectionManager {
    type Connection = PooledClient;
    type Error = AppError;

    async fn connect(&self) -> Result<Self::Connection, AppError> {
        if self.urls.is_empty() {
            return Err(AppError::Unavailable);
        }
        let idx = self.rr.fetch_add(1, Ordering::Relaxed) % self.urls.len();
        let endpoint = self.urls[idx].clone();
        // Cloning the base client shares its Arc<dyn HttpClient>, so every
        // slot funnels through one underlying hyper connection pool.
        let client = self.base_client.clone().with_url(endpoint.clone());
        client
            .query("SELECT 1")
            .fetch_one::<u8>()
            .await
            .map_err(|e| map_wire_error("connect probe", e))?;
        Ok(PooledClient {
            client,
            endpoint,
            allow_select_star_pool_default: self.allow_select_star,
        })
    }

    async fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), AppError> {
        conn.client
            .query("SELECT 1")
            .fetch_one::<u8>()
            .await
            .map_err(|e| map_wire_error("is_valid", e))?;
        Ok(())
    }

    fn has_broken(&self, _: &mut Self::Connection) -> bool {
        false
    }
}

pub async fn get_clickhouse_pool(
    config: impl Into<Option<ClickHouseConfig>>,
) -> Result<ClickHousePool, AppError> {
    let resolved = resolve_pool_config(config.into().unwrap_or_default())?;
    let manager = ClickHouseConnectionManager {
        base_client: build_base_client(&resolved),
        urls: resolved.urls,
        rr: AtomicUsize::new(0),
        allow_select_star: resolved.allow_select_star,
    };
    Pool::builder()
        .max_size(resolved.max_pool)
        .connection_timeout(resolved.connection_timeout)
        .test_on_check_out(false)
        .build(manager)
        .await
        .map_err(|e| {
            AppError::internal_error(
                format!("Failed to build ClickHouse pool: {e:?}"),
                Some("ClickHouse pool build failed".into()),
            )
        })
}

pub async fn test_clickhouse(pool: ClickHousePool) -> Result<bool, AppError> {
    let conn = pool.get().await.map_err(|e| match e {
        bb8::RunError::TimedOut => AppError::Unavailable,
        bb8::RunError::User(err) => err,
    })?;
    let v = conn
        .client
        .query("SELECT 1")
        .fetch_one::<u8>()
        .await
        .map_err(|e| map_wire_error("test_clickhouse", e))?;
    Ok(v == 1)
}

struct ResolvedPoolConfig {
    urls: Vec<String>,
    username: String,
    password: String,
    database: String,
    compression: Compression,
    request_timeout: Duration,
    connection_timeout: Duration,
    max_pool: u32,
    allow_select_star: bool,
}

fn resolve_pool_config(cfg: ClickHouseConfig) -> Result<ResolvedPoolConfig, AppError> {
    let secure = resolve_with_default(cfg.secure, "CLICKHOUSE_SECURE", true);
    let port: u16 = resolve_with_default(
        cfg.port,
        "CLICKHOUSE_PORT",
        if secure { 8443 } else { 8123 },
    );
    let hosts = resolve_hosts(cfg.hosts)?;
    let scheme = if secure { "https" } else { "http" };
    let urls: Vec<String> = hosts
        .iter()
        .map(|h| {
            if h.starts_with("http://") || h.starts_with("https://") {
                h.clone()
            } else {
                format!("{scheme}://{h}:{port}")
            }
        })
        .collect();

    Ok(ResolvedPoolConfig {
        urls,
        username: resolve_with_default(cfg.username, "CLICKHOUSE_USERNAME", "default".to_string()),
        password: resolve_optional_string(cfg.password, "CLICKHOUSE_PASSWORD").unwrap_or_default(),
        database: resolve_with_default(cfg.database, "CLICKHOUSE_DATABASE", "default".to_string()),
        compression: resolve_with_default(
            cfg.compression,
            "CLICKHOUSE_COMPRESSION",
            Compression::None,
        ),
        request_timeout: Duration::from_millis(resolve_with_default(
            cfg.request_timeout_ms,
            "CLICKHOUSE_REQUEST_TIMEOUT_MS",
            30_000u64,
        )),
        connection_timeout: Duration::from_millis(resolve_with_default(
            cfg.connection_timeout_ms,
            "CLICKHOUSE_CONNECTION_TIMEOUT_MS",
            5_000u64,
        )),
        max_pool: resolve_with_default(cfg.max_pool_size, "CLICKHOUSE_MAX_POOL_SIZE", 32u32),
        allow_select_star: resolve_with_default(
            cfg.allow_select_star,
            "CLICKHOUSE_ALLOW_SELECT_STAR",
            false,
        ),
    })
}

fn build_base_client(r: &ResolvedPoolConfig) -> ChClient {
    // `max_execution_time` surfaces request_timeout_ms via the server —
    // the clickhouse crate doesn't expose hyper-level timeouts.
    let mut base = ChClient::default()
        .with_user(&r.username)
        .with_password(&r.password)
        .with_database(&r.database)
        .with_product_info("arche", env!("CARGO_PKG_VERSION"))
        .with_option(
            "max_execution_time",
            r.request_timeout.as_secs().max(1).to_string(),
        );
    if matches!(r.compression, Compression::Lz4) {
        base = base.with_compression(ChCompression::Lz4);
    }
    base
}

fn resolve_hosts(cfg_hosts: Option<Vec<String>>) -> Result<Vec<String>, AppError> {
    if let Some(h) = cfg_hosts
        && !h.is_empty()
    {
        return Ok(h);
    }
    if let Ok(v) = std::env::var("CLICKHOUSE_HOSTS") {
        let parsed: Vec<String> = v
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !parsed.is_empty() {
            return Ok(parsed);
        }
    }
    if let Ok(v) = std::env::var("CLICKHOUSE_HOST") {
        let t = v.trim();
        if !t.is_empty() {
            return Ok(vec![t.to_string()]);
        }
    }
    Err(AppError::internal_error(
        "Config error [hosts/CLICKHOUSE_HOSTS]: Value not provided and environment variable not set"
            .to_string(),
        None,
    ))
}

pub struct ClickHouseConnection<'a> {
    inner: PooledConnection<'a, ClickHouseConnectionManager>,
    settings: IndexMap<String, String>,
}

impl<'a> ClickHouseConnection<'a> {
    pub fn with_setting(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.settings.insert(key.into(), value.into());
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }

    pub fn query(&'a self, sql: &'static str) -> QueryBuilder<'a> {
        self.query_builder(sql.to_string())
    }

    pub fn query_dynamic(&'a self, sql: impl Into<String>) -> QueryBuilder<'a> {
        self.query_builder(sql.into())
    }

    fn query_builder(&'a self, sql: String) -> QueryBuilder<'a> {
        QueryBuilder {
            conn: self,
            sql,
            binds: Vec::new(),
            settings: IndexMap::new(),
            allow_select_star: false,
        }
    }

    pub async fn execute(&self, sql: &'static str) -> Result<(), AppError> {
        self.execute_inner(sql).await
    }

    pub async fn execute_dynamic(&self, sql: impl Into<String>) -> Result<(), AppError> {
        let sql = sql.into();
        self.execute_inner(&sql).await
    }

    async fn execute_inner(&self, sql: &str) -> Result<(), AppError> {
        check_select_star_guard(sql, false, self.inner.allow_select_star_pool_default)?;
        let mut q = self.inner.client.clone().query(sql);
        for (k, v) in &self.settings {
            q = q.with_option(k, v);
        }
        q.execute().await.map_err(|e| map_wire_error("execute", e))
    }
}

pub trait ClickHousePoolExt {
    fn get_conn(
        &self,
    ) -> impl std::future::Future<Output = Result<ClickHouseConnection<'_>, AppError>> + Send;
}

impl ClickHousePoolExt for ClickHousePool {
    async fn get_conn(&self) -> Result<ClickHouseConnection<'_>, AppError> {
        let inner = self.get().await.map_err(|e| match e {
            bb8::RunError::TimedOut => AppError::Unavailable,
            bb8::RunError::User(err) => err,
        })?;
        Ok(ClickHouseConnection {
            inner,
            settings: IndexMap::new(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum BindValue {
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
}

impl BindValue {
    fn apply(self, q: ChQuery) -> ChQuery {
        match self {
            BindValue::I64(v) => q.bind(v),
            BindValue::U64(v) => q.bind(v),
            BindValue::F64(v) => q.bind(v),
            BindValue::Bool(v) => q.bind(v),
            BindValue::Str(v) => q.bind(v),
            BindValue::Bytes(v) => q.bind(v),
        }
    }
}

impl From<i64> for BindValue {
    fn from(v: i64) -> Self {
        Self::I64(v)
    }
}
impl From<u64> for BindValue {
    fn from(v: u64) -> Self {
        Self::U64(v)
    }
}
impl From<f64> for BindValue {
    fn from(v: f64) -> Self {
        Self::F64(v)
    }
}
impl From<bool> for BindValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<String> for BindValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}
impl From<&str> for BindValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_string())
    }
}
impl From<Vec<u8>> for BindValue {
    fn from(v: Vec<u8>) -> Self {
        Self::Bytes(v)
    }
}

pub struct QueryBuilder<'a> {
    conn: &'a ClickHouseConnection<'a>,
    sql: String,
    binds: Vec<BindValue>,
    settings: IndexMap<String, String>,
    allow_select_star: bool,
}

impl<'a> QueryBuilder<'a> {
    pub fn bind<V: Into<BindValue>>(mut self, value: V) -> Self {
        self.binds.push(value.into());
        self
    }

    pub fn with_setting(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.settings.insert(key.into(), value.into());
        self
    }

    pub fn allow_select_star(mut self) -> Self {
        self.allow_select_star = true;
        self
    }

    pub async fn fetch_one<T>(self) -> Result<T, AppError>
    where
        T: Row + for<'de> Deserialize<'de> + Send + 'static,
    {
        let mut rows: Vec<T> = self.fetch_all::<T>().await?;
        match rows.len() {
            0 => Err(AppError::NotFound {
                resource: "clickhouse:row".into(),
            }),
            1 => Ok(rows.swap_remove(0)),
            n => Err(AppError::Conflict {
                message: format!("expected 1 row, got {n}"),
            }),
        }
    }

    pub async fn fetch_optional<T>(self) -> Result<Option<T>, AppError>
    where
        T: Row + for<'de> Deserialize<'de> + Send + 'static,
    {
        let mut rows: Vec<T> = self.fetch_all::<T>().await?;
        match rows.len() {
            0 => Ok(None),
            1 => Ok(Some(rows.swap_remove(0))),
            n => Err(AppError::Conflict {
                message: format!("expected at most 1 row, got {n}"),
            }),
        }
    }

    pub async fn fetch_all<T>(self) -> Result<Vec<T>, AppError>
    where
        T: Row + for<'de> Deserialize<'de> + Send + 'static,
    {
        check_select_star_guard(
            &self.sql,
            self.allow_select_star,
            self.conn.inner.allow_select_star_pool_default,
        )?;
        let mut q = self.conn.inner.client.clone().query(&self.sql);
        // Per-query settings override per-connection on collision.
        for (k, v) in self.conn.settings.iter().chain(self.settings.iter()) {
            q = q.with_option(k, v);
        }
        for b in self.binds {
            q = b.apply(q);
        }
        q.fetch_all::<T>()
            .await
            .map_err(|e| map_wire_error("fetch_all", e))
    }
}

// ClickHouse is column-oriented; `SELECT *` reads every column from disk —
// a perf disaster on wide event tables. Reject the pattern at build time
// via a single regex against the raw SQL. False positives in comments /
// string literals / subqueries are acceptable — the three escape hatches
// (per-query, per-pool, `CLICKHOUSE_ALLOW_SELECT_STAR=true`) are cheap to
// reach for. The `regex` crate dep exists ONLY for this guard.
fn check_select_star_guard(
    sql: &str,
    per_query_allow: bool,
    pool_default_allow: bool,
) -> Result<(), AppError> {
    if per_query_allow || pool_default_allow || env_allows_select_star() {
        return Ok(());
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)\bSELECT\b\s+(?:DISTINCT\s+)?\*|\b\w+\.\*")
            .expect("hardcoded SELECT-* regex pattern must compile")
    });
    if !re.is_match(sql) {
        return Ok(());
    }
    Err(AppError::BadRequest {
        error_values: None,
        message: Some(
            "SELECT * is disallowed. Use .allow_select_star() (per-query), \
             .allow_select_star(true) on ClickHouseConfigBuilder (per-pool), \
             or CLICKHOUSE_ALLOW_SELECT_STAR=true (global)."
                .to_string(),
        ),
        description: Some(format!("offending SQL: {}", safe_truncate(sql, 200))),
    })
}

fn env_allows_select_star() -> bool {
    std::env::var("CLICKHOUSE_ALLOW_SELECT_STAR")
        .ok()
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(false)
}

fn safe_truncate(s: &str, max: usize) -> &str {
    s.char_indices().nth(max).map_or(s, |(i, _)| &s[..i])
}

pub async fn with_one_retry<F, Fut, T>(
    pool: &ClickHousePool,
    op_name: &'static str,
    op: F,
) -> Result<T, AppError>
where
    F: Fn(ClickHouseConnection<'_>) -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    let c1 = pool.get_conn().await?;
    let ep = c1.endpoint().to_string();
    match op(c1).await {
        Ok(v) => Ok(v),
        Err(AppError::DependencyFailed {
            retryable: true, ..
        }) => {
            tracing::info!(
                op = op_name,
                endpoint = %ep,
                "ClickHouse op failed, retrying once on a fresh checkout"
            );
            op(pool.get_conn().await?).await
        }
        Err(e) => Err(e),
    }
}
