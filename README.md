<div align="center">

# arche

**The opinionated backend foundation for Axum applications.**

[![Crates.io](https://img.shields.io/crates/v/arche.svg)](https://crates.io/crates/arche)
[![Documentation](https://img.shields.io/docsrs/arche)](https://docs.rs/arche)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Cloud integrations, databases, auth, LLM inference, tool-calling agents, encryption,
streaming JSON/CSV, WebSockets, and structured error handling — wired up and ready to go.

`arche` sits _around_ Axum, not in place of it.

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
arche = "4.15.0"
```

## Modules

| Module                  | What it does                                                                                                                                       |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`aws`](#aws)           | S3, SES, KMS, and CloudFront via official AWS SDKs                                                                                                 |
| [`gcp`](#gcp)           | Generic GCP REST client + **Vertex AI** (Gemini + Claude); wrappers for Sheets, Drive, Cloud KMS, Cloud Storage, Cloud CDN, and Google OAuth login |
| [`oidc`](#oidc)         | OpenID Connect both ways — _"Sign in with Google"_ client + build-your-own identity provider (authorization-code + PKCE, RS256)                    |
| [`llm`](#llm)           | Canonical LLM types + `LlmProvider` trait — backend-agnostic                                                                                       |
| [`agent`](#agent)       | Tool-calling agent engine, session state, SSE streaming                                                                                            |
| [`database`](#database) | Postgres, Redis, and ClickHouse connection pooling with health checks                                                                              |
| [`queue`](#queue) | Kafka producer/consumer with a raw message stream, explicit offset commits, and health checks                                                    |
| [`jwt`](#jwt)           | HS256 token generation, verification, and expiry helpers                                                                                           |
| [`csv`](#csv)           | Async CSV read/write — batch, streaming, and from URL                                                                                              |
| [`json`](#json)         | Streaming JSON array parsing with metadata extraction                                                                                              |
| [`crypto`](#crypto)     | AES-128-CBC encryption with PBKDF2 key derivation                                                                                                  |
| [`sockets`](#sockets)   | WebSocket connection registry with broadcast                                                                                                       |
| [`error`](#error)       | Axum-compatible structured error responses (400–503)                                                                                               |
| [`middleware`](#middleware) | Axum layers — rewrites extractor rejections into `AppError` JSON so clients see one error contract                                             |
| [`utils`](#utils)       | Alphanumeric nano IDs, timestamp validation, date/time conversions, pagination                                                                     |

> [!TIP]
> Every service module exports a **config builder** to wire up credentials
> programmatically — or omit it entirely (pass `None`) and let arche resolve
> everything from environment variables.

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

| Env Var                | Description                        |
| ---------------------- | ---------------------------------- |
| `S3_CRED_SOURCE`       | `"IAM"` (default) or `"env"`       |
| `S3_ACCESS_KEY_ID`     | Required when source is `"env"`    |
| `S3_SECRET_ACCESS_KEY` | Required when source is `"env"`    |
| `S3_REGION`            | AWS region (default: `ap-south-1`) |

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

| Env Var      | Description                        |
| ------------ | ---------------------------------- |
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

| Env Var      | Description                        |
| ------------ | ---------------------------------- |
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

> [!NOTE]
> `caller_reference: None` auto-generates a fresh nanoid on every call, so a
> retried request creates a **duplicate** invalidation. Pass a stable value for
> idempotent retries.

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

| Env Var                      | Description                        |
| ---------------------------- | ---------------------------------- |
| `AWS_REGION`                 | AWS region (default: `ap-south-1`) |
| `CLOUDFRONT_DISTRIBUTION_ID` | Optional default distribution ID   |

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

> [!WARNING]
> **Caveats for the metadata-server path:**
>
> - **Scopes are ignored** — the endpoint returns whatever scopes the pod's
>   bound service account has. Need narrower scopes? Use an explicit
>   `ServiceAccountKey`.
> - **`GCP_METADATA_URL` is read once, at construction.** Changing it later has
>   no effect — set it before the first `GcpClient::new(None, None, …)` call
>   (handy for pointing tests at a mock).
> - **Signed URLs are unsupported** — V4 signing (`GcsClient::sign_*`) needs the
>   SA's private key, which the metadata server never exposes. Those calls
>   return a clear error here.

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

> [!NOTE]
> Pass either `Some(key)` or `Some(path)` — never both. Scope is preset to
> `https://www.googleapis.com/auth/spreadsheets`.

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

| Env Var              | Description                                        |
| -------------------- | -------------------------------------------------- |
| `GCP_KMS_PROJECT_ID` | GCP project hosting the KMS key (required)         |
| `GCP_KMS_LOCATION`   | KMS location (default: `global`)                   |
| `GCP_KMS_BASE_URL`   | Override the Cloud KMS endpoint (testing / VPC-SC) |
| `GCP_KMS_KEY_RING`   | Used by `GcpKmsKey::from_env()`                    |
| `GCP_KMS_KEY_NAME`   | Used by `GcpKmsKey::from_env()`                    |

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
Bucket is per-call so one client can target many buckets.

> [!WARNING]
> Uploads and downloads **buffer the full object in memory** — fine for reports
> and assets, but route large media through a streaming path instead.

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

| Env Var                      | Description                                                               |
| ---------------------------- | ------------------------------------------------------------------------- |
| `GCS_BASE_URL`               | Override the storage endpoint (testing / VPC-SC; ignored for signed URLs) |
| `GCS_SIGNED_URL_EXPIRY_SECS` | Default expiry for `signed_get_url` (default: 900, max: 604800)           |

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

| Env Var              | Description                                |
| -------------------- | ------------------------------------------ |
| `GCP_CDN_PROJECT_ID` | GCP project hosting the URL map (required) |
| `GCP_CDN_URL_MAP`    | Optional default URL map name              |
| `GCP_CDN_BASE_URL`   | Override the Compute API endpoint          |

**Scope:** global URL maps only — regional URL maps
(`/regions/{region}/urlMaps/...`) are not supported and will return 404.

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

| Method          | When               | Source                                                                                                             |
| --------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------ |
| API Key         | Gemini only        | `VertexConfig::with_api_key(...)` or `VERTEX_API_KEY` / `GEMINI_API_KEY` env                                       |
| Service Account | Gemini + Anthropic | `VertexConfig::with_service_account_key(ServiceAccountKey)` or `with_service_account_key_path("/path/to/sa.json")` |

> [!NOTE]
> If an API key is present it takes priority; **Anthropic models require
> service-account auth**. `VERTEX_PROJECT_ID` / `VERTEX_REGION` override config
> (default region `asia-south1`). Service-account creds must be passed via
> `VertexConfig` — arche does **not** auto-resolve `GOOGLE_APPLICATION_CREDENTIALS`.

**Token cache** — every GCP REST call goes through a process-local token
cache: JWT-bearer flow against `oauth2.googleapis.com/token`, signed RS256
with the service-account key, retried once on transient failures, refreshed
60 s before expiry, single-flighted per `(client_email, scopes)` pair.

---

### OIDC

Both halves of OpenID Connect — speak **to** a provider, or **be** one.

|               | `arche::oidc` (client)                       | `arche::oidc::server`                                       |
| ------------- | -------------------------------------------- | ----------------------------------------------------------- |
| **Role**      | Relying party — _"Sign in with Google"_      | Identity provider — _"Sign in with your service"_           |
| **You get**   | authorize URL → code → **verified** ID token | validate → single-use code → **signed** ID token            |
| **You bring** | provider metadata + your config              | four small trait impls (registry · signer · tokens · store) |

Authorization-code flow with mandatory **PKCE** (S256), **RS256** ID tokens, and a
JWKS cache that handles key rotation transparently. The two halves never share
code — an interop test drives the client against the server over real HTTP to
prove they speak the same dialect.

> [!TIP]
> Full walkthrough, both sequence diagrams, and a single hover-tooltip canvas live in [`docs/oidc/`](docs/oidc/README.md).

#### Client — "Sign in with Google" (or any OIDC provider)

Provider endpoints come from a `ProviderMetadata`: the shipped `google()` preset,
a struct literal, or `ProviderMetadata::discover(...)` (fetches
`/.well-known/openid-configuration`).

```rust
use arche::gcp::oauth::google;
use arche::oidc::{OidcClient, OidcConfig, ProviderMetadata, Verifier};

// Build once, hold on state — both are Clone.
let client = OidcClient::new(
    google(),                                        // or ProviderMetadata::discover(...).await?
    OidcConfig {
        client_id: "123.apps.googleusercontent.com".into(),
        client_secret: "secret".into(),
        redirect_uri: "https://app.example/auth/google/callback".into(),
        scopes: None,                                // defaults to "openid email profile"
    },
)?;
let verifier = Verifier::new(client.provider())?;

// 1 ─ send the user to the provider's consent screen
let url = client.auth_url(&state, &pkce_challenge);

// 2 ─ on callback, trade the code for tokens
let tokens = client.exchange_code(&code, &pkce_verifier).await?;

// 3 ─ verify the ID token into YOUR claims type
#[derive(serde::Deserialize)]
struct Claims { sub: String, email: String, email_verified: bool }

let claims: Claims = verifier
    .verify_id_token(&tokens.id_token, &[client.client_id()])
    .await?;
```

> [!NOTE]
> `verify_id_token::<C>` validates RS256, the signing `kid` (against a rotation-aware
> JWKS cache), the signature, and `iss` / `aud` / `exp` / `iat` / `nbf` (60 s skew) —
> **then** deserializes into your `C`. A bad token is `AppError::Unauthorized` (the
> real reason is logged at debug, never leaked); an unreachable provider is
> `AppError::DependencyFailed`; a valid token that doesn't fit `C` is an internal
> error, not a 401. Nonce is yours to check — put it in `C` and compare.

#### Server — be your own identity provider

arche runs the protocol logic; it does **not** serve HTTP. You wire four routes and
call four methods. The server is generic over five capabilities — arche ships a
built-in **only where correctness is _math_, never _policy_**:

| Capability        | Trait               | Built-in                 | Bring your own when…                                                            |
| ----------------- | ------------------- | ------------------------ | ------------------------------------------------------------------------------- |
| Client resolution | `ClientRegistry`    | — _always yours_         | static list · DB · config service (override `verify_secret` for hashed secrets) |
| Token signing     | `TokenSigner`       | `SigningKey` (local RSA) | keys live in KMS / HSM, or you rotate                                           |
| Access tokens     | `AccessTokenIssuer` | — _always yours_         | opaque random · your own JWT · a stored token                                   |
| Code storage      | `CodeStore`         | — _always yours_         | in-memory (single node) · Redis `GETDEL` · PG `DELETE…RETURNING`                |
| Refresh tokens    | `RefreshTokenStore` | — _always yours_         | any store with atomic delete-on-read — same shape as `CodeStore`                |

<details>
<summary><b>1 &middot; Implement your five seams</b> — click to expand</summary>

```rust
use arche::error::AppError;
use arche::oidc::server::{
    AccessTokenIssuer, ClientRegistration, ClientRegistry, CodeStore, IssuedAccessToken,
    PendingGrant, RefreshTokenStore, SigningKey,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// Signer: a local RSA key is the one built-in. Persist it — a fresh key
// invalidates every issued ID token.  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048
let key = SigningKey::from_pem("2026-07-key", &std::fs::read_to_string("key.pem")?)?;

// Registry: resolve partner apps. `verify_secret` is defaulted (constant-time
// plaintext); override it when you store hashed secrets.
struct StaticClients(Vec<ClientRegistration>);
impl ClientRegistry for StaticClients {
    async fn find(&self, id: &str) -> Result<Option<ClientRegistration>, AppError> {
        Ok(self.0.iter().find(|c| c.client_id == id).cloned())
    }
}

// Access token: opaque noise is fine when only the ID token is consumed;
// mint a real JWT when something must verify it.
struct OpaqueTokens;
impl AccessTokenIssuer for OpaqueTokens {
    async fn issue(&self, _g: &PendingGrant) -> Result<IssuedAccessToken, AppError> {
        Ok(IssuedAccessToken { token: arche::utils::nano_id_of(43), expires_in: 3600 })
    }
}

// Code store: `take` MUST be atomic delete-on-read (the single-use guarantee).
// In-memory works on one node; behind a load balancer use Redis GETDEL / PG.
#[derive(Default)]
struct MemStore(Mutex<HashMap<String, (PendingGrant, Instant)>>);
impl CodeStore for MemStore {
    async fn put(&self, code: String, g: PendingGrant, ttl: Duration) -> Result<(), AppError> {
        let exp = Instant::now().checked_add(ttl).unwrap_or_else(Instant::now);
        self.0.lock().await.insert(code, (g, exp));
        Ok(())
    }
    async fn take(&self, code: &str) -> Result<Option<PendingGrant>, AppError> {
        Ok(self.0.lock().await.remove(code)
            .filter(|(_, exp)| *exp > Instant::now())
            .map(|(g, _)| g))
    }
}

// Refresh store: identical shape — `take` is delete-on-read, giving rotation +
// reuse-invalidation for free. Only touched when a client is granted `offline_access`.
#[derive(Default)]
struct MemRefreshStore(Mutex<HashMap<String, (PendingGrant, Instant)>>);
impl RefreshTokenStore for MemRefreshStore {
    async fn put(&self, token: String, g: PendingGrant, ttl: Duration) -> Result<(), AppError> {
        let exp = Instant::now().checked_add(ttl).unwrap_or_else(Instant::now);
        self.0.lock().await.insert(token, (g, exp));
        Ok(())
    }
    async fn take(&self, token: &str) -> Result<Option<PendingGrant>, AppError> {
        Ok(self.0.lock().await.remove(token)
            .filter(|(_, exp)| *exp > Instant::now())
            .map(|(g, _)| g))
    }
}
```

</details>

**2 &middot; Build the server** — once, at startup:

```rust
use arche::oidc::server::{OidcServer, OidcServerConfig};

let server = OidcServer::new(
    OidcServerConfig {
        issuer: "https://id.example.com".into(),     // https; becomes `iss` + endpoint prefix
        code_ttl: None,           // 5 min
        id_token_ttl: None,       // 1 h
        refresh_token_ttl: None,  // 30 days
        // Add "offline_access" here to enable refresh tokens for clients that request it:
        allowed_scopes: None,     // ["openid", "email", "profile"]
    },
    clients, key, OpaqueTokens, MemStore::default(), MemRefreshStore::default(),
)?;
```

**3 &middot; Wire four routes** — each is one method call:

```rust
use arche::oidc::server::{DiscoveryDocument, OidcServerError, TokenRequest};

// GET /.well-known/openid-configuration
let mut doc = DiscoveryDocument::standard(server.issuer());   // all fields pub — override to taste
Json(doc)

// GET /jwks   (send Cache-Control: public, max-age=3600)
Json(server.jwks_document())

// GET /authorize
let validated = match server.validate_authorize(&params).await {
    Ok(v) => v,
    Err(e) if e.redirectable() =>
        return redirect(e.redirect_url(&params.redirect_uri, params.state.as_deref())?),
    Err(e) => return bad_request(e.to_string()),   // unknown client / bad redirect — NEVER redirect
};
match session_user(&headers) {
    Some(u) => redirect(server.issue_code(validated, u.id, u.claims()).await?),
    None    => redirect("/login"),                 // stash `validated`, resume after login
}

// POST /token   (send Cache-Control: no-store on success)
let req = TokenRequest { /* form fields */,
    basic_auth: auth_header.and_then(TokenRequest::parse_basic_authorization) };
match server.exchange(req).await {
    Ok(payload)                        => json_no_store(payload),
    Err(OidcServerError::Internal(_))  => server_error(),     // 5xx — infra, retryable
    Err(e)                             => token_error(e.error_code()),   // 400 / 401
}
```

> [!IMPORTANT]
> **One instance, cloned per request — never one-per-request.** Build the `OidcServer`
> once at startup and keep it on state; `.clone()` is a single refcount bump. This isn't
> just efficiency: the `CodeStore` spans **two** requests — `/authorize` calls `put(code)`,
> and `/token` (a _different_ request, seconds later) calls `take(code)`. A server rebuilt
> per request hands the second call an empty store, and every login fails with `invalid_grant`.

> [!TIP]
> Keep it **isolated**: hold the server in its own service state, reached through the `State`
> extractor — no shared god-object, no middleware required. Give each service its own state
> and compose routers; reserve shared state for truly-global primitives (DB pools, secrets).

**Claims are yours; protocol claims are arche's.** `issue_code(request, subject, claims)`
takes the `sub` (1–255 ASCII) and any `Serialize` claims and mints them verbatim —
**except** the reserved set (`iss`, `sub`, `aud`, `iat`, `exp`, `nbf`, `nonce`, `jti`,
`azp`, `at_hash`, `c_hash`), which arche strips from your input and stamps itself.
`auth_time` is deliberately _not_ reserved — you assert it (and must, to honor a
`max_age` request). To resume after a login page, round-trip the serializable
`ValidatedAuthorizeRequest` back into `issue_code`; it re-checks `client_id` /
`redirect_uri` against the registry, so a tampered stash is rejected, not used.

> [!TIP]
> **Refresh tokens** are opt-in per client via the `offline_access` scope. Add
> `offline_access` to `allowed_scopes`; when a client requests it and you grant it,
> `exchange` returns a `refresh_token` alongside the ID token. A subsequent
> `grant_type=refresh_token` call **rotates** it — arche re-mints the ID/access
> tokens (dropping the one-time `nonce`, keeping your original claims) and issues a
> fresh refresh token, invalidating the old one via the store's atomic `take`. A
> replayed old token gets `invalid_grant`. Claims are snapshotted at authorization
> time — arche has no user model to re-fetch them.

> [!WARNING]
> Not supported, by design: the client-credentials grant and `/userinfo` (claims
> ride in the ID token). Key rotation is a `TokenSigner` choice, not a limitation.

**Deeper reading:**

- [`docs/oidc/README.md`](docs/oidc/README.md) — index for both halves
- [`docs/oidc/architecture.md`](docs/oidc/architecture.md) — the two halves, component diagram, five seams, where each defense lives
- [`docs/oidc/sequence.md`](docs/oidc/sequence.md) — login flow both directions, what `state` / PKCE defend, error + wire tables
- [`docs/oidc/extending.md`](docs/oidc/extending.md) — client & server quickstarts, code for each of the five traits

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

**Single calls — `one_shot`:**

```rust
use arche::llm::{one_shot, GenerateRequest};

// text out
let text = one_shot(&client, &GenerateRequest::one_shot(model, system, prompt))
    .await?
    .text();                       // Option<String>

// typed out — the tool's parameters describe T, and the model is asked to call it
let guess: Detection = one_shot(
    &client,
    &GenerateRequest::one_shot(model, system, prompt).with_tools(vec![report_tool()]),
)
.await?
.parse()?;                          // first tool call's arguments
```

`GenerateRequest::one_shot` is the single-turn shape: one user message, temperature 0.
Every option is a request builder — `with_tools`, `with_max_tokens`, `with_web_fetch`
(lets the model fetch URLs named in the prompt; Gemini only, and combining it with
custom tools is Gemini-3-only), `with_thinking_budget` (`0` disables Gemini thinking).
Responses are plain `GenerateResponse`: `text()`, `tool_calls()`, `parse::<T>()`,
`usage`, `stop_reason`.

**Types you'll use:**

| Type                                   | Purpose                                                                                               |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `LlmProvider` (trait)                  | `generate()` + `stream_generate()` on a canonical `GenerateRequest`. Implement this to add a backend. |
| `GenerateRequest` / `GenerateResponse` | Canonical request/response, provider-neutral                                                          |
| `Message`, `Role`, `ContentPart`       | Conversation turns — text, tool calls, tool results                                                   |
| `StreamChunk`                          | `Text(String)` \| `ToolCall { id, name, arguments }` \| `Done { finish_reason, usage }`               |
| `ToolDefinition` + `ParameterSchema`   | Strictly-typed tool descriptions; serializes to valid JSON Schema                                     |
| `Usage`                                | Token accounting (input/output/total)                                                                 |
| `one_shot`                             | One call on a prepared request; returns `GenerateResponse`                                            |

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

| Arche provides                                                                                          | You write                                                                                         |
| ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Orchestration loop, streaming, SSE event types, session mutation, tool-calling loop, history compaction | System prompt, tool schemas, tool executors (`impl AgentFlow`), HTTP handler, session persistence |

**Extension points:**

| Need                                                          | Plug point                                                                       |
| ------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Different LLM backend                                         | `impl LlmProvider for YourClient`                                                |
| Custom history compaction (vector recall, server-side memory) | `impl HistoryCompactor`                                                          |
| Custom UI events from tools                                   | `ToolOutput::text(..).data(type, payload)` → reaches client via `SseEvent::Data` |

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

| Env Var          | Description                                                                      |
| ---------------- | -------------------------------------------------------------------------------- |
| `PG_HOST`        | Database host                                                                    |
| `PG_PORT`        | Database port                                                                    |
| `PG_DATABASE`    | Database name                                                                    |
| `PG_MAX_CONN`    | Maximum pool connections                                                         |
| `PG_USERNAME`    | Username                                                                         |
| `PG_PASSWORD`    | Password                                                                         |
| `PG_CREDENTIALS` | JSON string `{"username":"...","password":"..."}` (alternative to separate vars) |

#### Redis

Connection pooling with `bb8`, optional password auth, and health checks.

```rust
use arche::database::redis::{get_redis_pool, test_redis, RedisConfigBuilder};

let pool = get_redis_pool(None).await?;
let is_healthy = test_redis(pool.clone()).await?;
```

| Env Var          | Description              |
| ---------------- | ------------------------ |
| `REDIS_HOST`     | Redis host               |
| `REDIS_PORT`     | Redis port               |
| `REDIS_MAX_CONN` | Maximum pool connections |
| `REDIS_PASSWORD` | Optional password        |

#### ClickHouse

Read-only connection pooling with `bb8` (round-robin across replicas) and a
typed row API. SQL templates are `&'static str` — a compile-time check that
prevents user input from being concatenated into a query.

```rust
use arche::database::clickhouse::{
    get_clickhouse_pool, ClickHousePoolExt, Row, Deserialize,
};

let pool = get_clickhouse_pool(None).await?;
let conn = pool.get_conn().await?;

#[derive(Row, Deserialize)]
struct EventCount { event: String, n: u64 }

let counts: Vec<EventCount> = conn
    .query("SELECT event, count() AS n FROM events WHERE day = ? GROUP BY event")
    .bind("2026-05-25")
    .fetch_all().await?;
```

Notes:

- Bare `SELECT *` / `SELECT t.*` are blocked. Call `.allow_select_star()` on a
  query, set `.allow_select_star(true)` on the config, or set
  `CLICKHOUSE_ALLOW_SELECT_STAR=true` to bypass.
- Writes go through Kafka → Kafka Connect ClickHouse Sink, not this connector.

> [!WARNING]
> `query` / `execute` take `&'static str` on purpose — user input can't be
> concatenated into the SQL. Runtime-built SQL is still possible via
> `query_dynamic(String)` / `execute_dynamic(String)`, but those **shift
> injection-safety onto you**.

| Env Var                            | Description                                         | Default                      |
| ---------------------------------- | --------------------------------------------------- | ---------------------------- |
| `CLICKHOUSE_HOSTS`                 | Comma-separated replica hostnames                   | — (required)                 |
| `CLICKHOUSE_HOST`                  | Single-host fallback if `CLICKHOUSE_HOSTS` is unset | —                            |
| `CLICKHOUSE_PORT`                  | Server port                                         | 8443 (secure) / 8123 (plain) |
| `CLICKHOUSE_DATABASE`              | Default database                                    | `default`                    |
| `CLICKHOUSE_USERNAME`              | Username                                            | `default`                    |
| `CLICKHOUSE_PASSWORD`              | Password                                            | (empty)                      |
| `CLICKHOUSE_SECURE`                | HTTPS toggle                                        | `true`                       |
| `CLICKHOUSE_MAX_POOL_SIZE`         | Max pool connections                                | `32`                         |
| `CLICKHOUSE_CONNECTION_TIMEOUT_MS` | Pool-acquire timeout                                | `5000`                       |
| `CLICKHOUSE_REQUEST_TIMEOUT_MS`    | Per-request `max_execution_time`                    | `30000`                      |
| `CLICKHOUSE_COMPRESSION`           | `lz4` or `none`                                     | `none`                       |
| `CLICKHOUSE_ALLOW_SELECT_STAR`     | Global `SELECT *` escape hatch                      | `false`                      |

---

### Queue

#### Kafka

A thin wrapper around [`rdkafka`](https://docs.rs/rdkafka) providing a
convenience producer, a raw parsed-message consumer stream, and a broker
health check. Unlike a typical high-level consumer wrapper, batching and
commit timing are not owned by arche — callers decide that policy entirely,
for example using `tokio_stream::StreamExt::chunks_timeout`.

Kafka configuration is always explicit — no field is ever resolved from
the process environment. Configuration is split into three types:
`KafkaConnectionConfig` (fields shared by every role: `brokers`,
`socket_timeout_ms`, `extra_options`), `KafkaProducerConfig` (embeds
`KafkaConnectionConfig` plus `topic`/`message_timeout_ms`), and
`KafkaConsumerConfig` (embeds `KafkaConnectionConfig` plus
`topics`/`group_id`/etc.) — each builder exposes the common methods
(`.broker()`, `.socket_timeout_ms()`, `.extra_option()`, ...) directly, so
there's no nesting required at the call site.

```rust
use arche::queue::kafka::{
    get_kafka_producer, get_kafka_consumer, test_kafka,
    KafkaProducerConfig, KafkaConsumerConfig, CommitMode,
};
use arche::tokio_stream::StreamExt;
use std::time::Duration;

// Producer and consumer configs are fully independent types.
let producer_config = KafkaProducerConfig::builder()
    .broker("localhost:9092")
    .topic("orders")
    .build();

let consumer_config = KafkaConsumerConfig::builder()
    .broker("localhost:9092")
    .topic("orders") // or .topics(["orders", "returns"]) for multiple
    .group_id("orders-service")
    .build();

// Producer
let producer = get_kafka_producer(producer_config.clone()).await?;
producer.send_json("order-123", &serde_json::json!({ "event": "order_placed" })).await?;

// Consumer — caller owns batching policy; here, up to 100 messages or every 5s
let consumer = get_kafka_consumer(consumer_config).await?;
let mut batches = consumer.messages().chunks_timeout(100, Duration::from_secs(5));
tokio::pin!(batches);
while let Some(batch) = batches.next().await {
    let mut last_ok = None;
    let parsed: Vec<_> = batch
        .into_iter()
        .filter_map(|r| match r {
            Ok(msg) => {
                last_ok = Some(msg.clone());
                Some((msg.key, msg.value))
            }
            Err(e) => {
                tracing::warn!(?e, "skipping message");
                None
            }
        })
        .collect();

    for (key, message) in &parsed {
        println!("key={key}, message={message}");
    }

    if let Some(msg) = last_ok {
        consumer.commit(&msg, CommitMode::Async)?;
    }
}

// Health check — reuses the `connection` config already embedded in the
// producer config, so there's no need to build a third config from scratch.
let is_healthy = test_kafka(producer_config.connection).await?;
```

Notes:

- `KafkaProducerConfig`/`KafkaConsumerConfig` are deliberately separate
  types (each embedding a shared `KafkaConnectionConfig`), rather than one
  struct with fields only some roles read — this avoids the ambiguity of a
  producer silently needing to pick just one topic out of a list meant for
  a consumer's subscriptions, and makes it a compile error to pass a
  consumer's config where a producer's is expected (or vice versa).
- `socket_timeout_ms` (default `5000`) applies to all three roles
  (producer, consumer, and `test_kafka`) via `KafkaConnectionConfig`. Note this
  is a **behavior change from librdkafka's own bare default of 60000ms** —
  producer/consumer requests will now time out after 5s by default instead
  of 60s; override via `.socket_timeout_ms(60_000)` if you need the longer
  window.
- Offsets are committed manually by default (`enable.auto.commit=false`,
  `auto.offset.reset=earliest` by default) only when the caller explicitly
  calls `commit`, giving full control over delivery semantics and an
  at-least-once guarantee (a message is never marked done until your code
  says so).
- Optionally, `KafkaConsumerConfigBuilder::auto_commit(true)` re-enables
  librdkafka's periodic background auto-commit (`auto_commit_interval_ms`
  controls the frequency, default `5000`). **Tradeoff:** with auto-commit
  enabled, an offset becomes eligible for commit as soon as the message is
  handed to your code via `messages()` — not when your processing logic
  finishes — so a crash between those two points can silently skip a
  message on redelivery (at-most-once), unlike the default manual-commit
  flow (at-least-once). `KafkaConsumer::commit()` remains callable
  regardless, if you want to force an earlier commit alongside the
  background timer.
- `KafkaConsumer::is_rebalancing()` reports whether a partition rebalance
  is currently in progress; this is informational only and not enforced —
  callers may check it if they want to defer committing during
  reassignment.
- Null/empty keys and values are rejected on the producer side (`send_json`,
  `send_json_to_topic`, and `send_bytes` all return `AppError::BadRequest`
  for an empty key, a `null`/empty-string JSON value, or an empty byte
  payload — nothing is sent to the broker). The consumer applies the same
  rule defensively: any message with an empty key or a `null`/empty-string
  value is skipped (logged as a warning) rather than yielded from
  `messages()`, protecting against messages written by other, non-arche
  producers into the same topic.
- SASL/SSL and other librdkafka settings (e.g. `security.protocol`,
  `sasl.mechanism`, `sasl.username`) are not exposed as dedicated config
  fields — pass them via `.extra_option(...)` / `.extra_options([...])`
  (available on all three builders), which map directly to librdkafka's
  `key=value` client settings.

---

### JWT

Token generation and verification using HS256.

> [!NOTE]
> This is symmetric **HS256** for your app's own access / refresh tokens. For
> asymmetric **RS256** ID tokens in an OIDC flow (signing or verifying "Sign in
> with…" tokens), see [`oidc`](#oidc) — different keys, different purpose.

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

> [!NOTE]
> The `salt` must be **≥ 16 bytes**. This is a fixed CBC + PBKDF2-SHA1 scheme;
> when you own both ends and want authenticated encryption, an AEAD cipher
> (AES-GCM) is the stronger default.

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

| Variant               | Status | Constructor                                                    |
| --------------------- | ------ | -------------------------------------------------------------- |
| `BadRequest`          | 400    | `AppError::bad_request(errors, message, description)`          |
| `Unauthorized`        | 401    | Direct construction                                            |
| `Forbidden`           | 403    | Direct construction                                            |
| `NotFound`            | 404    | `AppError::not_found("resource")`                              |
| `Conflict`            | 409    | `AppError::conflict("message")`                                |
| `UnprocessableEntity` | 422    | `AppError::unprocessable_entity(errors, message, description)` |
| `DependencyFailed`    | 424    | `AppError::dependency_failed("upstream", "detail")`            |
| `InternalError`       | 500    | `AppError::internal_error(error, message)`                     |
| `Unavailable`         | 503    | Direct construction                                            |

`InternalError` responses are **sanitized by default** — no leaked SQL or infra
details.

> [!WARNING]
> The `verbose-errors` feature echoes raw error details into responses. Enable
> it in dev / staging only — **never in production**.

```toml
arche = { version = "4.10.0", features = ["verbose-errors"] }
```

---

### Middleware

Axum layers that keep the wire format consistent with [`AppError`](#error).

**`extractor_rejection`** — by default, when an axum extractor fails
(malformed JSON body, missing `Content-Type`, wrong field type, oversized
payload), axum replies with a plain-text 400/413/415/422 before your handler
runs — bypassing the `AppError` shape entirely. This layer intercepts those
rejection responses and rewrites them as `AppError::bad_request` JSON, so
clients see one error contract everywhere. Handler-authored responses
(already JSON) pass through untouched.

```rust
use axum::{middleware::from_fn, Router};

let app: Router = Router::new()
    // ...routes...
    .layer(from_fn(arche::middleware::extractor_rejection));
```

```json
// POST /plugin with body `{}` (missing field) now returns 400:
{
  "error_values": null,
  "message": "Failed to deserialize the JSON body into the target type: missing field `plugin_id` at line 1 column 2",
  "description": null
}
```

Add the layer **before** any logging/tracing layers (inner-most position) so
observability middleware sees the rewritten response.

---

### Utils

ID generation, date/time conversion traits, and pagination helpers.

#### Nano IDs

URL-safe, **strictly alphanumeric** unique IDs (`0-9 a-z A-Z` — no `-` or `_`),
so they're safe in subdomains, file names, and anywhere symbol characters
cause friction:

```rust
use arche::utils::{nano_id, nano_id_of};

// 21 characters — same collision resistance class as a standard nanoid
let id = nano_id();          // e.g. "V1StGXR8Z5jdHi6BmyT9k"

// Custom length
let short = nano_id_of(8);   // e.g. "fX3kQ9aZ"
```

#### Date/time & pagination

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

`axum` · `tokio` · `serde` · `serde_json` · `sqlx` · `time` · `tracing` · `tracing-subscriber` · `reqwest` · `jsonwebtoken` · `nanoid` · `thiserror` · `base64` · `bb8` · `bb8-redis` · `clickhouse` (as `ch_client`) · `rdkafka` · `csv-async` · `futures` · `tokio-stream` · `dotenv` · `aws-config` · `aws-sdk-s3` · `aws-sdk-sesv2` · `aws-sdk-kms` · `aws-sdk-cloudfront`

---

## Design Principles

- **Explicit over implicit** — no hidden global state or magic
- **Composition over inheritance** — thin wrappers you combine as needed
- **Production-first defaults** — sane defaults, sanitized errors, pooled connections
- **Async-native** — built on Tokio from the ground up

## What arche is _not_

- A framework that replaces Axum
- A code generator or project template
- A monolithic abstraction over third-party libraries

---

## License

[MIT](LICENSE)
