use std::future::Future;
use std::time::Duration;

use crate::error::AppError;

use super::types::PendingGrant;

pub trait RefreshTokenStore: Send + Sync + 'static {
    fn put(
        &self,
        token: String,
        grant: PendingGrant,
        ttl: Duration,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    fn take(
        &self,
        token: &str,
    ) -> impl Future<Output = Result<Option<PendingGrant>, AppError>> + Send;
}
