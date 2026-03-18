use bb8::{Pool, PooledConnection};
use bb8_redis::RedisConnectionManager;
use bb8_redis::redis::AsyncCommands;

use crate::config::{resolve_optional_string, resolve_required, resolve_required_string};
use crate::error::AppError;

pub use crate::config::redis::{RedisConfig, RedisConfigBuilder};

pub type RedisPool = bb8::Pool<bb8_redis::RedisConnectionManager>;

#[allow(dead_code)]
pub async fn get_redis_pool(
    config: impl Into<Option<RedisConfig>>,
) -> Result<Pool<RedisConnectionManager>, AppError> {
    let config = config.into().unwrap_or_default();

    let host = resolve_required_string(config.host, "REDIS_HOST", "host")?;
    let port: u16 = resolve_required(config.port, "REDIS_PORT", "port")?;
    let max_conn: u32 =
        resolve_required(config.max_connections, "REDIS_MAX_CONN", "max_connections")?;
    let password = resolve_optional_string(config.password, "REDIS_PASSWORD");

    let redis_url = if let Some(pwd) = password {
        format!("redis://:{}@{}:{}", pwd, host, port)
    } else {
        format!("redis://{}:{}", host, port)
    };

    let redis_conn_manager = bb8_redis::RedisConnectionManager::new(redis_url).map_err(|e| {
        AppError::internal_error(
            format!("Failed to create Redis connection manager: {}", e),
            None,
        )
    })?;

    bb8::Pool::builder()
        .max_size(max_conn)
        .build(redis_conn_manager)
        .await
        .map_err(|e| AppError::internal_error(format!("Failed to create Redis pool: {}", e), None))
}

pub async fn _get_redis_conn(
    redis_pool: &Pool<RedisConnectionManager>,
) -> Result<PooledConnection<'_, RedisConnectionManager>, AppError> {
    redis_pool.get().await.map_err(|e| {
        AppError::internal_error(
            e.to_string(),
            Some("Failed to get Redis connection from pool".to_string()),
        )
    })
}

pub async fn test_redis(
    redis_pool: bb8::Pool<bb8_redis::RedisConnectionManager>,
) -> Result<bool, AppError> {
    let mut redis_conn: PooledConnection<RedisConnectionManager> =
        redis_pool.get().await.map_err(|e| {
            AppError::internal_error(
                e.to_string(),
                Some("Failed to get Redis connection from pool".to_string()),
            )
        })?;

    redis_conn
        .set::<&str, &str, ()>("test-key", "test-value")
        .await
        .map_err(|e| {
            AppError::internal_error(e.to_string(), Some("Failed to set test key".to_string()))
        })?;

    let value: String = redis_conn.get("test-key").await.map_err(|e| {
        AppError::internal_error(e.to_string(), Some("Failed to get test key".to_string()))
    })?;

    let _: () = redis_conn.del("test-key").await.map_err(|e| {
        AppError::internal_error(e.to_string(), Some("Failed to delete test key".to_string()))
    })?;

    Ok(value == "test-value")
}
