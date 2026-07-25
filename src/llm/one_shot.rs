use crate::error::AppError;

use super::provider::LlmProvider;
use super::types::{GenerateRequest, GenerateResponse};

pub async fn one_shot(
    provider: &dyn LlmProvider,
    request: &GenerateRequest,
) -> Result<GenerateResponse, AppError> {
    provider.generate(request).await
}
