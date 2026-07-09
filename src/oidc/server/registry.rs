use std::future::Future;

use crate::error::AppError;

use super::types::ClientRegistration;

pub trait ClientRegistry: Send + Sync + 'static {
    fn find(
        &self,
        client_id: &str,
    ) -> impl Future<Output = Result<Option<ClientRegistration>, AppError>> + Send;

    fn verify_secret(
        &self,
        client_id: &str,
        presented: &str,
    ) -> impl Future<Output = Result<bool, AppError>> + Send {
        async move {
            match self.find(client_id).await? {
                Some(c) => Ok(super::ct_eq(
                    c.client_secret.as_bytes(),
                    presented.as_bytes(),
                )),
                None => {
                    super::ct_eq(b"unknown-client-timing-equalizer", presented.as_bytes());
                    Ok(false)
                }
            }
        }
    }
}
