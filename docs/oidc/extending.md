# Extending

The client side is turnkey — you configure it. The server side is generic
over four traits you implement.

| You want to | Plug into |
|---|---|
| Log users in via a provider (Google, any OIDC) | `OidcClient` + `Verifier` (config only) |
| Resolve which partner apps may log in against you | `impl ClientRegistry` |
| Sign ID tokens (local key, or KMS/HSM, or rotation) | `impl TokenSigner`, or the built-in `SigningKey` |
| Mint the access token (opaque, or your own JWT) | `impl AccessTokenIssuer` |
| Persist single-use codes (memory, Redis, Postgres) | `impl CodeStore` |

Nothing in `arche::oidc::server` changes when you implement any of these.

## Client quickstart

"Sign in with Google", end to end. `google()` is a `ProviderMetadata` preset;
any other provider comes from `ProviderMetadata::discover(key, issuer, http)`.

```rust
use arche::gcp::oauth::google;
use arche::oidc::{OidcClient, OidcConfig, Verifier};

// Startup — build once, hold on app state (both are Clone).
let client = OidcClient::new(
    google(),
    OidcConfig {
        client_id: cfg.google_client_id,
        client_secret: cfg.google_client_secret,
        redirect_uri: "https://app.example/auth/google/callback".into(),
        scopes: None, // defaults to "openid email profile"
    },
)?;
let verifier = Verifier::new(client.provider())?;

// GET /auth/google/login — mint state + PKCE, stash them, redirect.
let state = safe_nanoid();
let pkce_verifier = safe_nanoid();
let challenge = base64url_nopad(sha256(&pkce_verifier));
// stash `state` + `pkce_verifier` (HttpOnly cookie or server session), then:
redirect(client.auth_url(&state, &challenge));

// GET /auth/google/callback?code&state — verify state, exchange, verify token.
// 1. returned `state` MUST equal the stashed one.
let tokens = client.exchange_code(&code, &stashed_pkce_verifier).await?;

#[derive(serde::Deserialize)]
struct Claims { sub: String, email: String, email_verified: bool }

let claims: Claims = verifier
    .verify_id_token(&tokens.id_token, &[client.client_id()])
    .await?;
if !claims.email_verified {
    return Err(AppError::Unauthorized);
}
// upsert user on claims.sub, mint YOUR session, redirect home.
```

`verify_id_token::<C>` validates RS256, the signing `kid` (against a
rotation-aware JWKS cache), the signature, and `iss`/`aud`/`exp`/`iat`/`nbf`
with 60 s skew — then deserializes into your `C`. A bad token is
`AppError::Unauthorized`; an unreachable provider is
`AppError::DependencyFailed`. Nonce checking is yours (you know the nonce you
sent — put it in `C` and compare).

## Server quickstart

Assemble `OidcServer` from your four implementations (below) and wire four
routes. Full runnable wiring lives in `tests/oidc_interop.rs`.

```rust
use arche::oidc::server::{OidcServer, OidcServerConfig, SigningKey};

let server = OidcServer::new(
    OidcServerConfig {
        issuer: "https://id.example.com".into(), // https; iss + endpoint prefix
        code_ttl: None,        // default 5 min
        id_token_ttl: None,    // default 1 h
        allowed_scopes: None,  // default ["openid","email","profile"], must include openid
    },
    MyRegistry(/* ... */),                                  // ClientRegistry
    SigningKey::from_pem("2026-07-key", &pem)?,             // TokenSigner
    MyAccessTokens,                                         // AccessTokenIssuer
    MyStore(/* ... */),                                     // CodeStore
)?;
// server: Clone (cheap, Arc), lives on app state.
```

The four handlers (axum shapes, abbreviated):

```rust
// GET /.well-known/openid-configuration
Json(DiscoveryDocument::standard(server.issuer())) // override fields to match your routes

// GET /jwks   — send Cache-Control: public, max-age=3600
Json(server.jwks_document())

// GET /authorize
let validated = match server.validate_authorize(&params).await {
    Ok(v) => v,
    Err(e) if e.redirectable() => return redirect(e.redirect_url(&params.redirect_uri, params.state.as_deref())?),
    Err(e) => return bad_request(e.to_string()), // unknown client / bad redirect — never redirect
};
match session_user(&headers) {
    Some(user) => redirect(server.issue_code(validated, user.id, user.claims()).await?),
    None => redirect("/login"), // stash `validated`, resume after login
}

// POST /token   — send Cache-Control: no-store on success
let req = TokenRequest { /* form fields */, basic_auth: auth_header.and_then(TokenRequest::parse_basic_authorization) };
match server.exchange(req).await {
    Ok(payload) => json_no_store(payload),
    Err(OidcServerError::Internal(_)) => server_error(), // 5xx — retryable
    Err(e) => token_error(e.error_code()),               // 400/401
}
```

## The four seams

### `ClientRegistry` — who may log in against you

`find` resolves a client; the defaulted `verify_secret` compares plaintext in
constant time. No built-in — the simplest impl is a static list; production
points at a DB.

```rust
use arche::oidc::server::{ClientRegistration, ClientRegistry};
use arche::error::AppError;

struct StaticClients(Vec<ClientRegistration>);

impl ClientRegistry for StaticClients {
    async fn find(&self, client_id: &str) -> Result<Option<ClientRegistration>, AppError> {
        Ok(self.0.iter().find(|c| c.client_id == client_id).cloned())
    }
}
```

Storing **hashed** secrets? Override `verify_secret` — never return plaintext
from `find`:

```rust
impl ClientRegistry for DbClients {
    async fn find(&self, client_id: &str) -> Result<Option<ClientRegistration>, AppError> {
        // SELECT ... ; return the registration WITHOUT the real secret if you like
    }
    async fn verify_secret(&self, client_id: &str, presented: &str) -> Result<bool, AppError> {
        let hash = self.load_secret_hash(client_id).await?;
        Ok(argon2_verify(&hash, presented)) // constant-time inside argon2
    }
}
```

A lookup *failure* (DB down) must surface as `Err(AppError)` — arche maps it to
`server_error`, never to "unknown client".

### `TokenSigner` — sign ID tokens

Custody-only: arche builds the JWT and asks you to sign bytes. Local keys? Use
the built-in `SigningKey::from_pem(kid, pem)` (PKCS#8/#1, ≥2048 bits) — no code
to write. Key in a KMS/HSM? Implement three methods:

```rust
use arche::oidc::server::{TokenSigner, rsa_public_jwk};
use arche::error::AppError;

struct KmsSigner { kid: String, public_pem: String /* + KMS handle */ }

impl TokenSigner for KmsSigner {
    fn kid(&self) -> &str { &self.kid }

    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, AppError> {
        let digest = sha256(message);
        self.kms.asymmetric_sign(&self.kid, &digest).await // RSASSA-PKCS1-v1_5
            .map_err(|e| AppError::internal_error(format!("kms sign: {e}"), None))
    }

    fn jwks(&self) -> serde_json::Value {
        serde_json::json!({ "keys": [rsa_public_jwk(&self.kid, &self.public_pem).unwrap()] })
    }
}
```

Key rotation is just a signer whose `jwks()` returns several keys while `sign`
uses the newest — no arche change. Whatever `alg` you sign with must match your
`DiscoveryDocument` and JWKS (all three are yours; arche can't cross-check).

### `AccessTokenIssuer` — mint the access token

`issue(grant) -> IssuedAccessToken { token, expires_in }`. No built-in — the
token's nature is your policy. Opaque noise is fine when only the ID token is
consumed (the Shopify flow):

```rust
use arche::oidc::server::{AccessTokenIssuer, IssuedAccessToken, PendingGrant};
use arche::error::AppError;

struct OpaqueTokens;

impl AccessTokenIssuer for OpaqueTokens {
    async fn issue(&self, _grant: &PendingGrant) -> Result<IssuedAccessToken, AppError> {
        Ok(IssuedAccessToken { token: arche::utils::nano_id_of(43), expires_in: 3600 })
    }
}
```

The opaque token is stored nowhere and verifiable by nobody. The moment
something must *accept* it as a credential, mint a real one — sign a JWT (reuse
your `TokenSigner`) or persist an opaque token and check it on introspection.
`grant` gives you the subject, client, and scope to embed.

### `CodeStore` — single-use code memory

`put` remembers what a code stands for; `take` returns it **and deletes it in
one atomic step** — that delete-on-read is the entire replay defense. No
built-in. An in-memory map works for a single instance:

```rust
use arche::oidc::server::{CodeStore, PendingGrant};
use arche::error::AppError;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Default)]
struct MemStore(Mutex<HashMap<String, (PendingGrant, Instant)>>);

impl CodeStore for MemStore {
    async fn put(&self, code: String, grant: PendingGrant, ttl: Duration) -> Result<(), AppError> {
        let expires = Instant::now().checked_add(ttl).unwrap_or_else(Instant::now);
        self.0.lock().await.insert(code, (grant, expires));
        Ok(())
    }
    async fn take(&self, code: &str) -> Result<Option<PendingGrant>, AppError> {
        Ok(self.0.lock().await.remove(code)
            .filter(|(_, exp)| *exp > Instant::now())
            .map(|(g, _)| g))
    }
}
```

Behind a load balancer this breaks — `/authorize` and `/token` can hit
different pods. Use a shared store that keeps `take` atomic: **Redis** `GETDEL`,
or **Postgres** `DELETE ... RETURNING`. Same trait, `take` runs the atomic
op instead of a map remove.

## Not supported (deliberately, for now)

Refresh tokens, the client-credentials grant, and `/userinfo` (claims ride in
the ID token). Key rotation is a `TokenSigner` choice, not a limitation.
