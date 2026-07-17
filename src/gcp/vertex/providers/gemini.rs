use crate::error::AppError;
use crate::gcp::vertex::client::VertexClient;
use crate::gcp::vertex::config::ResolvedAuth;
use crate::llm::{ContentPart, GenerateRequest, GenerateResponse, Role, StreamChunk, Usage};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenConfig>,
}

#[derive(Serialize, Deserialize)]
struct Content {
    role: String,
    parts: Vec<Part>,
}

/// Thinking models emit `thoughtSignature` on a separate thought Part that may
/// precede (non-streaming) or follow (streaming) the function_call Part. All
/// fields optional so any Part shape deserializes.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    function_call: Option<FnCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    function_response: Option<FnResponse>,
}

#[derive(Serialize, Deserialize)]
struct FnCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct FnResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tool {
    function_declarations: Vec<FunctionDecl>,
}

#[derive(Serialize)]
struct FunctionDecl {
    name: String,
    description: String,
    parameters: crate::llm::ParameterSchema,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

/// Gemini 2.5+ thinking config. Setting `thinking_budget: 0` disables thinking
/// and avoids the `thought_signature` requirement on function_call parts.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    thinking_budget: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    candidates: Option<Vec<Candidate>>,
    usage_metadata: Option<UsageMeta>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<Content>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMeta {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

impl From<UsageMeta> for Usage {
    fn from(u: UsageMeta) -> Self {
        Usage {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
        }
    }
}

pub(crate) async fn generate(
    client: &VertexClient,
    request: &GenerateRequest,
) -> Result<GenerateResponse, AppError> {
    let url = endpoint(&client.auth, &request.model, false);
    let wire = to_wire(request);
    let req = client.authorize(client.http.post(&url).json(&wire)).await?;

    let resp = client.send(req).await?;
    let wire: Response = resp
        .json()
        .await
        .map_err(|e| AppError::dependency_failed("vertex-ai", format!("Parse failed: {e}")))?;

    Ok(from_wire(wire))
}

pub(crate) async fn stream_generate(
    client: &VertexClient,
    request: &GenerateRequest,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, AppError>> + Send>>, AppError> {
    let base = endpoint(&client.auth, &request.model, true);
    let url = format!("{base}?alt=sse");
    let wire = to_wire(request);
    let req = client.authorize(client.http.post(&url).json(&wire)).await?;

    let resp = client.send(req).await?;
    Ok(parse_sse(resp))
}

fn endpoint(auth: &ResolvedAuth, model: &str, stream: bool) -> String {
    let method = if stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    match auth {
        ResolvedAuth::ApiKey { .. } => {
            format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:{method}")
        }
        ResolvedAuth::ServiceAccount {
            project_id, region, ..
        } => {
            let host = if region == "global" {
                "aiplatform.googleapis.com".to_string()
            } else {
                format!("{region}-aiplatform.googleapis.com")
            };
            format!(
                "https://{host}/v1/projects/{project_id}/locations/{region}/publishers/google/models/{model}:{method}"
            )
        }
    }
}

fn to_wire(req: &GenerateRequest) -> Request {
    let contents = req
        .messages
        .iter()
        .map(|m| Content {
            role: match m.role {
                Role::User => "user".into(),
                Role::Assistant => "model".into(),
            },
            parts: m
                .content
                .iter()
                .map(|p| match p {
                    ContentPart::Text(text) => Part {
                        text: Some(text.clone()),
                        ..Default::default()
                    },
                    ContentPart::ToolCall {
                        name,
                        arguments,
                        thought_signature,
                        ..
                    } => Part {
                        function_call: Some(FnCall {
                            name: name.clone(),
                            args: arguments.clone(),
                        }),
                        thought_signature: thought_signature.clone(),
                        ..Default::default()
                    },
                    ContentPart::ToolResult { name, content, .. } => Part {
                        function_response: Some(FnResponse {
                            name: name.clone(),
                            // Gemini requires a Struct here; wrap primitives.
                            response: if content.is_object() {
                                content.clone()
                            } else {
                                serde_json::json!({ "result": content })
                            },
                        }),
                        ..Default::default()
                    },
                })
                .collect(),
        })
        .collect();

    let system_instruction = req.system.as_ref().map(|s| Content {
        role: "user".into(),
        parts: vec![Part {
            text: Some(s.clone()),
            ..Default::default()
        }],
    });

    let generation_config = if req.max_tokens.is_some()
        || req.temperature.is_some()
        || req.top_p.is_some()
        || req.top_k.is_some()
        || req.thinking_budget.is_some()
    {
        Some(GenConfig {
            max_output_tokens: req.max_tokens,
            temperature: req.temperature,
            top_p: req.top_p,
            top_k: req.top_k,
            thinking_config: req
                .thinking_budget
                .map(|thinking_budget| ThinkingConfig { thinking_budget }),
        })
    } else {
        None
    };

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(vec![Tool {
            function_declarations: req
                .tools
                .iter()
                .map(|t| FunctionDecl {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                })
                .collect(),
        }])
    };

    Request {
        contents,
        system_instruction,
        generation_config,
        tools,
    }
}

fn from_wire(resp: Response) -> GenerateResponse {
    let mut content = Vec::new();
    let mut stop_reason = None;

    if let Some(candidates) = &resp.candidates
        && let Some(candidate) = candidates.first()
    {
        stop_reason = candidate.finish_reason.clone();
        if let Some(c) = &candidate.content {
            let mut pending_signature: Option<String> = None;
            for part in &c.parts {
                if let Some(sig) = &part.thought_signature {
                    pending_signature = Some(sig.clone());
                }
                if let Some(fc) = &part.function_call {
                    content.push(ContentPart::ToolCall {
                        id: nanoid::nanoid!(),
                        name: fc.name.clone(),
                        arguments: fc.args.clone(),
                        thought_signature: pending_signature.take(),
                    });
                } else if part.thought != Some(true)
                    && let Some(text) = &part.text
                {
                    content.push(ContentPart::Text(text.clone()));
                }
            }
        }
    }

    GenerateResponse {
        content,
        stop_reason,
        usage: resp.usage_metadata.map(Usage::from),
    }
}

fn parse_sse(
    resp: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = Result<StreamChunk, AppError>> + Send>> {
    let stream = async_stream::stream! {
        let mut byte_stream = futures::StreamExt::fuse(resp.bytes_stream());
        let mut buffer: Vec<u8> = Vec::new();
        let mut latest_usage: Option<Usage> = None;
        let mut done_sent = false;
        let mut pending_signature: Option<String> = None;
        let mut buffered_tool_calls: Vec<(String, serde_json::Value)> = Vec::new();

        while let Some(chunk) = futures::StreamExt::next(&mut byte_stream).await {
            let bytes = chunk.map_err(|e| {
                AppError::dependency_failed("vertex-ai", format!("Stream read error: {e}"))
            })?;
            buffer.extend_from_slice(&bytes);

            while let Some((pos, sep_len)) = find_frame_boundary(&buffer) {
                let frame = buffer.drain(..pos + sep_len).collect::<Vec<u8>>();
                let Ok(event) = std::str::from_utf8(&frame[..frame.len() - sep_len]) else {
                    tracing::debug!("Gemini SSE frame was not valid UTF-8");
                    continue;
                };

                let Some(data) = event.strip_prefix("data: ") else {
                    continue;
                };

                if data.trim() == "[DONE]" {
                    if !done_sent {
                        let sig = pending_signature.take();
                        for (name, args) in buffered_tool_calls.drain(..) {
                            yield Ok(StreamChunk::ToolCall {
                                id: nanoid::nanoid!(),
                                name,
                                arguments: args,
                                thought_signature: sig.clone(),
                            });
                        }
                        yield Ok(StreamChunk::Done {
                            finish_reason: "STOP".into(),
                            usage: latest_usage.take(),
                        });
                        done_sent = true;
                    }
                    continue;
                }

                match serde_json::from_str::<Response>(data) {
                    Ok(response) => {
                        if let Some(meta) = response.usage_metadata {
                            latest_usage = Some(meta.into());
                        }
                        let Some(candidates) = response.candidates else { continue };
                        for candidate in candidates {
                            if let Some(content) = candidate.content {
                                for part in content.parts {
                                    if let Some(sig) = part.thought_signature {
                                        pending_signature = Some(sig);
                                    }
                                    if let Some(fc) = part.function_call {
                                        buffered_tool_calls.push((fc.name, fc.args));
                                    } else if part.thought != Some(true)
                                        && let Some(text) = part.text
                                    {
                                        yield Ok(StreamChunk::Text(text));
                                    }
                                }
                            }
                            if !done_sent
                                && let Some(reason) = candidate.finish_reason
                                && (reason == "STOP" || reason == "MAX_TOKENS")
                            {
                                let sig = pending_signature.take();
                                for (name, args) in buffered_tool_calls.drain(..) {
                                    yield Ok(StreamChunk::ToolCall {
                                        id: nanoid::nanoid!(),
                                        name,
                                        arguments: args,
                                        thought_signature: sig.clone(),
                                    });
                                }
                                yield Ok(StreamChunk::Done {
                                    finish_reason: reason,
                                    usage: latest_usage.take(),
                                });
                                done_sent = true;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse Gemini SSE chunk: {e}, data: {data}");
                    }
                }
            }
        }
    };

    Box::pin(stream)
}

fn find_frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    // Prefer CRLF-CRLF (4 bytes) if it appears before any LF-LF match.
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n");
    let lf = buf.windows(2).position(|w| w == b"\n\n");
    match (crlf, lf) {
        (Some(c), Some(l)) => {
            if c <= l {
                Some((c, 4))
            } else {
                Some((l, 2))
            }
        }
        (Some(c), None) => Some((c, 4)),
        (None, Some(l)) => Some((l, 2)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_endpoint_never_carries_the_key() {
        let auth = ResolvedAuth::ApiKey {
            api_key: "secret-key".into(),
        };
        let url = endpoint(&auth, "gemini-2.5-flash", false);
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent"
        );

        let stream_url = endpoint(&auth, "gemini-2.5-flash", true);
        assert_eq!(
            stream_url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent"
        );
        assert!(!url.contains("secret-key") && !stream_url.contains("secret-key"));
    }

    #[test]
    fn to_wire_emits_thinking_config_when_budget_set() {
        let req = GenerateRequest::new("m", vec![]).with_thinking_budget(0);
        let json = serde_json::to_value(to_wire(&req)).unwrap();
        assert_eq!(
            json["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
    }

    #[test]
    fn to_wire_omits_generation_config_without_knobs() {
        let req = GenerateRequest::new("m", vec![]);
        let json = serde_json::to_value(to_wire(&req)).unwrap();
        assert!(json.get("generationConfig").is_none());
    }

    #[test]
    fn to_wire_keeps_thinking_config_absent_when_only_other_knobs_set() {
        let req = GenerateRequest::new("m", vec![]).with_temperature(0.0);
        let json = serde_json::to_value(to_wire(&req)).unwrap();
        assert!(json["generationConfig"].get("thinkingConfig").is_none());
    }
}
