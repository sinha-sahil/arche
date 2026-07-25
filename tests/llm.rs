use std::pin::Pin;

use arche::error::AppError;
use arche::llm::{
    ContentPart, GenerateRequest, GenerateResponse, LlmProvider, LlmStream, ParameterSchema, Role,
    ToolDefinition, Usage, one_shot,
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

fn tool_call(name: &str, args: serde_json::Value) -> ContentPart {
    ContentPart::ToolCall {
        id: format!("{name}-1"),
        name: name.to_string(),
        arguments: args,
        thought_signature: None,
    }
}

fn report_tool() -> ToolDefinition {
    ToolDefinition::new("report", "reports").with_parameters(
        ParameterSchema::object().with_property("answer", ParameterSchema::string("the answer")),
    )
}

#[derive(Debug, serde::Deserialize)]
struct Out {
    answer: String,
}

#[tokio::test]
async fn one_shot_request_is_a_single_user_turn_at_zero_temperature() {
    let request = GenerateRequest::one_shot("m", "sys", "hi");

    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].role, Role::User);
    assert_eq!(request.system.as_deref(), Some("sys"));
    assert_eq!(request.temperature, Some(0.0));
    assert!(request.tools.is_empty());
    assert!(!request.web_fetch);
}

#[tokio::test]
async fn every_option_is_a_request_builder() {
    let request = GenerateRequest::one_shot("m", "sys", "hi")
        .with_web_fetch(true)
        .with_thinking_budget(0)
        .with_max_tokens(512)
        .with_tools(vec![report_tool()]);

    assert!(request.web_fetch);
    assert_eq!(request.thinking_budget, Some(0));
    assert_eq!(request.max_tokens, Some(512));
    assert_eq!(request.tools.len(), 1);
}

#[tokio::test]
async fn text_joins_parts_and_usage_survives() {
    let provider = stub(
        vec![
            ContentPart::Text("part one ".into()),
            ContentPart::Text("part two".into()),
        ],
        Some(usage()),
    );
    let done = one_shot(&provider, &GenerateRequest::one_shot("m", "sys", "hi"))
        .await
        .unwrap();

    assert_eq!(done.text().unwrap(), "part one part two");
    assert_eq!(done.stop_reason(), Some("STOP"));
    assert_eq!(done.usage.as_ref().unwrap().total_tokens, Some(15));
}

#[tokio::test]
async fn usage_is_readable_even_when_the_model_returned_nothing() {
    let provider = stub(vec![], Some(usage()));
    let done = one_shot(&provider, &GenerateRequest::one_shot("m", "sys", "hi"))
        .await
        .unwrap();

    assert!(done.text().is_none());
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, Some(10));
}

#[tokio::test]
async fn parse_deserializes_the_tool_arguments() {
    let provider = stub(
        vec![tool_call("report", serde_json::json!({"answer": "42"}))],
        Some(usage()),
    );
    let request = GenerateRequest::one_shot("m", "sys", "hi").with_tools(vec![report_tool()]);

    let done = one_shot(&provider, &request).await.unwrap();
    assert_eq!(done.parse::<Out>().unwrap().answer, "42");
}

#[tokio::test]
async fn a_response_carrying_text_and_a_tool_call_exposes_both() {
    let provider = stub(
        vec![
            ContentPart::Text("here you go".into()),
            tool_call("report", serde_json::json!({"answer": "42"})),
        ],
        None,
    );
    let done = one_shot(&provider, &GenerateRequest::one_shot("m", "sys", "hi"))
        .await
        .unwrap();

    assert_eq!(done.text().unwrap(), "here you go");
    assert_eq!(done.parse::<Out>().unwrap().answer, "42");
}

#[tokio::test]
async fn parse_takes_the_first_tool_call_and_the_rest_stay_inspectable() {
    let provider = stub(
        vec![
            tool_call("report", serde_json::json!({"answer": "first"})),
            tool_call("other", serde_json::json!({"answer": "second"})),
        ],
        None,
    );
    let done = one_shot(&provider, &GenerateRequest::one_shot("m", "sys", "hi"))
        .await
        .unwrap();

    assert_eq!(done.parse::<Out>().unwrap().answer, "first");
    assert_eq!(done.tool_calls().len(), 2);
    let names: Vec<&str> = done
        .tool_calls()
        .iter()
        .filter_map(|p| match p {
            ContentPart::ToolCall { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["report", "other"]);
}

#[tokio::test]
async fn parse_errors_without_a_tool_call_and_on_mismatched_arguments() {
    let provider = stub(vec![ContentPart::Text("prose instead".into())], None);
    let done = one_shot(&provider, &GenerateRequest::one_shot("m", "sys", "hi"))
        .await
        .unwrap();
    assert!(
        done.parse::<serde_json::Value>()
            .unwrap_err()
            .is_dependency_error()
    );

    #[derive(Debug, serde::Deserialize)]
    struct Typed {
        #[allow(dead_code)]
        answer: u32,
    }
    let provider = stub(
        vec![tool_call(
            "report",
            serde_json::json!({"answer": "not a number"}),
        )],
        None,
    );
    let done = one_shot(&provider, &GenerateRequest::one_shot("m", "sys", "hi"))
        .await
        .unwrap();
    assert!(done.parse::<Typed>().unwrap_err().is_dependency_error());
}
