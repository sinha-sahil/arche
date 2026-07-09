use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::AppError;

use super::reject_token;

const DEFAULT_TTL: Duration = Duration::from_secs(3600);
const MAX_TTL_SECS: u64 = 86_400;
const STALE_GRACE_MULTIPLIER: u32 = 2;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Jwk {
    pub(super) kid: String,
    pub(super) n: String,
    pub(super) e: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Default)]
struct State {
    keys: HashMap<String, Jwk>,
    expires_at: Option<Instant>,
    ttl: Duration,
}

impl State {
    fn is_fresh(&self) -> bool {
        self.expires_at
            .map(|exp| exp > Instant::now())
            .unwrap_or(false)
    }

    fn within_stale_grace(&self) -> bool {
        let Some(exp) = self.expires_at else {
            return false;
        };
        match exp.checked_add(self.ttl * STALE_GRACE_MULTIPLIER) {
            Some(deadline) => Instant::now() < deadline,
            None => true,
        }
    }
}

#[derive(Clone)]
pub(super) struct JwksCache {
    endpoint: String,
    dependency: String,
    state: Arc<RwLock<State>>,
}

impl JwksCache {
    pub(super) fn new(endpoint: String, dependency: String) -> Self {
        Self {
            endpoint,
            dependency,
            state: Arc::new(RwLock::new(State::default())),
        }
    }

    pub(super) async fn lookup(&self, http: &reqwest::Client, kid: &str) -> Result<Jwk, AppError> {
        if let Some(jwk) = self.lookup_if_fresh(kid).await {
            return Ok(jwk);
        }

        let mut state = self.state.write().await;
        if state.is_fresh()
            && let Some(jwk) = state.keys.get(kid).cloned()
        {
            return Ok(jwk);
        }

        match fetch(http, &self.endpoint, &self.dependency).await {
            Ok((keys, ttl)) => {
                state.keys = keys.into_iter().map(|k| (k.kid.clone(), k)).collect();
                state.expires_at = Instant::now().checked_add(ttl);
                state.ttl = ttl;
                state
                    .keys
                    .get(kid)
                    .cloned()
                    .ok_or_else(|| reject_token(format!("unknown signing key: {kid}")))
            }
            Err(fetch_err) => {
                if state.within_stale_grace()
                    && let Some(jwk) = state.keys.get(kid).cloned()
                {
                    tracing::warn!(kid = %kid, "serving stale JWKS key while upstream unreachable");
                    return Ok(jwk);
                }
                Err(fetch_err)
            }
        }
    }

    async fn lookup_if_fresh(&self, kid: &str) -> Option<Jwk> {
        let state = self.state.read().await;
        if state.is_fresh() {
            state.keys.get(kid).cloned()
        } else {
            None
        }
    }
}

async fn fetch(
    http: &reqwest::Client,
    endpoint: &str,
    dependency: &str,
) -> Result<(Vec<Jwk>, Duration), AppError> {
    let resp = http
        .get(endpoint)
        .send()
        .await
        .map_err(|e| AppError::dependency_failed(dependency, format!("jwks fetch: {e}")))?;
    let status = resp.status();
    let ttl = parse_cache_control_max_age(resp.headers()).unwrap_or(DEFAULT_TTL);
    if !status.is_success() {
        return Err(AppError::dependency_failed(
            dependency,
            format!("jwks endpoint returned {status}"),
        ));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| AppError::dependency_failed(dependency, format!("jwks body: {e}")))?;
    let parsed: Jwks = serde_json::from_slice(&body).map_err(|e| {
        AppError::dependency_failed_permanent(dependency, format!("jwks parse: {e}"))
    })?;
    Ok((parsed.keys, ttl))
}

fn parse_cache_control_max_age(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::CACHE_CONTROL)?.to_str().ok()?;
    for directive in value.split(',') {
        let directive = directive.trim();
        if let Some(rest) = directive
            .strip_prefix("max-age=")
            .or_else(|| directive.strip_prefix("max-age ="))
        {
            return rest
                .parse::<u64>()
                .ok()
                .map(|s| Duration::from_secs(s.min(MAX_TTL_SECS)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(value: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::CACHE_CONTROL, value.parse().unwrap());
        h
    }

    #[test]
    fn cache_control_max_age_parsing() {
        assert_eq!(
            parse_cache_control_max_age(&headers_with("public, max-age=21600, must-revalidate")),
            Some(Duration::from_secs(21600))
        );
        assert!(parse_cache_control_max_age(&reqwest::header::HeaderMap::new()).is_none());
        assert!(parse_cache_control_max_age(&headers_with("max-age=oops")).is_none());
    }
}
