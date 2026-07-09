use std::future::Future;

use crate::error::AppError;

use super::types::PendingGrant;

pub struct IssuedAccessToken {
    pub token: String,
    pub expires_in: u64,
}

impl std::fmt::Debug for IssuedAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedAccessToken")
            .field("token", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

pub trait AccessTokenIssuer: Send + Sync + 'static {
    fn issue(
        &self,
        grant: &PendingGrant,
    ) -> impl Future<Output = Result<IssuedAccessToken, AppError>> + Send;
}
