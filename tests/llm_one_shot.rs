use std::pin::Pin;

use arche::error::AppError;
use arche::llm::{
    ContentPart, GenerateRequest, GenerateResponse, LlmProvider, LlmStream, ToolDefinition, Usage,
    one_shot, one_shot_tool, one_shot_tool_with_usage, one_shot_with_usage,
};

struct StubProvider {
    resp: GenerateResponse,
}

impl LlmProvider for StubProvider {
    fn generate<'a>(
        &'a self,
        _request: &'a GenerateRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<GenerateResponse, AppError>> + Send + 'a>>
    {
        let resp = self.resp.clone();
        Box::pin(async move { Ok(resp) })
    }

    fn stream_generate<'a>(
        &'a self,
        _request: &'a GenerateRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<LlmStream, AppError>> + Send + 'a>> {
        Box::pin(async move { Err(AppError::internal_error("stub has no stream".into(), None)) })
    }
}

fn stub(content: Vec<ContentPart>, usage: Option<Usage>) -> StubProvider {
    StubProvider {
        resp: GenerateResponse {
            content,
            stop_reason: Some("STOP".into()),
            usage,
        },
    }
}

fn usage() -> Usage {
    Usage {
        input_tokens: Some(10),
        output_tokens: Some(5),
        total_tokens: Some(15),
    }
}

#[tokio::test]
async fn one_shot_with_usage_returns_text_and_usage() {
    let provider = stub(vec![ContentPart::Text("hello".into())], Some(usage()));
    let (text, usage) = one_shot_with_usage(&provider, "m", "sys", "hi")
        .await
        .unwrap();
    assert_eq!(text, "hello");
    assert_eq!(usage.unwrap().total_tokens, Some(15));
}

#[tokio::test]
async fn one_shot_errors_on_empty_text() {
    let provider = stub(vec![], None);
    let err = one_shot(&provider, "m", "sys", "hi").await.unwrap_err();
    assert!(err.is_dependency_error());
}

#[tokio::test]
async fn one_shot_tool_with_usage_deserializes_args_and_returns_usage() {
    #[derive(serde::Deserialize)]
    struct Out {
        answer: String,
    }
    let provider = stub(
        vec![ContentPart::ToolCall {
            id: "t1".into(),
            name: "report".into(),
            arguments: serde_json::json!({"answer": "42"}),
            thought_signature: None,
        }],
        Some(usage()),
    );
    let (out, usage) = one_shot_tool_with_usage::<Out>(
        &provider,
        "m",
        "sys",
        "hi",
        ToolDefinition::new("report", "reports"),
    )
    .await
    .unwrap();
    assert_eq!(out.answer, "42");
    assert_eq!(usage.unwrap().input_tokens, Some(10));
}

#[tokio::test]
async fn one_shot_tool_errors_when_no_tool_call() {
    let provider = stub(vec![ContentPart::Text("prose instead".into())], None);
    let err = one_shot_tool::<serde_json::Value>(
        &provider,
        "m",
        "sys",
        "hi",
        ToolDefinition::new("report", "reports"),
    )
    .await
    .unwrap_err();
    assert!(err.is_dependency_error());
}

#[tokio::test]
async fn one_shot_tool_errors_on_mismatched_args() {
    #[derive(Debug, serde::Deserialize)]
    struct Out {
        #[allow(dead_code)]
        answer: u32,
    }
    let provider = stub(
        vec![ContentPart::ToolCall {
            id: "t1".into(),
            name: "report".into(),
            arguments: serde_json::json!({"answer": "not a number"}),
            thought_signature: None,
        }],
        None,
    );
    let err = one_shot_tool::<Out>(
        &provider,
        "m",
        "sys",
        "hi",
        ToolDefinition::new("report", "reports"),
    )
    .await
    .unwrap_err();
    assert!(err.is_dependency_error());
}
