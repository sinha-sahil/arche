# Architecture

Two independent halves of one protocol:

1. **`arche::oidc` (client / relying party)** — build the authorize redirect,
   exchange the code for tokens, verify the ID token. Pure protocol logic over
   `reqwest`; you supply config and a `ProviderMetadata`.
2. **`arche::oidc::server` (identity provider)** — validate the authorize
   request, mint the single-use code, sign the ID token. arche owns the
   protocol; **you** own client resolution, code storage, token signing, and
   access-token minting via four traits.

Neither half imports the other in production code. The only cross-reference is
one `use` line inside a `#[cfg(test)]` interop test that drives the client
against the server over real HTTP — compatibility proven without shared code.

## Map

Hover any node for a one-line explanation (GitHub, VS Code, and mermaid.live
all render the tooltips).

```mermaid
flowchart LR
    Consumer["Consumer code<br/><i>impl the 4 server traits, HTTP handlers, client wiring</i>"]

    subgraph CLIENT["arche::oidc — relying party"]
        direction TB
        OC["OidcClient"]
        PM["ProviderMetadata"]
        TR["TokenResponse"]
        VF["Verifier"]
        JC["JwksCache<br/><i>private</i>"]
        CFG["OidcConfig"]
        GG["google()"]
    end

    subgraph SERVER["arche::oidc::server — identity provider"]
        direction TB
        OS["OidcServer&lt;C,T,A,S&gt;"]
        CR(("ClientRegistry<br/>trait"))
        TS(("TokenSigner<br/>trait"))
        AT(("AccessTokenIssuer<br/>trait"))
        CS(("CodeStore<br/>trait"))
        SK["SigningKey"]
        HELP["rsa_public_jwk()"]
        DD["DiscoveryDocument"]
        OE["OidcServerError"]
        PG["PendingGrant"]
    end

    %% client composition
    CFG --> OC
    PM --> OC
    GG -->|builds| PM
    OC -->|returns| TR
    PM --> VF
    VF --> JC
    TR --> VF

    %% server generics + trait impls
    OS -->|C| CR
    OS -->|T| TS
    OS -->|A| AT
    OS -->|S| CS
    SK -. implements .-> TS
    Consumer -. implements .-> CR
    Consumer -. implements .-> AT
    Consumer -. implements .-> CS
    OS -->|mints and stores| PG
    OS -->|serves| DD
    OS -->|rejects with| OE

    %% consumer wiring
    Consumer --> OC
    Consumer --> OS
    Consumer --> HELP

    click OC href "#oidcclient" "Relying-party client. new(provider, config) validates endpoints. auth_url(state, challenge) builds the consent redirect; exchange_code(code, verifier) trades the code for tokens."
    click PM href "#providermetadata" "A provider's address book: key, issuers, auth/token/jwks endpoints, extra_auth_params. Struct literal, google() preset, or discover(key, issuer, http)."
    click TR href "#tokenresponse" "What exchange_code returns: access_token, id_token (the JWT you verify), expires_in, token_type, optional scope/refresh_token. Debug redacts tokens."
    click VF href "#verifier" "Is this ID token genuinely from the provider, for us, now? verify_id_token::<C>(id_token, audiences) checks RS256, kid, signature, iss/aud/exp/iat/nbf, then deserializes into your type C."
    click JC href "#jwkscache" "Private. In-memory JWKS cache: TTL from Cache-Control, refetch on unknown kid (rotation), stale-grace serving when upstream is down, single fetch under concurrency."
    click CFG href "#oidcconfig" "Your registration at the provider: client_id, client_secret, redirect_uri (required), scopes (Option, defaults openid email profile). Debug redacts the secret."
    click GG href "#google" "arche::gcp::oauth::google() — a ProviderMetadata preset with Google's endpoints and quirks (prompt=select_account, access_type=online)."

    click OS href "#oidcserver" "The protocol brain, generic over your four traits. new(config, clients, signer, access_tokens, store). Methods: validate_authorize, issue_code, exchange, jwks_document. Cheap to clone (Arc)."
    click CR href "#clientregistry" "YOU implement. find(client_id) -> Option<ClientRegistration>, plus a defaulted verify_secret (constant-time plaintext) you override for hashed secrets. No built-in."
    click TS href "#tokensigner" "YOU implement, or use SigningKey. kid() + async sign(bytes) + jwks(). arche builds the JWT; the signer just signs bytes — so a KMS/HSM impl is one API call."
    click AT href "#accesstokenissuer" "YOU implement. issue(grant) -> IssuedAccessToken { token, expires_in }. Opaque random, your own JWT, or a stored token. No built-in."
    click CS href "#codestore" "YOU implement. put(code, grant, ttl) + take(code) with atomic delete-on-read (the single-use guarantee). In-memory, Redis GETDEL, PG DELETE..RETURNING. No built-in."
    click SK href "#signingkey" "The built-in TokenSigner: an RSA private key loaded from PEM (from_pem(kid, pem), >=2048 bits), held in process. Signs RS256 locally, serves its public half in jwks(). Debug redacts."
    click HELP href "#rsa_public_jwk" "Helper: rsa_public_jwk(kid, public_pem) -> JWKS entry. Lets any custom TokenSigner build its jwks() in one call."
    click DD href "#discoverydocument" "Your public metadata — YOU build and serve it. standard(issuer) gives the conventional shape; all fields pub so you override endpoints/scopes/claims to match your routes."
    click OE href "#oidcservererror" "Protocol failure vocabulary (RFC 6749 codes). error_code(), redirectable(), redirect_url(uri, state) — which refuses to build a redirect for non-redirectable errors. Internal(AppError) for infra failures."
    click PG href "#pendinggrant" "What a code stands for between /authorize and /token: the validated request, the subject (sub), and your chosen claims. Stored in the CodeStore, keyed by the code."

    click Consumer href "#consumer" "Your service: implementations of ClientRegistry / TokenSigner (or SigningKey) / AccessTokenIssuer / CodeStore, the four HTTP handlers, and the client-side login wiring."

    classDef trait fill:#f4e9ff,stroke:#8858c4,color:#333;
    classDef builtin fill:#e9f4ff,stroke:#3c78b8,color:#333;
    classDef consumer fill:#fff4e5,stroke:#c48e3c,color:#333;
    class CR,TS,AT,CS trait
    class OC,PM,TR,VF,JC,CFG,GG,OS,SK,HELP,DD,OE,PG builtin
    class Consumer consumer
```

### Legend

- **Oval nodes (`(( ))`)** — traits. The seams you plug into.
- **Purple-tinted** — trait definitions.
- **Blue-tinted** — concrete arche code (types, helpers, the built-in `SigningKey`).
- **Orange-tinted** — consumer code (out of this crate).

## The four server seams

`OidcServer` is generic over four traits. The asymmetry is deliberate — a
built-in exists only where it encodes *math*, never *policy*:

| Trait | Built-in | Why |
|---|---|---|
| `TokenSigner` | `SigningKey` (local RSA) | Correct RS256 signing + JWK derivation is math; a wrong hand-roll is a security bug. A local PEM is a legitimate production posture. |
| `ClientRegistry` | **none** | Where partner registrations live (static, DB, config service) is your policy. |
| `AccessTokenIssuer` | **none** | Whether the access token is opaque noise or a verifiable JWT is your policy. |
| `CodeStore` | **none** | Where codes persist (memory vs. Redis/PG) is your deployment policy — and single-instance vs. clustered is a correctness choice only you can make. |

`SigningKey` is also **custody-only**: arche assembles the JWT (header,
base64url, splice) inside `mint_id_token` and asks the signer only to *sign
these bytes*. That is what makes a Cloud KMS / HSM signer a ~15-line impl
rather than a JOSE project.

## Where each defense lives

| Defense | Enforced by | Where |
|---|---|---|
| Codes go only to registered URIs | `validate_authorize` + re-check in `issue_code`, against your registry | `server/mod.rs` |
| Unknown client / unregistered URI never redirect | validated first; `redirect_url` refuses non-redirectable errors | `server/mod.rs` + `types.rs` |
| PKCE mandatory, S256 only, challenge format-checked | `validate_authorize` / `issue_code` | `server/mod.rs` |
| `openid` scope required, tokens format-checked, narrowed to `allowed_scopes` | `validate_authorize` | `server/mod.rs` |
| Code single-use | atomic delete-on-read in `take` | your `CodeStore` |
| Code bound to client + redirect_uri; PKCE verifier matches | `exchange` vs. `PendingGrant`, `verify_pkce` | `server/mod.rs` |
| Client secret compared in constant time | default `verify_secret` / your override | `registry.rs` |
| Consumer claims can't forge protocol claims | `RESERVED_CLAIMS` strip in `mint_id_token` | `server/mod.rs` |
| `sub` bounded to 1–255 ASCII | `issue_code` | `server/mod.rs` |
| Issuer is https (loopback-http for dev only) | `OidcServer::new` | `server/mod.rs` |
| Signing-key custody | your `TokenSigner` — local PEM or KMS/HSM | `keys.rs` |

On the client side, the `Verifier` does the mirror job: RS256-only, `kid`
lookup against a rotation-aware JWKS cache, then `iss`/`aud`/`exp`/`iat`/`nbf`
with 60 s clock skew. Anything wrong with the token is a uniform
`AppError::Unauthorized` (real reason logged at debug); the provider being
unreachable is `AppError::DependencyFailed`.

## Not supported (deliberately, for now)

Refresh tokens, the client-credentials grant, and `/userinfo` (claims ride in
the ID token). Key rotation is *not* an arche limitation — it is a
`TokenSigner` implementation choice (serve several keys from `jwks()`, sign
with the newest).

## Next

- [sequence.md](sequence.md) — what a login looks like at runtime, both directions.
- [extending.md](extending.md) — run a client, stand up the server, implement the four traits.
