<div align="center">

# arche

**The opinionated backend foundation for Axum applications.**

[![Crates.io](https://img.shields.io/crates/v/arche.svg)](https://crates.io/crates/arche)
[![Documentation](https://img.shields.io/docsrs/arche)](https://docs.rs/arche)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Cloud integrations, databases, auth, LLM inference, tool-calling agents, encryption,
streaming JSON/CSV, WebSockets, and structured error handling — wired up and ready to go.

`arche` sits *around* Axum, not in place of it.

[Getting Started](#getting-started) · [Modules](#modules) · [API Reference](#api-reference) · [Design Principles](#design-principles)

</div>

---

## Why arche?

Every backend service re-implements the same infrastructure plumbing — cloud SDK
setup, database pools, auth primitives, error handling, config resolution. **arche**
bundles these into a single, cohesive Rust crate built on well-established libraries
so you can skip the boilerplate and focus on business logic.

## Getting Started

Add arche to your `Cargo.toml`:

```toml
[dependencies]
arche = "4.5.1"
```

## Modules

| Module | What it does |
|---|---|
| [`aws`](#aws) | S3, SES, KMS, and CloudFront via official AWS SDKs |
| [`gcp`](#gcp) | Generic GCP REST client + **Vertex AI** (Gemini + Claude); wrappers for Sheets, Drive, Cloud KMS, Cloud Storage, Cloud CDN, and Google OAuth login |
| [`llm`](#llm) | Canonical LLM types + `LlmProvider` trait — backend-agnostic |
| [`agent`](#agent) | Tool-calling agent engine, session state, SSE streaming |
| [`database`](#database) | Postgres and Redis connection pooling with health checks |
| [`jwt`](#jwt) | HS256 token generation, verification, and expiry helpers |
| [`csv`](#csv) | Async CSV read/write — batch, streaming, and from URL |
| [`json`](#json) | Streaming JSON array parsing with metadata extraction |
| [`crypto`](#crypto) | AES-128-CBC encryption with PBKDF2 key derivation |
| [`sockets`](#sockets) | WebSocket connection registry with broadcast |
| [`error`](#error) | Axum-compatible structured error responses (400–503) |
| [`utils`](#utils) | Timestamp validation, date/time conversions, pagination |

Every service module exports a **config builder** so you can wire up credentials
programmatically — or omit it entirely and let arche resolve everything from
environment variables.

```rust
// Pass None to resolve entirely from env vars
let pool = arche::database::pg::get_pg_pool(None).await?;

// Or configure explicitly
let config = arche::database::pg::PgConfigBuilder::default()
    .host(Some("localhost".into()))
    .port(Some(5432))
    .build();
let pool = arche::database::pg::get_pg_pool(config).await?;
```

All components are modular and explicit — nothing is hidden or magical.

---

## API Reference

### AWS

AWS SDK integrations built on official SDKs. Default region: `ap-south-1`.

#### S3

```rust
use arche::aws::s3::{get_s3_client, S3ConfigBuilder};

// From env vars
let client = get_s3_client(None).await?;

// Or with explicit config
let config = S3ConfigBuilder::default()
    .credential_source(Some("env".into()))
    .access_key_id(Some("AKIA...".into()))
    .secret_access_key(Some("secret".into()))
    .build();
let client = get_s3_client(config).await?;
```

| Env Var | Description |
|---|---|
| `S3_CRED_SOURCE` | `"IAM"` (default) or `"env"` |
| `S3_ACCESS_KEY_ID` | Required when source is `"env"` |
| `S3_SECRET_ACCESS_KEY` | Required when source is `"env"` |
| `S3_REGION` | AWS region (default: `ap-south-1`) |

#### KMS

```rust
use arche::aws::kms::KMSClient;

// Default region
let kms = KMSClient::new_with_region("ap-south-1").await;

// Encrypt / decrypt
let ciphertext = kms.encrypt("alias/my-key", b"sensitive data").await?;
let plaintext = kms.decrypt(&ciphertext).await?;

// Decrypt base64-encoded ciphertext directly
let plaintext = kms.decrypt_base64("base64string...").await?;
```

| Env Var | Description |
|---|---|
| `AWS_REGION` | AWS region (default: `ap-south-1`) |

#### SES

```rust
use arche::aws::ses::SESClient;

let ses = SESClient::new_with_region("ap-south-1").await;

// Plain email (with optional HTML body)
let message_id = ses.send_email(
    "from@example.com",
    "to@example.com",
    "Subject line",
    "Plain text body",
    Some("<h1>HTML body</h1>"),
).await?;

// Templated email
let message_id = ses.send_templated_email(
    "from@example.com",
    "to@example.com",
    "TemplateName",
    r#"{"name": "Alice"}"#,
).await?;
```

| Env Var | Description |
|---|---|
| `AWS_REGION` | AWS region (default: `ap-south-1`) |

#### CloudFront

```rust
use arche::aws::cloudfront::{get_cloudfront_client, CloudFrontClient, CloudFrontConfigBuilder};

let aws = get_cloudfront_client(None).await;
let cf = CloudFrontClient::new(aws, None);
```

**Invalidate paths** — submits a CloudFront invalidation and returns immediately
with the invalidation ID and status (typically `"InProgress"`).

```rust
let result = cf.invalidate_paths(
    Some("E1ABCXYZ"),
    vec!["/index.html".into(), "/assets/*".into()],
    None, // caller_reference: None auto-generates a nanoid (not retry-safe); pass a stable value for idempotent retries
).await?;
println!("{} -> {}", result.id, result.status);
```

Per CloudFront limits: paths must start with `/`, max 3000 paths per call,
caller reference ≤ 128 chars.

**Get invalidation status** — fetch the current status of a previously created
invalidation. Returns the same `InvalidationResult` shape; status transitions
from `"InProgress"` to `"Completed"` (typically 5–15 minutes).

```rust
let status = cf.get_invalidation(Some("E1ABCXYZ"), &result.id).await?;
println!("{}", status.status);
```

**Default distribution ID** — set once on the client (or via
`CLOUDFRONT_DISTRIBUTION_ID` env) so per-call `distribution_id` can be `None`:

```rust
let config = CloudFrontConfigBuilder::default()
    .distribution_id("E1ABCXYZ")
    .build();
let cf = CloudFrontClient::new(aws, config);

cf.invalidate_paths(None, vec!["/index.html".into()], None).await?;
cf.get_invalidation(None, "I2J3K4L5...").await?;
```

| Env Var | Description |
|---|---|
| `AWS_REGION` | AWS region (default: `ap-south-1`) |
| `CLOUDFRONT_DISTRIBUTION_ID` | Optional default distribution ID |

---

### GCP

Service-account-authenticated REST client for any Google Cloud API, plus
ergonomic wrappers for Sheets, Drive, and Vertex AI. Built on `reqwest` —
honors `HTTPS_PROXY` / `NO_PROXY` like everything else.

#### Service account credentials

`ServiceAccountKey` is the canonical credential type. Two ways to construct:

```rust
use arche::gcp::ServiceAccountKey;

// From individual fields (e.g. separate env vars or a secrets manager)
let key = ServiceAccountKey::new(client_email, private_key);

// Or from a GCP service-account JSON file on disk
let key = ServiceAccountKey::from_path("/etc/secrets/sa.json").await?;
```

`\n` literals from `.env`-style storage are normalized to real newlines
automatically. The `private_key` is never readable back from the struct and
is masked in `Debug` output.

#### Authentication modes

Every `GcpClient` constructor accepts credentials in three forms, tried in
order:

1. **Explicit `ServiceAccountKey`** — `GcpClient::new(Some(key), None, scopes)`.
2. **Path to a service-account JSON file** — `GcpClient::new(None, Some(path), scopes)`.
3. **Neither → GKE / GCE metadata server (Workload Identity).** When both are
   `None`, arche falls back to
   `${GCP_METADATA_URL:-http://metadata.google.internal}` and exchanges the
   pod's bound service account for an OAuth token. This is the standard
   no-secrets-on-disk flow for pods running on GKE, Cloud Run, or GCE.

```rust
// Workload Identity — no creds passed in, no env vars required on GKE.
let kms = arche::gcp::kms::get_kms_client(None, None, None).await?;
```

Tokens are cached with a 60 s safety margin, single-flighted per scope set,
and retried once on transient failures — uniformly across all three modes.

**Caveats for the metadata-server path:**

- **Scopes are ignored.** The metadata endpoint returns whatever scopes the
  pod's bound service account has, regardless of what you pass to the
  client. If you need narrower scopes, use an explicit `ServiceAccountKey`.
- **`GCP_METADATA_URL` is read at construction time.** Changing the env var
  after the client is built has no effect — set it before the first
  `GcpClient::new(None, None, …)` call. Useful for pointing tests at a
  mock HTTP server.
- **Signed URLs are unsupported.** V4 signed URLs (`GcsClient::sign_*`)
  require the SA's private key, which the metadata server never exposes.
  Those calls return a clear error on this path — use an explicit
  `ServiceAccountKey` for signed-URL workflows.

#### Sheets

```rust
let sheets = arche::gcp::sheets::client(Some(key), None).await?;

let resp = sheets
    .get(format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{spreadsheet_id}/values/{range}",
    ))
    .await?
    .send()
    .await?;
```

Pass either `Some(key)` or `Some(path)` — never both. Scope is preset to
`https://www.googleapis.com/auth/spreadsheets`.

#### Drive

```rust
let drive = arche::gcp::drive::client(None, Some("/etc/secrets/sa.json".into())).await?;

let bytes = drive
    .get(format!("https://www.googleapis.com/drive/v3/files/{file_id}?alt=media"))
    .await?
    .send().await?
    .bytes().await?;
```

Scope is preset to `https://www.googleapis.com/auth/drive`.

#### Cloud KMS

Encrypt and decrypt against Google Cloud KMS using the same service-account
JWT auth as the rest of the GCP family — token caching, retries, and
concurrent-fetch deduplication come for free via `GcpClient`.

```rust
use arche::gcp::kms::{get_kms_client, GcpKmsConfig, GcpKmsKey};

// Build the client. Any unset field falls back to its GCP_KMS_* env var.
let kms = get_kms_client(
    Some(key),
    None,
    GcpKmsConfig::builder().project_id("my-project").build(),
).await?;

// Or fully env-driven (project_id required; location defaults to "global"):
let kms = get_kms_client(Some(key), None, None).await?;

// Key identifier — passed per call so one client can target multiple keys
let kms_key = GcpKmsKey::new("my-keyring", "my-key");
// or: let kms_key = GcpKmsKey::from_env()?;  // GCP_KMS_KEY_RING + GCP_KMS_KEY_NAME

// Encrypt — returns ciphertext + the key version that wrapped it
let out = kms.encrypt(&kms_key, b"sensitive data").await?;
// out.ciphertext: Vec<u8>
// out.key_version: e.g. "projects/.../cryptoKeys/my-key/cryptoKeyVersions/3"
//   Persist this alongside the ciphertext for key-rotation auditing.

let plaintext = kms.decrypt(&kms_key, &out.ciphertext).await?;

// If the ciphertext is already base64 (e.g. read from a DB column):
let plaintext = kms.decrypt_base64(&kms_key, &b64_string).await?;
```

| Env Var | Description |
|---|---|
| `GCP_KMS_PROJECT_ID` | GCP project hosting the KMS key (required) |
| `GCP_KMS_LOCATION` | KMS location (default: `global`) |
| `GCP_KMS_BASE_URL` | Override the Cloud KMS endpoint (testing / VPC-SC) |
| `GCP_KMS_KEY_RING` | Used by `GcpKmsKey::from_env()` |
| `GCP_KMS_KEY_NAME` | Used by `GcpKmsKey::from_env()` |

Already have a `GcpClient` configured for other services? Reuse it via the
exported scope — keeps a single token cache across Sheets / Drive / KMS:

```rust
let kms_gcp = my_gcp_client.with_scopes([arche::gcp::kms::KMS_SCOPE]);
let kms = arche::gcp::kms::GcpKmsClient::new(
    kms_gcp,
    "my-project".into(),
    "global".into(),
    None,
);
```

#### Cloud Storage (GCS)

Object upload / download / delete / list / head, plus V4-signed GET URLs.
Bucket is per-call so one client can target many buckets. Uploads and
downloads buffer the full object in memory — keep this in mind for large
files.

```rust
use arche::gcp::gcs::{get_gcs_client, GcsConfig};
use std::collections::HashMap;
use std::time::Duration;

// All fields optional — gcs_base_url and signed-URL expiry default fine.
let gcs = get_gcs_client(Some(key), None, None).await?;

// Upload with optional user metadata (e.g. `modified_by`)
let mut meta = HashMap::new();
meta.insert("modified_by".into(), "alice".into());
let object = gcs.upload(
    "my-bucket",
    "reports/q4.pdf",
    pdf_bytes,
    "application/pdf",
    meta,
).await?;
// object.generation: Option<i64>  — round-trippable into download/head/delete

// Read it back
let bytes = gcs.download("my-bucket", "reports/q4.pdf", None).await?;

// Or a specific historical version (requires Object Versioning on the bucket)
let old = gcs.download("my-bucket", "reports/q4.pdf", Some(1_700_000_123_456_789)).await?;

// Metadata-only fetch
let meta = gcs.head("my-bucket", "reports/q4.pdf", None).await?;

// Merge-patch user metadata (existing keys overwritten, others untouched)
let mut update = HashMap::new();
update.insert("reviewed_by".into(), "bob".into());
gcs.patch_metadata("my-bucket", "reports/q4.pdf", update).await?;

// List a prefix; pass `versions: true` to include non-current generations
let page = gcs.list("my-bucket", Some("reports/"), None, false).await?;

// V4-signed GET URL (defaults to client's expiry, max 7 days). Always
// points at `storage.googleapis.com` regardless of GCS_BASE_URL.
let url = gcs.signed_get_url("my-bucket", "reports/q4.pdf", Some(Duration::from_secs(600)))?;
```

| Env Var | Description |
|---|---|
| `GCS_BASE_URL` | Override the storage endpoint (testing / VPC-SC; ignored for signed URLs) |
| `GCS_SIGNED_URL_EXPIRY_SECS` | Default expiry for `signed_get_url` (default: 900, max: 604800) |

`upload` switches automatically to multipart when called — user metadata
travels with the bytes in one request, no separate PATCH needed. Object names
containing `/` are URL-encoded for the JSON-API path but left literal in
V4-signed paths, matching GCS spec.

#### Cloud CDN

Cache invalidation against a global URL map, plus operation polling.

```rust
use arche::gcp::cdn::{get_cdn_client, GcpCdnConfig};

let cdn = get_cdn_client(
    Some(key),
    None,
    GcpCdnConfig::builder()
        .project_id("my-project")
        .url_map("my-lb-url-map") // optional default; pass per-call to override
        .build(),
).await?;

// Invalidate. `path` must start with `/` and may use `*` as a suffix wildcard.
// `host` scopes the invalidation to a hostname routed by the URL map.
let op = cdn.invalidate(None, "/static/*", Some("cdn.example.com")).await?;
// op.name — the operation name; pass it to `invalidation_status` to poll
// op.status — "PENDING" / "RUNNING" / "DONE"
// op.progress — Option<i32>, 0..=100

// Poll until done
let status = cdn.invalidation_status(&op.name).await?;
assert_eq!(status.status, "DONE");
```

| Env Var | Description |
|---|---|
| `GCP_CDN_PROJECT_ID` | GCP project hosting the URL map (required) |
| `GCP_CDN_URL_MAP` | Optional default URL map name |
| `GCP_CDN_BASE_URL` | Override the Compute API endpoint |

**Scope:** global URL maps only — regional URL maps
(`/regions/{region}/urlMaps/...`) are not supported and will return 404.

#### Google OAuth (Sign in with Google)

Server-side OAuth 2.0 + OpenID Connect: build the authorize URL, exchange the
returned code for tokens, verify the issued ID token. JWKS are cached in-memory
and key rotation is handled transparently — every algorithm other than RS256
is rejected.

```rust
use arche::gcp::oauth::{get_oauth_client, GcpOAuthConfig, Verifier, build_auth_url, exchange_code};

// All three fields fall back to GCP_OAUTH_CLIENT_ID / _CLIENT_SECRET / _REDIRECT_URI.
let oauth = get_oauth_client(
    GcpOAuthConfig::builder()
        .client_id("123.apps.googleusercontent.com")
        .client_secret("secret")
        .redirect_uri("https://app.example/auth/google/callback")
        .build(),
)?;

// 1. Redirect the user to Google's consent screen.
let url = build_auth_url(&oauth, &state, &pkce_challenge);

// 2. On callback, trade `code` for tokens.
let tokens = exchange_code(&oauth, &code, &pkce_verifier).await?;

// 3. Verify the ID token. `audiences` is the allow-list — your client_id(s).
let verifier = Verifier::new()?;
let claims = verifier
    .verify_id_token(&tokens.id_token, &["123.apps.googleusercontent.com"])
    .await?;
println!("verified sub={}", claims.sub);
```

| Env Var | Description |
|---|---|
| `GCP_OAUTH_CLIENT_ID` | OAuth 2.0 client ID |
| `GCP_OAUTH_CLIENT_SECRET` | OAuth 2.0 client secret |
| `GCP_OAUTH_REDIRECT_URI` | Registered redirect URI |

`Verifier` is cheap to construct and `Clone` — typically held once on
`AppState`. Verification failures (bad signature, wrong audience, expired,
unknown `kid` after a forced refresh) all surface as `AppError::Unauthorized`.
JWKS / token-endpoint outages surface as
`AppError::DependencyFailed { upstream: "gcp-oauth", … }`.

See [`docs/gcp/oauth.md`](docs/gcp/oauth.md) for the end-to-end flow diagram
and the Google Cloud Console setup steps.

#### Any other GCP REST API

`GcpClient` works for any Google API that accepts `Authorization: Bearer …`:

```rust
use arche::gcp::GcpClient;

let pubsub = GcpClient::new(
    Some(key),
    None,
    ["https://www.googleapis.com/auth/pubsub"],
).await?;

pubsub
    .post(format!("https://pubsub.googleapis.com/v1/projects/{p}/topics/{t}:publish"))
    .await?
    .json(&payload)
    .send().await?;
```

For full HTTP control (custom timeouts, TLS config, connection pool),
bring your own `reqwest::Client`:

```rust
let http = reqwest::Client::builder()
    .connect_timeout(std::time::Duration::from_secs(5))
    .build()?;

let storage = GcpClient::with_http(
    http,
    Some(key),
    None,
    ["https://www.googleapis.com/auth/devstorage.read_only"],
).await?;
```

One service account, multiple APIs — share a single token cache:

```rust
let drive   = arche::gcp::drive::client(Some(key), None).await?;
let sheets  = drive.with_scopes(["https://www.googleapis.com/auth/spreadsheets"]);
let storage = drive.with_scopes(["https://www.googleapis.com/auth/devstorage.read_only"]);
```

#### Vertex AI

`VertexClient` implements [`arche::llm::LlmProvider`](#llm) for **Gemini** and
**Anthropic Claude** models on Google Cloud. The provider (Gemini or Anthropic) is
captured at construction; the model is specified per-request.

```rust
use arche::gcp::vertex::{get_vertex_client, VertexConfig, VertexProvider};
use arche::gcp::ServiceAccountKey;
use arche::llm::{GenerateRequest, LlmProvider, Message, StreamChunk};

// Gemini via API key (resolved from VERTEX_API_KEY / GEMINI_API_KEY env)
let client = get_vertex_client(VertexProvider::Gemini, None).await?;

// Service-account auth (required for Anthropic, optional for Gemini)
let key = ServiceAccountKey::new(client_email, private_key);
let client = get_vertex_client(
    VertexProvider::Anthropic,
    Some(VertexConfig::default()
        .with_service_account_key(key)
        .with_project_id("my-project")
        .with_region("us-east5")),
).await?;

let request = GenerateRequest::new(
    "gemini-2.0-flash",
    vec![Message::user("Explain quantum computing in one sentence.")],
)
.with_system("You are a helpful assistant.")
.with_max_tokens(256)
.with_temperature(0.7);

// Non-streaming
let response = client.generate(&request).await?;
println!("{}", response.text().unwrap_or_default());

// Streaming
use futures::StreamExt;
let mut stream = client.stream_generate(&request).await?;
while let Some(chunk) = stream.next().await {
    match chunk? {
        StreamChunk::Text(text) => print!("{text}"),
        StreamChunk::ToolCall { name, arguments, .. } => { /* dispatch tool */ }
        StreamChunk::Done { finish_reason, usage } => {
            println!("\n[{finish_reason}] usage={usage:?}");
        }
    }
}
```

**Function calling** (typed schemas via `arche::llm::ParameterSchema`):

```rust
use arche::llm::{ParameterSchema, ToolDefinition};

let tools = vec![
    ToolDefinition::new("get_weather", "Get current weather for a city")
        .with_parameters(
            ParameterSchema::object()
                .with_property("city", ParameterSchema::string("City name"))
                .with_required(["city"]),
        ),
];

let request = GenerateRequest::new(
    "gemini-2.0-flash",
    vec![Message::user("What's the weather in Tokyo?")],
)
.with_tools(tools);
```

**Authentication:**

| Method | When | Source |
|---|---|---|
| API Key | Gemini only | `VertexConfig::with_api_key(...)` or `VERTEX_API_KEY` / `GEMINI_API_KEY` env |
| Service Account | Gemini + Anthropic | `VertexConfig::with_service_account_key(ServiceAccountKey)` or `with_service_account_key_path("/path/to/sa.json")` |

If an API key is present, it takes priority. Service account auth is required for
Anthropic models. `VERTEX_PROJECT_ID` / `VERTEX_REGION` env vars override
config; default region is `asia-south1`. Service-account credentials must be
passed via `VertexConfig` — arche does not auto-resolve `GOOGLE_APPLICATION_CREDENTIALS`.

**Token cache** — every GCP REST call goes through a process-local token
cache: JWT-bearer flow against `oauth2.googleapis.com/token`, signed RS256
with the service-account key, retried once on transient failures, refreshed
60 s before expiry, single-flighted per `(client_email, scopes)` pair.

---

### LLM

Canonical, provider-agnostic types and the `LlmProvider` trait that every backend
implements. Use it directly when you just want to call an LLM; build on top of it
when you want tool-calling orchestration (see [`agent`](#agent)).

```rust
use arche::llm::{GenerateRequest, LlmProvider, Message, ParameterSchema, ToolDefinition};

// `client` is anything implementing `LlmProvider` —
// VertexClient, or your own OpenAi/Bedrock/Ollama/local impl.
let request = GenerateRequest::new(
    "gemini-2.0-flash",
    vec![Message::user("Hello!")],
)
.with_system("Be concise.")
.with_temperature(0.3);

let response = client.generate(&request).await?;
```

**Types you'll use:**

| Type | Purpose |
|---|---|
| `LlmProvider` (trait) | `generate()` + `stream_generate()` on a canonical `GenerateRequest`. Implement this to add a backend. |
| `GenerateRequest` / `GenerateResponse` | Canonical request/response, provider-neutral |
| `Message`, `Role`, `ContentPart` | Conversation turns — text, tool calls, tool results |
| `StreamChunk` | `Text(String)` \| `ToolCall { id, name, arguments }` \| `Done { finish_reason, usage }` |
| `ToolDefinition` + `ParameterSchema` | Strictly-typed tool descriptions; serializes to valid JSON Schema |
| `Usage` | Token accounting (input/output/total) |

**Writing a custom backend:**

```rust
use arche::llm::{GenerateRequest, GenerateResponse, LlmProvider, LlmStream};
use arche::error::AppError;
use std::future::Future;
use std::pin::Pin;

pub struct OpenAiClient { /* http client, api key */ }

impl LlmProvider for OpenAiClient {
    fn generate<'a>(&'a self, request: &'a GenerateRequest)
        -> Pin<Box<dyn Future<Output = Result<GenerateResponse, AppError>> + Send + 'a>>
    { Box::pin(async move { /* POST, convert */ todo!() }) }

    fn stream_generate<'a>(&'a self, request: &'a GenerateRequest)
        -> Pin<Box<dyn Future<Output = Result<LlmStream, AppError>> + Send + 'a>>
    { Box::pin(async move { /* POST stream, convert SSE */ todo!() }) }
}
```

Drops into `arche::agent::get_agent_engine(my_client, config)` with no other changes.

---

### Agent

Tool-calling agent engine: orchestrates LLM rounds, invokes your tools, streams SSE
events to the client, manages session history (with optional compaction).

```rust
use arche::agent::{get_agent_engine, AgentConfig, AgentFlow, AgentSession, ToolOutput, to_sse_event};
use arche::gcp::vertex::{get_vertex_client, VertexProvider};
use arche::llm::{ParameterSchema, ToolDefinition};

struct ShoppingFlow;

impl AgentFlow for ShoppingFlow {
    fn system_prompt(&self) -> String {
        "You help shoppers find products.".into()
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition::new("search_catalog", "Search products by query")
                .with_parameters(
                    ParameterSchema::object()
                        .with_property("query", ParameterSchema::string("Query"))
                        .with_required(["query"]),
                ),
        ]
    }

    fn execute_tool<'a>(
        &'a self,
        name: &'a str,
        args: &'a serde_json::Value,
        _session: &'a AgentSession,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput, arche::error::AppError>> + Send + 'a>> {
        Box::pin(async move {
            // Run your business logic, return text for the LLM + optional data for the client
            Ok(ToolOutput::text("Found 3 matches")
                .data("product_list", serde_json::json!([/* ... */])))
        })
    }
}

// Wire it up
let client = get_vertex_client(VertexProvider::Gemini, None).await?;
let config = AgentConfig::builder("gemini-2.0-flash").build()?;
let engine = get_agent_engine(client, config)
    .with_default_summarizer("gemini-2.0-flash-lite"); // optional, cheap summarization

// Per request
let mut session = AgentSession::new("sess-1", "shopping");
let stream = engine.run(&ShoppingFlow, &mut session, "find red shoes");
// Map each SseEvent via `to_sse_event(..)` to an axum SSE Event.
```

**What arche provides vs. what you write:**

| Arche provides | You write |
|---|---|
| Orchestration loop, streaming, SSE event types, session mutation, tool-calling loop, history compaction | System prompt, tool schemas, tool executors (`impl AgentFlow`), HTTP handler, session persistence |

**Extension points:**

| Need | Plug point |
|---|---|
| Different LLM backend | `impl LlmProvider for YourClient` |
| Custom history compaction (vector recall, server-side memory) | `impl HistoryCompactor` |
| Custom UI events from tools | `ToolOutput::text(..).data(type, payload)` → reaches client via `SseEvent::Data` |

**Deeper reading:**

- [`docs/agent/architecture.md`](docs/agent/architecture.md) — module layering, component diagram with hover tooltips
- [`docs/agent/sequence.md`](docs/agent/sequence.md) — request lifecycle, error paths, SSE wire format
- [`docs/agent/extending.md`](docs/agent/extending.md) — step-by-step guides for each plug point

---

### Database

#### Postgres

Connection pooling with `sqlx`, configurable credentials, and health checks.

```rust
use arche::database::pg::{get_pg_pool, test_pg, PgConfigBuilder};

let pool = get_pg_pool(None).await?;
let is_healthy = test_pg(pool.clone()).await?;
```

| Env Var | Description |
|---|---|
| `PG_HOST` | Database host |
| `PG_PORT` | Database port |
| `PG_DATABASE` | Database name |
| `PG_MAX_CONN` | Maximum pool connections |
| `PG_USERNAME` | Username |
| `PG_PASSWORD` | Password |
| `PG_CREDENTIALS` | JSON string `{"username":"...","password":"..."}` (alternative to separate vars) |

#### Redis

Connection pooling with `bb8`, optional password auth, and health checks.

```rust
use arche::database::redis::{get_redis_pool, test_redis, RedisConfigBuilder};

let pool = get_redis_pool(None).await?;
let is_healthy = test_redis(pool.clone()).await?;
```

| Env Var | Description |
|---|---|
| `REDIS_HOST` | Redis host |
| `REDIS_PORT` | Redis port |
| `REDIS_MAX_CONN` | Maximum pool connections |
| `REDIS_PASSWORD` | Optional password |

---

### JWT

Token generation and verification using HS256.

```rust
use arche::jwt::{generate_tokens, verify_token, generate_expiry_time};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

// Generate an access + refresh token pair
let tokens = generate_tokens(
    Claims { sub: "user_123".into(), exp: generate_expiry_time(3600) },
    Claims { sub: "user_123".into(), exp: generate_expiry_time(86400) },
    &access_secret,
    &refresh_secret,
)?;

// Verify a token
let data = verify_token::<Claims>(&tokens.access_token, &access_secret, None)?;
```

---

### CSV

Async CSV processing powered by `csv-async`. Supports reading from bytes, files, and
URLs — with both batch and streaming modes.

```rust
use arche::csv::CsvClient;

// Default config (comma-delimited, with headers)
let csv = CsvClient::new();

// Or customize
let csv = CsvClient::new()
    .delimiter(b';')
    .has_headers(true)
    .flexible(true);
```

#### Reading

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct Record { name: String, age: u32, city: String }

// From bytes
let records: Vec<Record> = csv.read().from_bytes(data).deserialize().await?;

// From file
let records: Vec<Record> = csv.read().from_file("data.csv").deserialize().await?;

// From URL
let records: Vec<Record> = csv.read().from_url("https://example.com/data.csv")
    .deserialize().await?;

// Batch processing (memory-efficient for large files)
csv.read().from_file("large.csv")
    .deserialize_batched(1000, |batch: Vec<Record>| async move {
        // Process 1000 records at a time
        Ok(())
    }).await?;
```

#### Writing

```rust
use serde::Serialize;

#[derive(Serialize)]
struct Output { name: String, score: f64 }

let records = vec![
    Output { name: "Alice".into(), score: 95.5 },
    Output { name: "Bob".into(),   score: 87.0 },
];

// To bytes
let bytes: Vec<u8> = csv.write_all(&records).await?;

// To file
csv.write_file("output.csv", &records).await?;
```

#### Streaming

```rust
// Record-by-record reading
let mut stream = csv.read().from_file("large.csv").stream().await?;
while let Some(record) = stream.next_deserialized::<Record>().await {
    let record = record?;
}

// Record-by-record writing
let mut writer = csv.writer_to_file("output.csv").await?;
writer.serialize(&Output { name: "Alice".into(), score: 95.5 }).await?;
writer.finish().await?;
```

---

### JSON

Streaming JSON array parsing optimized for large payloads. Extracts metadata fields
before the target array and streams array elements one-by-one or in batches —
without loading the full document into memory.

```rust
use arche::json::JsonClient;
use serde::Deserialize;

#[derive(Deserialize)]
struct Item { id: u64, name: String }

let json = JsonClient::new();

// Stream a root-level JSON array from bytes
let source = json.from_bytes(data);
let mut stream = source.stream_root_array();

while let Some(item) = stream.next::<Item>().await {
    let item = item?;
}

// Stream a nested array with metadata capture
// Given: {"total": 1000, "items": [{...}, {...}, ...]}
let json = JsonClient::new();
let source = json.from_bytes(data);
let mut stream = source.stream_array("items").await;

while let Some(item) = stream.next::<Item>().await {
    let item = item?;
}
let total: u64 = stream.field("total")?;

// Batch iteration
let batch = stream.next_batch::<Item>(100).await;

// Stream directly from S3
let source = JsonClient::new().from_s3(&s3_client, "my-bucket", "data.json").await?;
let mut stream = source.stream_array("results").await;
```

---

### Crypto

AES-128-CBC encryption with PBKDF2-HMAC-SHA1 key derivation (65,536 iterations).

```rust
use arche::crypto::{encrypt_cbc, decrypt_cbc};

let secret = "my-secret-key";
let salt = "my-salt-value-16"; // minimum 16 bytes

// Encrypt — returns raw ciphertext bytes
let ciphertext = encrypt_cbc(secret, salt, "sensitive data")?;

// Decrypt — expects base64-encoded ciphertext input
let plaintext = decrypt_cbc(secret, salt, &base64_ciphertext)?;
```

---

### Sockets

WebSocket connection registry with broadcast support. Manages a thread-safe map of
active connections for fan-out messaging.

```rust
use arche::sockets::SocketConnectionManager;

let manager = SocketConnectionManager::new();

// Register a connection (typically in a WebSocket upgrade handler)
manager.add(&connection_id, sender)?;

// Broadcast to all connected clients
manager.broadcast("Hello, everyone!".into())?;

// List active connections
let ids = manager.get_connections()?;

// Remove a connection on disconnect
manager.remove(connection_id)?;
```

---

### Error

Axum-compatible structured error handling. Every variant converts to a JSON response
with the appropriate HTTP status code.

```rust
use arche::error::AppError;

async fn handler() -> Result<impl axum::response::IntoResponse, AppError> {
    Err(AppError::Unauthorized)
}
```

**Variants:**

| Variant | Status | Constructor |
|---|---|---|
| `BadRequest` | 400 | `AppError::bad_request(errors, message, description)` |
| `Unauthorized` | 401 | Direct construction |
| `Forbidden` | 403 | Direct construction |
| `NotFound` | 404 | `AppError::not_found("resource")` |
| `Conflict` | 409 | `AppError::conflict("message")` |
| `UnprocessableEntity` | 422 | `AppError::unprocessable_entity(errors, message, description)` |
| `DependencyFailed` | 424 | `AppError::dependency_failed("upstream", "detail")` |
| `InternalError` | 500 | `AppError::internal_error(error, message)` |
| `Unavailable` | 503 | Direct construction |

`InternalError` responses are **sanitized by default** — no leaked SQL or infra
details. Enable `verbose-errors` to expose raw error details (dev/staging only):

```toml
arche = { version = "4.5.1", features = ["verbose-errors"] }
```

---

### Utils

Date/time conversion traits and pagination helpers.

```rust
use arche::utils::{validate_timestamp, FromOffsetDateTime, PaginationParams};
use time::OffsetDateTime;

// Check if a timestamp is in the future
let is_valid = validate_timestamp(timestamp, false)?;

// Convert OffsetDateTime to ISO string
let iso = offset_dt.to_iso_string()?;

// Pagination query params (for Axum extractors)
let params = PaginationParams { page_number: Some(1), page_size: Some(20) };
```

---

## Re-exported Dependencies

arche re-exports these crates so you don't need to add them separately:

`axum` · `tokio` · `serde` · `serde_json` · `sqlx` · `time` · `tracing` · `tracing-subscriber` · `reqwest` · `jsonwebtoken` · `nanoid` · `thiserror` · `base64` · `bb8` · `bb8-redis` · `csv-async` · `futures` · `tokio-stream` · `dotenv` · `aws-config` · `aws-sdk-s3` · `aws-sdk-sesv2` · `aws-sdk-kms` · `aws-sdk-cloudfront`

---

## Design Principles

- **Explicit over implicit** — no hidden global state or magic
- **Composition over inheritance** — thin wrappers you combine as needed
- **Production-first defaults** — sane defaults, sanitized errors, pooled connections
- **Async-native** — built on Tokio from the ground up

## What arche is *not*

- A framework that replaces Axum
- A code generator or project template
- A monolithic abstraction over third-party libraries

---

## License

[MIT](LICENSE)
