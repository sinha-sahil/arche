use crate::error::AppError;
use crate::llm::{GenerateRequest, GenerateResponse, LlmProvider, LlmStream};
use std::pin::Pin;

use super::config::ResolvedAuth;
use super::providers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexProvider {
    Gemini,
    Anthropic,
}

pub struct VertexClient {
    pub(crate) http: reqwest::Client,
    pub(crate) auth: ResolvedAuth,
    pub(crate) provider: VertexProvider,
}

impl VertexClient {
    pub(crate) fn new(http: reqwest::Client, auth: ResolvedAuth, provider: VertexProvider) -> Self {
        Self {
            http,
            auth,
            provider,
        }
    }

    pub(crate) async fn authorize(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, AppError> {
        match &self.auth {
            ResolvedAuth::ApiKey { api_key } => Ok(req.header("x-goog-api-key", api_key.as_str())),
            ResolvedAuth::ServiceAccount { token_source, .. } => {
                let bearer = token_source
                    .access_token(&["https://www.googleapis.com/auth/cloud-platform"])
                    .await?;
                Ok(req.header("Authorization", format!("Bearer {bearer}")))
            }
        }
    }

    pub(crate) async fn send(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, AppError> {
        let resp = req.send().await.map_err(|e| {
            AppError::dependency_failed("vertex-ai", format!("Request failed: {e}"))
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::dependency_failed(
                "vertex-ai",
                format!("API error ({status}): {body}"),
            ));
        }

        Ok(resp)
    }
}

impl LlmProvider for VertexClient {
    fn generate<'a>(
        &'a self,
        request: &'a GenerateRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<GenerateResponse, AppError>> + Send + 'a>>
    {
        Box::pin(async move {
            match self.provider {
                VertexProvider::Gemini => providers::gemini::generate(self, request).await,
                VertexProvider::Anthropic => providers::anthropic::generate(self, request).await,
            }
        })
    }

    fn stream_generate<'a>(
        &'a self,
        request: &'a GenerateRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<LlmStream, AppError>> + Send + 'a>> {
        Box::pin(async move {
            match self.provider {
                VertexProvider::Gemini => providers::gemini::stream_generate(self, request).await,
                VertexProvider::Anthropic => {
                    providers::anthropic::stream_generate(self, request).await
                }
            }
        })
    }
}
