use serde::de::DeserializeOwned;

use crate::error::AppError;

use super::provider::LlmProvider;
use super::types::{ContentPart, GenerateRequest, Message, ToolDefinition, Usage};

/// One-shot text completion at temperature 0 — no tools, no session.
pub async fn one_shot(
    provider: &dyn LlmProvider,
    model: &str,
    system: &str,
    prompt: &str,
) -> Result<String, AppError> {
    one_shot_with_usage(provider, model, system, prompt)
        .await
        .map(|(text, _)| text)
}

/// [`one_shot`] that also returns the provider's token usage when reported.
pub async fn one_shot_with_usage(
    provider: &dyn LlmProvider,
    model: &str,
    system: &str,
    prompt: &str,
) -> Result<(String, Option<Usage>), AppError> {
    let req = GenerateRequest::new(model, vec![Message::user(prompt)])
        .with_system(system)
        .with_temperature(0.0);

    let resp = provider.generate(&req).await?;

    let text = resp
        .content
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    if text.is_empty() {
        return Err(AppError::dependency_failed("llm", "model returned no text"));
    }
    Ok((text, resp.usage))
}

/// Forces a single call to `tool` (whose parameters describe `T`) and deserializes its arguments into `T`.
pub async fn one_shot_tool<T: DeserializeOwned>(
    provider: &dyn LlmProvider,
    model: &str,
    system: &str,
    prompt: &str,
    tool: ToolDefinition,
) -> Result<T, AppError> {
    one_shot_tool_with_usage(provider, model, system, prompt, tool)
        .await
        .map(|(value, _)| value)
}

/// [`one_shot_tool`] that also returns the provider's token usage when reported.
pub async fn one_shot_tool_with_usage<T: DeserializeOwned>(
    provider: &dyn LlmProvider,
    model: &str,
    system: &str,
    prompt: &str,
    tool: ToolDefinition,
) -> Result<(T, Option<Usage>), AppError> {
    let req = GenerateRequest::new(model, vec![Message::user(prompt)])
        .with_system(system)
        .with_temperature(0.0)
        .with_tools(vec![tool]);

    let resp = provider.generate(&req).await?;

    for part in &resp.content {
        if let ContentPart::ToolCall { arguments, .. } = part {
            return serde_json::from_value::<T>(arguments.clone())
                .map(|value| (value, resp.usage.clone()))
                .map_err(|e| AppError::dependency_failed("llm", format!("bad tool args: {e}")));
        }
    }

    Err(AppError::dependency_failed(
        "llm",
        "model returned no tool call",
    ))
}
