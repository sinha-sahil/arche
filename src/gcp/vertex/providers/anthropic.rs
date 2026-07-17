use crate::error::AppError;
use crate::gcp::vertex::client::VertexClient;
use crate::gcp::vertex::config::ResolvedAuth;
use crate::llm::{
    ContentPart, GenerateRequest, GenerateResponse, ParameterSchema, Role, StreamChunk, Usage,
};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Serialize)]
struct Request {
    anthropic_version: String,
    max_tokens: u32,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<WireToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize, Deserialize)]
struct WireMessage {
    role: String,
    content: WireContent,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum WireContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize)]
struct WireToolDef {
    name: String,
    description: String,
    input_schema: ParameterSchema,
}

#[derive(Deserialize)]
struct Response {
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireUsage {
    input_tokens: u32,
    output_tokens: u32,
}

impl From<WireUsage> for Usage {
    fn from(u: WireUsage) -> Self {
        Usage {
            input_tokens: Some(u.input_tokens),
            output_tokens: Some(u.output_tokens),
            total_tokens: Some(u.input_tokens.saturating_add(u.output_tokens)),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart {
        #[serde(default)]
        message: Option<StreamMessageInit>,
    },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        #[serde(default)]
        content_block: Option<ContentBlockInfo>,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: Delta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {},
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDeltaBody },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Deserialize)]
struct StreamMessageInit {
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlockInfo {
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "text")]
    Text {},
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Delta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct MessageDeltaBody {
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

pub(crate) async fn generate(
    client: &VertexClient,
    request: &GenerateRequest,
) -> Result<GenerateResponse, AppError> {
    let url = endpoint(&client.auth, &request.model, false)?;
    let req = client
        .authorize(client.http.post(&url).json(&to_wire(request)))
        .await?;

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
    let url = endpoint(&client.auth, &request.model, true)?;
    let mut wire = to_wire(request);
    wire.stream = Some(true);
    let req = client.authorize(client.http.post(&url).json(&wire)).await?;

    let resp = client.send(req).await?;
    Ok(parse_sse(resp))
}

fn endpoint(auth: &ResolvedAuth, model: &str, stream: bool) -> Result<String, AppError> {
    let method = if stream {
        "streamRawPredict"
    } else {
        "rawPredict"
    };
    match auth {
        ResolvedAuth::ApiKey { .. } => Err(AppError::internal_error(
            "Anthropic models on Vertex AI require service account auth (VERTEX_PROJECT_ID + GOOGLE_APPLICATION_CREDENTIALS)".into(),
            None,
        )),
        ResolvedAuth::ServiceAccount {
            project_id,
            region,
            ..
        } => {
            let host = if region == "global" {
                "aiplatform.googleapis.com".to_string()
            } else {
                format!("{region}-aiplatform.googleapis.com")
            };
            Ok(format!(
                "https://{host}/v1/projects/{project_id}/locations/{region}/publishers/anthropic/models/{model}:{method}"
            ))
        }
    }
}

fn to_wire(req: &GenerateRequest) -> Request {
    let messages = req
        .messages
        .iter()
        .map(|m| WireMessage {
            role: match m.role {
                Role::User => "user".into(),
                Role::Assistant => "assistant".into(),
            },
            content: match m.content.as_slice() {
                [ContentPart::Text(text)] => WireContent::Text(text.clone()),
                parts => WireContent::Blocks(to_blocks(parts)),
            },
        })
        .collect();

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| WireToolDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                })
                .collect(),
        )
    };

    Request {
        anthropic_version: "vertex-2023-10-16".into(),
        max_tokens: req.max_tokens.unwrap_or(4096),
        messages,
        system: req.system.clone(),
        tools,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        stream: None,
    }
}

fn to_blocks(parts: &[ContentPart]) -> Vec<ContentBlock> {
    parts
        .iter()
        .map(|p| match p {
            ContentPart::Text(text) => ContentBlock::Text { text: text.clone() },
            ContentPart::ToolCall {
                id,
                name,
                arguments,
                ..
            } => ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: arguments.clone(),
            },
            ContentPart::ToolResult {
                tool_call_id,
                content,
                ..
            } => ContentBlock::ToolResult {
                tool_use_id: tool_call_id.clone(),
                content: value_to_tool_result_string(content),
            },
        })
        .collect()
}

/// Anthropic's `tool_result.content` is a raw string. If our canonical content is
/// a JSON string, unwrap it (`Value::String("hi")` → `hi`). Any other JSON value
/// is serialized as-is so the model sees valid JSON.
fn value_to_tool_result_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn from_wire(resp: Response) -> GenerateResponse {
    let content = resp
        .content
        .into_iter()
        .map(|block| match block {
            ContentBlock::Text { text } => ContentPart::Text(text),
            ContentBlock::ToolUse { id, name, input } => ContentPart::ToolCall {
                id,
                name,
                arguments: input,
                thought_signature: None,
            },
            ContentBlock::ToolResult { .. } => ContentPart::Text("[tool_result]".into()),
        })
        .collect();

    GenerateResponse {
        content,
        stop_reason: resp.stop_reason,
        usage: resp.usage.map(Usage::from),
    }
}

fn parse_sse(
    resp: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = Result<StreamChunk, AppError>> + Send>> {
    let stream = async_stream::stream! {
        let mut byte_stream = futures::StreamExt::fuse(resp.bytes_stream());
        let mut buffer: Vec<u8> = Vec::new();

        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_json = String::new();

        let mut input_usage: Option<u32> = None;
        let mut output_usage: Option<u32> = None;
        let mut done_sent = false;

        while let Some(chunk) = futures::StreamExt::next(&mut byte_stream).await {
            let bytes = chunk.map_err(|e| {
                AppError::dependency_failed("vertex-ai", format!("Stream read error: {e}"))
            })?;
            buffer.extend_from_slice(&bytes);

            while let Some((pos, sep_len)) = find_frame_boundary(&buffer) {
                let frame = buffer.drain(..pos + sep_len).collect::<Vec<u8>>();
                let Ok(event_block) = std::str::from_utf8(&frame[..frame.len() - sep_len]) else {
                    tracing::debug!("Anthropic SSE frame was not valid UTF-8");
                    continue;
                };

                let mut data_line = None;
                for line in event_block.lines() {
                    if let Some(d) = line.strip_prefix("data: ") {
                        data_line = Some(d);
                    }
                }

                let Some(data) = data_line else { continue };

                match serde_json::from_str::<StreamEvent>(data) {
                    Ok(event) => match event {
                        StreamEvent::MessageStart { message } => {
                            if let Some(m) = message
                                && let Some(u) = m.usage
                            {
                                input_usage = Some(u.input_tokens);
                                output_usage = Some(u.output_tokens);
                            }
                        }
                        StreamEvent::ContentBlockStart { content_block } => {
                            if let Some(ContentBlockInfo::ToolUse { id, name }) = content_block {
                                current_tool_id = id;
                                current_tool_name = name;
                                current_tool_json.clear();
                            }
                        }
                        StreamEvent::ContentBlockDelta {
                            delta: Delta::TextDelta { text },
                        } => {
                            yield Ok(StreamChunk::Text(text));
                        }
                        StreamEvent::ContentBlockDelta {
                            delta: Delta::InputJsonDelta { partial_json },
                        } => {
                            current_tool_json.push_str(&partial_json);
                        }
                        StreamEvent::ContentBlockStop {} => {
                            if !current_tool_name.is_empty() {
                                let arguments = if current_tool_json.is_empty() {
                                    serde_json::Value::Object(serde_json::Map::new())
                                } else {
                                    serde_json::from_str(&current_tool_json)
                                        .unwrap_or(serde_json::Value::Null)
                                };
                                yield Ok(StreamChunk::ToolCall {
                                    id: std::mem::take(&mut current_tool_id),
                                    name: std::mem::take(&mut current_tool_name),
                                    arguments,
                                    thought_signature: None,
                                });
                                current_tool_json.clear();
                            }
                        }
                        StreamEvent::MessageDelta { delta } => {
                            if let Some(u) = delta.usage {
                                input_usage = input_usage.or(Some(u.input_tokens));
                                output_usage = Some(u.output_tokens);
                            }
                            if !done_sent && let Some(reason) = delta.stop_reason {
                                yield Ok(StreamChunk::Done {
                                    finish_reason: reason,
                                    usage: final_usage(input_usage, output_usage),
                                });
                                done_sent = true;
                            }
                        }
                        StreamEvent::MessageStop => {
                            if !done_sent {
                                yield Ok(StreamChunk::Done {
                                    finish_reason: "end_turn".into(),
                                    usage: final_usage(input_usage, output_usage),
                                });
                                done_sent = true;
                            }
                        }
                        StreamEvent::Ping => {}
                    },
                    Err(e) => {
                        tracing::debug!("Failed to parse Anthropic SSE chunk: {e}, data: {data}");
                    }
                }
            }
        }
    };

    Box::pin(stream)
}

fn final_usage(input: Option<u32>, output: Option<u32>) -> Option<Usage> {
    if input.is_none() && output.is_none() {
        return None;
    }
    let total_tokens = input.zip(output).map(|(a, b)| a.saturating_add(b));
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens,
    })
}

fn find_frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
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
    fn value_to_tool_result_string_unwraps_json_string() {
        let v = serde_json::Value::String("hello".into());
        assert_eq!(value_to_tool_result_string(&v), "hello");
    }

    #[test]
    fn value_to_tool_result_string_serializes_object() {
        let v = serde_json::json!({"count": 3});
        let out = value_to_tool_result_string(&v);
        assert!(out.contains("\"count\""));
        assert!(out.contains("3"));
    }

    #[test]
    fn to_blocks_tool_result_sends_raw_string_to_anthropic() {
        let parts = vec![ContentPart::ToolResult {
            tool_call_id: "t1".into(),
            name: "search".into(),
            content: serde_json::Value::String("raw text".into()),
        }];
        let blocks = to_blocks(&parts);
        match &blocks[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, "raw text");
            }
            _ => panic!("expected tool_result block"),
        }
    }

    #[test]
    fn final_usage_none_when_both_missing() {
        assert!(final_usage(None, None).is_none());
    }

    #[test]
    fn final_usage_sums_when_both_present() {
        let u = final_usage(Some(10), Some(5)).unwrap();
        assert_eq!(u.input_tokens, Some(10));
        assert_eq!(u.output_tokens, Some(5));
        assert_eq!(u.total_tokens, Some(15));
    }

    #[test]
    fn find_frame_boundary_lf() {
        let b = b"event: x\ndata: hi\n\nnext";
        assert_eq!(find_frame_boundary(b), Some((17, 2)));
    }

    #[test]
    fn find_frame_boundary_crlf() {
        let b = b"event: x\r\ndata: hi\r\n\r\nnext";
        assert_eq!(find_frame_boundary(b), Some((18, 4)));
    }
}
