use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub enum ContentPart {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        /// Used by Gemini 2.5+ thinking models (`thoughtSignature`). Other providers ignore.
        thought_signature: Option<String>,
    },
    ToolResult {
        tool_call_id: String,
        name: String,
        content: serde_json::Value,
    },
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::Text(text.into())],
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::Text(text.into())],
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: serde_json::Value,
    ) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::ToolResult {
                tool_call_id: tool_call_id.into(),
                name: name.into(),
                content,
            }],
        }
    }

    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::ToolCall {
                id: id.into(),
                name: name.into(),
                arguments,
                thought_signature: None,
            }],
        }
    }

    pub fn tool_call_with_signature(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        thought_signature: Option<String>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::ToolCall {
                id: id.into(),
                name: name.into(),
                arguments,
                thought_signature,
            }],
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GenerateRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub thinking_budget: Option<u32>,
    /// Providers without a built-in fetcher ignore this.
    pub web_fetch: bool,
    pub tools: Vec<ToolDefinition>,
}

impl GenerateRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            system: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            thinking_budget: None,
            web_fetch: false,
            tools: Vec::new(),
        }
    }

    pub fn one_shot(
        model: impl Into<String>,
        system: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::new(model, vec![Message::user(prompt)])
            .with_system(system)
            .with_temperature(0.0)
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, v: u32) -> Self {
        self.max_tokens = Some(v);
        self
    }

    pub fn with_temperature(mut self, v: f32) -> Self {
        self.temperature = Some(v);
        self
    }

    pub fn with_top_p(mut self, v: f32) -> Self {
        self.top_p = Some(v);
        self
    }

    pub fn with_top_k(mut self, v: u32) -> Self {
        self.top_k = Some(v);
        self
    }

    pub fn with_thinking_budget(mut self, v: u32) -> Self {
        self.thinking_budget = Some(v);
        self
    }

    pub fn with_web_fetch(mut self, enabled: bool) -> Self {
        self.web_fetch = enabled;
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }
}

#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub content: Vec<ContentPart>,
    pub stop_reason: Option<String>,
    pub usage: Option<Usage>,
}

impl GenerateResponse {
    pub fn text(&self) -> Option<String> {
        let text: String = self
            .content
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();

        if text.is_empty() { None } else { Some(text) }
    }

    pub fn tool_calls(&self) -> Vec<&ContentPart> {
        self.content
            .iter()
            .filter(|p| matches!(p, ContentPart::ToolCall { .. }))
            .collect()
    }

    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    /// Arguments of the **first** tool call; use [`GenerateResponse::tool_calls`] for the rest.
    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T, crate::error::AppError> {
        let args = self
            .content
            .iter()
            .find_map(|p| match p {
                ContentPart::ToolCall { arguments, .. } => Some(arguments),
                _ => None,
            })
            .ok_or_else(|| {
                crate::error::AppError::dependency_failed("llm", "model returned no tool call")
            })?;
        serde_json::from_value(args.clone()).map_err(|e| {
            crate::error::AppError::dependency_failed("llm", format!("bad tool args: {e}"))
        })
    }
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        /// Used by Gemini 2.5+ thinking models (`thoughtSignature`). Other providers emit `None`.
        thought_signature: Option<String>,
    },
    Done {
        finish_reason: String,
        usage: Option<Usage>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub parameters: ParameterSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    #[serde(skip_serializing_if = "IndexMap::is_empty", default)]
    pub metadata: IndexMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSchema {
    #[serde(rename = "type")]
    pub schema_type: SchemaType,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<IndexMap<String, ParameterSchema>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<ParameterSchema>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,

    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: None,
            description: description.into(),
            parameters: ParameterSchema::object(),
            annotations: None,
            metadata: IndexMap::new(),
        }
    }

    pub fn with_parameters(mut self, params: ParameterSchema) -> Self {
        self.parameters = params;
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_annotations(mut self, annotations: ToolAnnotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    pub fn with_metadata(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn with_metadata_entries<I, K, V>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<serde_json::Value>,
    {
        for (k, v) in entries {
            self.metadata.insert(k.into(), v.into());
        }
        self
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn annotations(&self) -> Option<&ToolAnnotations> {
        self.annotations.as_ref()
    }

    pub fn metadata(&self) -> &IndexMap<String, serde_json::Value> {
        &self.metadata
    }
}

impl ParameterSchema {
    pub fn string(description: impl Into<String>) -> Self {
        Self {
            schema_type: SchemaType::String,
            description: Some(description.into()),
            properties: None,
            items: None,
            required: None,
            enum_values: None,
        }
    }

    pub fn integer(description: impl Into<String>) -> Self {
        Self {
            schema_type: SchemaType::Integer,
            description: Some(description.into()),
            properties: None,
            items: None,
            required: None,
            enum_values: None,
        }
    }

    pub fn number(description: impl Into<String>) -> Self {
        Self {
            schema_type: SchemaType::Number,
            description: Some(description.into()),
            properties: None,
            items: None,
            required: None,
            enum_values: None,
        }
    }

    pub fn boolean(description: impl Into<String>) -> Self {
        Self {
            schema_type: SchemaType::Boolean,
            description: Some(description.into()),
            properties: None,
            items: None,
            required: None,
            enum_values: None,
        }
    }

    pub fn string_enum(
        description: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            schema_type: SchemaType::String,
            description: Some(description.into()),
            properties: None,
            items: None,
            required: None,
            enum_values: Some(values.into_iter().map(|v| v.into()).collect()),
        }
    }

    pub fn array(items: ParameterSchema) -> Self {
        Self {
            schema_type: SchemaType::Array,
            description: None,
            properties: None,
            items: Some(Box::new(items)),
            required: None,
            enum_values: None,
        }
    }

    pub fn object() -> Self {
        Self {
            schema_type: SchemaType::Object,
            description: None,
            properties: Some(IndexMap::new()),
            items: None,
            required: None,
            enum_values: None,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_property(mut self, name: impl Into<String>, schema: ParameterSchema) -> Self {
        self.properties
            .get_or_insert_with(IndexMap::new)
            .insert(name.into(), schema);
        self
    }

    pub fn with_required(mut self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.required = Some(fields.into_iter().map(|f| f.into()).collect());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definition_serializes_to_expected_json_schema() {
        let tool = ToolDefinition::new("search", "Search for things").with_parameters(
            ParameterSchema::object()
                .with_property("query", ParameterSchema::string("Search text"))
                .with_property("limit", ParameterSchema::integer("Max results"))
                .with_required(["query"]),
        );

        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "search");
        assert_eq!(json["parameters"]["type"], "object");
        assert_eq!(json["parameters"]["properties"]["query"]["type"], "string");
        assert_eq!(json["parameters"]["required"], serde_json::json!(["query"]));
    }

    #[test]
    fn parameter_schema_preserves_property_order_in_json_output() {
        let schema = ParameterSchema::object()
            .with_property("zebra", ParameterSchema::string(""))
            .with_property("alpha", ParameterSchema::string(""))
            .with_property("mike", ParameterSchema::string(""));

        let json = serde_json::to_string(&schema).unwrap();
        let z = json.find("\"zebra\"").expect("zebra key in output");
        let a = json.find("\"alpha\"").expect("alpha key in output");
        let m = json.find("\"mike\"").expect("mike key in output");
        assert!(z < a && a < m, "unexpected order in: {json}");
    }

    #[test]
    fn parameter_schema_nested_array_round_trips() {
        let schema = ParameterSchema::object().with_property(
            "tags",
            ParameterSchema::array(ParameterSchema::string("A tag")),
        );
        let json = serde_json::to_string(&schema).unwrap();
        let back: ParameterSchema = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.schema_type, SchemaType::Object));
    }

    #[test]
    fn string_enum_renders_enum_field() {
        let schema = ParameterSchema::string_enum("choice", ["a", "b", "c"]);
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["enum"], serde_json::json!(["a", "b", "c"]));
    }

    #[test]
    fn one_shot_shape_sets_single_user_message_and_zero_temperature() {
        let req = GenerateRequest::one_shot("m", "sys", "hello");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, Role::User);
        assert_eq!(req.system.as_deref(), Some("sys"));
        assert_eq!(req.temperature, Some(0.0));
        assert!(req.tools.is_empty());
        assert!(!req.web_fetch);
    }

    #[test]
    fn generate_request_web_fetch_defaults_off_and_sets_via_builder() {
        let req = GenerateRequest::new("m", vec![]);
        assert!(!req.web_fetch);
        assert!(req.with_web_fetch(true).web_fetch);
    }

    #[test]
    fn generate_request_thinking_budget_defaults_off_and_sets_via_builder() {
        let req = GenerateRequest::new("m", vec![]);
        assert!(req.thinking_budget.is_none());
        let req = req.with_thinking_budget(0);
        assert_eq!(req.thinking_budget, Some(0));
    }

    #[test]
    fn message_tool_call_builds_assistant_message() {
        let m = Message::tool_call("call_1", "search", serde_json::json!({"q": "hi"}));
        assert_eq!(m.role, Role::Assistant);
        assert_eq!(m.content.len(), 1);
        assert!(matches!(m.content[0], ContentPart::ToolCall { .. }));
    }

    #[test]
    fn tool_definition_default_has_no_annotations_or_metadata() {
        let t = ToolDefinition::new("foo", "does foo");
        assert!(t.title().is_none());
        assert!(t.annotations().is_none());
        assert!(t.metadata().is_empty());
    }

    #[test]
    fn tool_definition_builders_chain_and_persist() {
        let t = ToolDefinition::new("create_cart", "Create a cart")
            .with_title("Create Cart")
            .with_annotations(ToolAnnotations {
                read_only: Some(false),
                destructive: Some(false),
                idempotent: Some(false),
                open_world: Some(false),
            })
            .with_metadata("mcp.invoking", "Creating cart…")
            .with_metadata("mcp.invoked", "Cart created");

        assert_eq!(t.title(), Some("Create Cart"));
        let a = t.annotations().expect("annotations set");
        assert_eq!(a.read_only, Some(false));
        assert_eq!(a.destructive, Some(false));
        assert_eq!(a.idempotent, Some(false));
        assert_eq!(a.open_world, Some(false));
        assert_eq!(
            t.metadata().get("mcp.invoking").and_then(|v| v.as_str()),
            Some("Creating cart…")
        );
        assert_eq!(
            t.metadata().get("mcp.invoked").and_then(|v| v.as_str()),
            Some("Cart created")
        );
    }

    #[test]
    fn tool_definition_with_metadata_entries_bulk_inserts() {
        let t = ToolDefinition::new("x", "x")
            .with_metadata_entries([("mcp.invoking", "Working…"), ("mcp.invoked", "Done")]);
        assert_eq!(t.metadata().len(), 2);
    }

    #[test]
    fn tool_definition_serialize_omits_unset_optional_fields() {
        let t = ToolDefinition::new("foo", "does foo");
        let v = serde_json::to_value(&t).unwrap();
        assert!(v.get("title").is_none(), "title leaked: {v}");
        assert!(v.get("annotations").is_none(), "annotations leaked: {v}");
        assert!(v.get("metadata").is_none(), "empty metadata leaked: {v}");
    }

    #[test]
    fn tool_definition_pre_extension_json_round_trips() {
        let pre = serde_json::json!({
            "name": "foo",
            "description": "does foo",
            "parameters": { "type": "object", "properties": {} }
        });
        let t: ToolDefinition = serde_json::from_value(pre).unwrap();
        let post = serde_json::to_value(&t).unwrap();
        assert_eq!(post.get("title"), None);
        assert_eq!(post.get("annotations"), None);
        assert_eq!(post.get("metadata"), None);
        assert_eq!(post["name"], "foo");
        assert_eq!(post["description"], "does foo");
    }

    #[test]
    fn tool_annotations_serialize_omits_none_fields() {
        let a = ToolAnnotations {
            read_only: Some(true),
            ..Default::default()
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["read_only"], true);
        assert!(v.get("destructive").is_none());
        assert!(v.get("idempotent").is_none());
        assert!(v.get("open_world").is_none());
    }

    #[test]
    fn tool_annotations_default_is_all_none() {
        let a = ToolAnnotations::default();
        assert!(a.read_only.is_none());
        assert!(a.destructive.is_none());
        assert!(a.idempotent.is_none());
        assert!(a.open_world.is_none());
    }
}
