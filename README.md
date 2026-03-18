# arche

**arche** is an opinionated backend foundation crate for building production-ready
applications with **Axum**.

It provides a curated set of building blocks commonly required in modern backend
services—cloud integrations, databases, authentication, middleware, and logging—
so you can focus on business logic instead of repetitive infrastructure wiring.

`arche` is designed to *sit around Axum*, not replace it.

## Why arche?

Most backend services end up re-implementing the same infrastructure concerns:

- Cloud SDK setup and ergonomics
- Database connection management
- Authentication primitives
- Middleware patterns
- Logging and tracing configuration
- Common error handling

**arche** brings these pieces together into a cohesive, Rust-native foundation,
built on top of well-established libraries and SDKs.

## What arche provides

### `aws`

AWS SDK integrations built on official SDKs:

- **S3**: Client initialization with support for IAM roles or environment-based credentials
- **SES**: Email sending with SES, including templated emails
- **KMS**: Key Management Service for encryption/decryption operations

### `gcp`

Google Cloud Platform integrations:

- **Drive**: Google Drive client with service account authentication
- **Sheets**: Google Sheets client with service account authentication

### `database`

Database connection management:

- **Postgres**: Connection pooling with `sqlx`, configurable credentials, health checks
- **Redis**: Connection pooling with `bb8`, async operations, health checks

### `jwt`

JWT utilities for authentication and authorization:

- Token generation and verification (HS256)
- Access/refresh token pair generation
- Token expiry helpers
- Custom claims support

### `csv`

Async CSV processing via a single reusable `CsvClient`:

- **Batch**: `read_all`, `read_file`, `write_all`, `write_file` — load everything at once
- **Streaming**: `reader` / `writer` factories for memory-efficient record-by-record I/O
- Configurable delimiter, quoting, escaping, headers, and more

### `error`

Axum-compatible error handling:

- `AppError` enum with HTTP error variants covering 400, 401, 403, 404, 409, 422, 424, 500, and 503
- Automatic `IntoResponse` conversion with structured JSON bodies
- `InternalError` responses are sanitized by default (no leaked SQL, infra details)
- Optional `verbose-errors` feature flag for dev/staging diagnostics
- `DependencyFailed` variant for upstream service failures (OpenSearch, Shopify, S3, etc.)

### `utils`

Common utilities for backend services:

- Timestamp validation and conversion helpers
- `OffsetDateTime` utilities (Unix, ISO8601)
- Pagination parameter types

All components are modular and explicit—nothing is hidden or magical.

## Module Reference

### AWS (`arche::aws`)

#### S3

Initialize an S3 client with automatic credential management:

```rust
use arche::aws::s3::get_s3_client;

let client = get_s3_client().await;
```

**Environment Variables:**

- `S3_CRED_SOURCE`: `"IAM"` (default) or `"env"` for environment-based credentials
- `S3_ACCESS_KEY_ID`: Required when using `"env"` credential source
- `S3_SECRET_ACCESS_KEY`: Required when using `"env"` credential source

#### KMS

Encrypt and decrypt data using AWS Key Management Service:

```rust
use arche::aws::kms::{get_kms_client, KMSClient};

// Initialize with default region (ap-south-1)
let client = get_kms_client().await;
let kms = KMSClient::new(client);

// Or with a specific region
let kms = KMSClient::new_with_region("us-east-1").await;

// Encrypt data
let plaintext = b"sensitive data";
let ciphertext = kms.encrypt("alias/my-key", plaintext).await?;

// Decrypt data
let decrypted = kms.decrypt(&ciphertext).await?;
```

**Credentials:** Uses IAM role credentials by default (recommended for EC2/ECS/Lambda).

### GCP (`arche::gcp`)

#### Drive

```rust
use arche::gcp::drive::get_drive_client;

let drive = get_drive_client().await?;
```

**Environment Variables:**

- `GCP_DRIVE_KEY`: Path to service account JSON key file

#### Sheets

```rust
use arche::gcp::sheets::get_sheets_client;

let sheets = get_sheets_client().await?;
```

**Environment Variables:**

- `GCP_SHEETS_KEY`: Path to service account JSON key file

### Database (`arche::database`)

#### Postgres

```rust
use arche::database::pg::{get_pg_pool, test_pg};

let pool = get_pg_pool().await;
let is_healthy = test_pg(pool.clone()).await;
```

**Environment Variables:**

- `PG_HOST`: Database host
- `PG_PORT`: Database port
- `PG_DATABASE`: Database name
- `PG_MAX_CONN`: Maximum connections in pool
- `PG_CREDENTIALS`: JSON string with `username` and `password` (alternative)
- `PG_USERNAME`: Username (if not using `PG_CREDENTIALS`)
- `PG_PASSWORD`: Password (if not using `PG_CREDENTIALS`)

#### Redis

```rust
use arche::database::redis::{get_redis_pool, test_redis};

let pool = get_redis_pool().await;
let is_healthy = test_redis(pool.clone()).await;
```

**Environment Variables:**

- `REDIS_HOST`: Redis host
- `REDIS_PORT`: Redis port
- `REDIS_MAX_CONN`: Maximum connections in pool

### JWT (`arche::jwt`)

```rust
use arche::jwt::{generate_tokens, verify_token, generate_expiry_time};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

// Generate tokens
let access_claims = Claims { sub: "user_id".into(), exp: generate_expiry_time(3600) };
let refresh_claims = Claims { sub: "user_id".into(), exp: generate_expiry_time(86400) };

let tokens = generate_tokens(
    access_claims,
    refresh_claims,
    &access_secret,
    &refresh_secret,
);

// Verify token
let token_data = verify_token::<Claims>(&token, secret, Some("audience".into()))?;
```

### CSV (`arche::csv`)

Async CSV processing powered by `csv-async`. Create one `CsvClient`, reuse it everywhere:

```rust
use arche::csv::CsvClient;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Record {
    name: String,
    age: u32,
    city: String,
}

// Default config (comma-delimited, with headers)
let csv = CsvClient::new();

// Or customize
let csv = CsvClient::new()
    .delimiter(b';')
    .has_headers(true)
    .flexible(true);
```

#### Batch reading

```rust
// From bytes
let data = b"name,age,city\nAlice,30,NYC\nBob,25,LA";
let records: Vec<Record> = csv.read_all(data.as_slice()).await?;

// From a file
let records: Vec<Record> = csv.read_file("data.csv").await?;

// Raw string records (no serde)
let raw_records = csv.read_records(data.as_slice()).await?;
```

#### Batch writing

```rust
#[derive(Serialize)]
struct Output {
    name: String,
    score: f64,
}

let records = vec![
    Output { name: "Alice".into(), score: 95.5 },
    Output { name: "Bob".into(), score: 87.0 },
];

// Write to in-memory bytes
let bytes: Vec<u8> = csv.write_all(&records).await?;

// Write to a file
csv.write_file("output.csv", &records).await?;
```

#### Streaming (memory-efficient)

```rust
// Record-by-record reading
let mut stream = csv.reader_from_file("large.csv").await?;
while let Some(result) = stream.next_deserialized::<Record>().await {
    let record = result?;
    // process one record at a time
}

// Record-by-record writing
let mut writer = csv.writer_to_file("output.csv").await?;
writer.serialize(&Output { name: "Alice".into(), score: 95.5 }).await?;
writer.write_fields(["Bob", "87.0"]).await?;
writer.finish().await?;
```

### Error (`arche::error`)

```rust
use arche::error::AppError;
use axum::response::IntoResponse;

async fn handler() -> Result<impl IntoResponse, AppError> {
    Err(AppError::Unauthorized)
}

// 400 — bad request with details
let error = AppError::bad_request(
    Some(errors_map),
    Some("Invalid input".into()),
    Some("Field validation failed".into()),
);

// 404 — resource not found
let error = AppError::not_found("client");

// 409 — unique constraint violation
let error = AppError::conflict("A client with this name already exists");

// 424 — upstream dependency failed (retryable)
let error = AppError::dependency_failed("opensearch", "index timeout");

// 424 — upstream dependency failed (permanent)
let error = AppError::dependency_failed_permanent("shopify", "invalid API key");

// 500 — internal error (response body is sanitized by default)
let error = AppError::internal_error("SQL error: ...".into(), None);
```

**Error Variants:**

| Variant | Status | Constructor |
|---|---|---|
| `BadRequest` | 400 | `bad_request(errors, message, description)` |
| `Unauthorized` | 401 | Direct construction |
| `Forbidden` | 403 | Direct construction |
| `NotFound` | 404 | `not_found(resource)` |
| `Conflict` | 409 | `conflict(message)` |
| `UnprocessableEntity` | 422 | `unprocessable_entity(errors, message, description)` |
| `DependencyFailed` | 424 | `dependency_failed(upstream, detail)` |
| `InternalError` | 500 | `internal_error(error, message)` |
| `Unavailable` | 503 | Direct construction |

**Feature Flags:**

- `verbose-errors` — When enabled, `InternalError` returns the raw error string to the client instead of a sanitized message. Intended for dev/staging only.

```toml
# In your Cargo.toml (dev/staging only)
arche = { version = "2.2.0", features = ["verbose-errors"] }
```

### Utils (`arche::utils`)

```rust
use arche::utils::{validate_timestamp, FromOffsetDateTime, PaginationParams};
use sqlx::types::time::OffsetDateTime;

// Timestamp validation
let is_future = validate_timestamp(timestamp, false);

// DateTime conversion
let iso_string = offset_dt.to_iso_string()?;

// Pagination
let params = PaginationParams {
    page_number: Some(1),
    page_size: Some(20),
};
```

## What arche is *not*

- ❌ A framework that replaces Axum
- ❌ A code generator or project template
- ❌ A monolithic abstraction over third-party libraries
- ❌ A "do-everything" utils crate

`arche` favors composition over abstraction.

## Design principles

- **Explicit over implicit**
- **Composition over inheritance**
- **Thin wrappers over official SDKs**
- **Production-first defaults**
- **No global state**
- **Async-first**

## Why arche?

Most backend services end up re-implementing the same infrastructure concerns:

- Cloud SDK setup and ergonomics
- Database connection management
- Authentication primitives
- Middleware patterns
- Logging and tracing configuration
- Common error handling

**arche** brings these pieces together into a cohesive, Rust-native foundation,
built on top of well-established libraries and SDKs.

---

## What arche provides

### `aws`
AWS SDK integrations built on official SDKs:
- **S3**: Client initialization with support for IAM roles or environment-based credentials
- **SES**: Email sending with SES, including templated emails
- **KMS**: Key Management Service for encryption/decryption operations

### `gcp`
Google Cloud Platform integrations:
- **Drive**: Google Drive client with service account authentication
- **Sheets**: Google Sheets client with service account authentication

### `database`
Database connection management:
- **Postgres**: Connection pooling with `sqlx`, configurable credentials, health checks
- **Redis**: Connection pooling with `bb8`, async operations, health checks

### `jwt`
JWT utilities for authentication and authorization:
- Token generation and verification (HS256)
- Access/refresh token pair generation
- Token expiry helpers
- Custom claims support

### `csv`
Async CSV processing via a single reusable `CsvClient`:
- **Batch**: `read_all`, `read_file`, `write_all`, `write_file` — load everything at once
- **Streaming**: `reader` / `writer` factories for memory-efficient record-by-record I/O
- Configurable delimiter, quoting, escaping, headers, and more

### `error`
Axum-compatible error handling:
- `AppError` enum with common HTTP error variants
- Automatic `IntoResponse` conversion
- Structured error responses with details

### `utils`
Common utilities for backend services:
- Timestamp validation and conversion helpers
- `OffsetDateTime` utilities (Unix, ISO8601)
- Pagination parameter types

All components are modular and explicit—nothing is hidden or magical.

---

## Module Reference

### AWS (`arche::aws`)

#### S3
Initialize an S3 client with automatic credential management:
```rust
use arche::aws::s3::get_s3_client;

let client = get_s3_client().await;
```

**Environment Variables:**
- `S3_CRED_SOURCE`: `"IAM"` (default) or `"env"` for environment-based credentials
- `S3_ACCESS_KEY_ID`: Required when using `"env"` credential source
- `S3_SECRET_ACCESS_KEY`: Required when using `"env"` credential source

#### KMS
Encrypt and decrypt data using AWS Key Management Service:

```rust
use arche::aws::kms::{get_kms_client, KMSClient};

// Initialize with default region (ap-south-1)
let client = get_kms_client().await;
let kms = KMSClient::new(client);

// Or with a specific region
let kms = KMSClient::new_with_region("us-east-1").await;

// Encrypt data
let plaintext = b"sensitive data";
let ciphertext = kms.encrypt("alias/my-key", plaintext).await?;

// Decrypt data
let decrypted = kms.decrypt(&ciphertext).await?;
```

**Credentials:** Uses IAM role credentials by default (recommended for EC2/ECS/Lambda).

---

### GCP (`arche::gcp`)

#### Drive
```rust
use arche::gcp::drive::get_drive_client;

let drive = get_drive_client().await?;
```

**Environment Variables:**
- `GCP_DRIVE_KEY`: Path to service account JSON key file

#### Sheets
```rust
use arche::gcp::sheets::get_sheets_client;

let sheets = get_sheets_client().await?;
```

**Environment Variables:**
- `GCP_SHEETS_KEY`: Path to service account JSON key file

---

### Database (`arche::database`)

#### Postgres
```rust
use arche::database::pg::{get_pg_pool, test_pg};

let pool = get_pg_pool().await;
let is_healthy = test_pg(pool.clone()).await;
```

**Environment Variables:**
- `PG_HOST`: Database host
- `PG_PORT`: Database port
- `PG_DATABASE`: Database name
- `PG_MAX_CONN`: Maximum connections in pool
- `PG_CREDENTIALS`: JSON string with `username` and `password` (alternative)
- `PG_USERNAME`: Username (if not using `PG_CREDENTIALS`)
- `PG_PASSWORD`: Password (if not using `PG_CREDENTIALS`)

#### Redis
```rust
use arche::database::redis::{get_redis_pool, test_redis};

let pool = get_redis_pool().await;
let is_healthy = test_redis(pool.clone()).await;
```

**Environment Variables:**
- `REDIS_HOST`: Redis host
- `REDIS_PORT`: Redis port
- `REDIS_MAX_CONN`: Maximum connections in pool

---

### JWT (`arche::jwt`)

```rust
use arche::jwt::{generate_tokens, verify_token, generate_expiry_time};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

// Generate tokens
let access_claims = Claims { sub: "user_id".into(), exp: generate_expiry_time(3600) };
let refresh_claims = Claims { sub: "user_id".into(), exp: generate_expiry_time(86400) };

let tokens = generate_tokens(
    access_claims,
    refresh_claims,
    &access_secret,
    &refresh_secret,
);

// Verify token
let token_data = verify_token::<Claims>(&token, secret, Some("audience".into()))?;
```

---

### CSV (`arche::csv`)

Async CSV processing powered by `csv-async`. Create one `CsvClient`, reuse it everywhere:

```rust
use arche::csv::CsvClient;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Record {
    name: String,
    age: u32,
    city: String,
}

// Default config (comma-delimited, with headers)
let csv = CsvClient::new();

// Or customize
let csv = CsvClient::new()
    .delimiter(b';')
    .has_headers(true)
    .flexible(true);
```

#### Batch reading

```rust
// From bytes
let data = b”name,age,city\nAlice,30,NYC\nBob,25,LA”;
let records: Vec<Record> = csv.read_all(data.as_slice()).await?;

// From a file
let records: Vec<Record> = csv.read_file(“data.csv”).await?;

// Raw string records (no serde)
let raw_records = csv.read_records(data.as_slice()).await?;
```

#### Batch writing

```rust
#[derive(Serialize)]
struct Output {
    name: String,
    score: f64,
}

let records = vec![
    Output { name: “Alice”.into(), score: 95.5 },
    Output { name: “Bob”.into(), score: 87.0 },
];

// Write to in-memory bytes
let bytes: Vec<u8> = csv.write_all(&records).await?;

// Write to a file
csv.write_file(“output.csv”, &records).await?;
```

#### Streaming (memory-efficient)

```rust
// Record-by-record reading
let mut stream = csv.reader_from_file(“large.csv”).await?;
while let Some(result) = stream.next_deserialized::<Record>().await {
    let record = result?;
    // process one record at a time
}

// Record-by-record writing
let mut writer = csv.writer_to_file(“output.csv”).await?;
writer.serialize(&Output { name: “Alice”.into(), score: 95.5 }).await?;
writer.write_fields([“Bob”, “87.0”]).await?;
writer.finish().await?;
```

