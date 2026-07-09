# Login Sequence

The authorization-code flow with PKCE, both directions. See
[architecture.md](architecture.md) for the static who-talks-to-whom.

## Serving logins (`arche::oidc::server`)

Partner app "shopfront" logs its users in via your identity provider. arche
does not run HTTP — you wire four routes, each calling one method.

```mermaid
sequenceDiagram
    autonumber
    participant RP as Partner backend
    participant B as User's browser
    participant H as Your handlers
    participant S as OidcServer
    participant CR as ClientRegistry<br/>(yours)
    participant CS as CodeStore<br/>(yours)
    participant TS as TokenSigner
    participant AT as AccessTokenIssuer<br/>(yours)

    Note over RP,H: Once — discovery (arche not involved)
    RP->>H: GET /.well-known/openid-configuration
    H->>H: DiscoveryDocument::standard(issuer) + your overrides
    H-->>RP: 200 JSON

    Note over B,CS: /authorize — a login begins
    RP->>B: redirect to your /authorize
    B->>H: GET /authorize?response_type=code&client_id&redirect_uri&state&code_challenge...
    H->>S: validate_authorize(&params).await
    S->>CR: find(client_id)
    CR-->>S: Some(ClientRegistration) | None
    alt unknown client or unregistered redirect_uri
        S-->>H: Err(...) redirectable() = false
        H-->>B: 400 on YOUR page — never redirect these
    else redirectable error (bad response_type, scope, mode, JAR)
        S-->>H: Err(e) redirectable() = true
        H->>S: e.redirect_url(&redirect_uri, state)
        H-->>B: 302 back to partner with ?error=...
    else valid
        S-->>H: Ok(ValidatedAuthorizeRequest)
    end

    Note over H: YOUR code — is this user logged in?
    opt no session
        H-->>B: stash validated request, show /login, resume after
    end
    H->>S: issue_code(validated, subject, claims).await
    S->>CR: find(client_id) — re-check client + redirect_uri
    S->>CS: put(code, PendingGrant{request, subject, claims}, code_ttl)
    S-->>H: "{redirect_uri}?code=...&state=..."
    H-->>B: 302 Location
    B->>RP: partner callback with code + state

    Note over RP,AT: /token — redemption (server-to-server, no browser)
    RP->>H: POST /token (grant_type, code, code_verifier, redirect_uri + client auth)
    H->>H: TokenRequest::parse_basic_authorization(header)
    H->>S: exchange(TokenRequest).await
    S->>CR: verify_secret(client_id, presented) — constant time
    S->>CS: take(code) — atomic delete-on-read (single-use enforced here)
    CS-->>S: Some(PendingGrant)
    S->>S: check client_id, redirect_uri, verify_pkce(verifier, challenge)
    S->>TS: sign(protocol claims stamped over consumer claims)
    TS-->>S: id_token JWT (RS256, header kid)
    S->>AT: issue(grant)
    AT-->>S: IssuedAccessToken { token, expires_in }
    S-->>H: Ok(TokenPayload)
    H-->>RP: 200 JSON, Cache-Control: no-store

    Note over RP,H: Verification — partner checks the signature
    RP->>H: GET /jwks
    H->>S: jwks_document()
    S->>TS: jwks()
    H-->>RP: 200 JSON — partner's Verifier now trusts sub
```

## Logging in via someone else (`arche::oidc` client)

Your app is the relying party against Google (or any OIDC provider).

```mermaid
sequenceDiagram
    autonumber
    participant U as User's browser
    participant A as Your app
    participant O as OidcClient / Verifier
    participant P as Provider (Google)

    Note over A,O: Once, at startup
    A->>O: OidcClient::new(google(), config) and Verifier::new(client.provider())

    Note over U,P: Login
    U->>A: GET /auth/google/login
    A->>A: mint state + PKCE verifier, stash both
    A->>O: client.auth_url(state, challenge)
    A-->>U: 302 to provider consent screen
    U->>P: authenticate + consent
    P-->>U: 302 to redirect_uri?code=...&state=...
    U->>A: GET /auth/google/callback?code&state

    A->>A: check returned state == stashed state
    A->>O: client.exchange_code(code, stashed_verifier).await
    O->>P: POST /token (client_secret + verifier)
    P-->>O: TokenResponse { id_token, ... }
    A->>O: verifier.verify_id_token::<Claims>(id_token, &[client_id]).await
    O->>P: GET /jwks (cached, rotation-aware)
    O-->>A: your Claims (signature + iss/aud/exp/iat/nbf verified)
    A->>A: check email_verified, upsert user on sub, mint YOUR session
    A-->>U: logged in
```

## Why state and PKCE

Two random per-login values arche threads through the flow (`auth_url(state,
challenge)` out, `exchange_code(code, verifier)` back). They defend opposite
substitution attacks — and both rest on the same fact: a browser's stash
(cookie/session) belongs to that browser alone; an attacker can forge the URL
but not the victim's stash.

- **`state`** — a CSRF token for the callback. Sent in two places at login: a
  cookie in the user's browser and the provider URL. The callback is legit
  only if the URL's `state` equals the stash's. Stops an attacker from feeding
  *their* login result into the victim's browser (their `state` won't match the
  victim's cookie).
- **PKCE** — `challenge = base64url(sha256(verifier))` rides the auth URL; the
  provider welds it onto the code. At exchange the backend reveals the
  `verifier`, which never entered any URL. Stops a *leaked code* from being
  redeemed by anyone else — the exchange demands the hash preimage, and only
  the flow that started the login has it. arche hardcodes S256 (no `plain`).

Rule of thumb: **state** stops the attacker's code landing in the victim's
session; **PKCE** stops the victim's code being redeemed in the attacker's
session.

## Refresh tokens (opt-in via `offline_access`)

Long-lived sessions without re-prompting. Off unless the granted scope
includes `offline_access` (operator adds it to `allowed_scopes`; your
`/authorize` handler decides whether to grant it).

```mermaid
sequenceDiagram
    autonumber
    participant RP as Partner backend
    participant S as OidcServer
    participant CS as CodeStore<br/>(yours)
    participant RS as RefreshTokenStore<br/>(yours)

    Note over RP,RS: First /token — code exchange with offline_access granted
    RP->>S: POST /token (grant_type=authorization_code, code, verifier)
    S->>CS: take(code) → grant (scope includes offline_access)
    S->>RS: put(refresh_token, grant, refresh_ttl)
    S-->>RP: { id_token, access_token, refresh_token }

    Note over RP,RS: Later /token — rotate
    RP->>S: POST /token (grant_type=refresh_token, refresh_token=RT₁ + client auth)
    S->>RS: take(RT₁) → grant   (RT₁ now GONE)
    S->>S: check grant.client_id == authenticated client; drop one-time nonce
    S->>RS: put(RT₂, grant, refresh_ttl)   (rotation)
    S-->>RP: { id_token (fresh), access_token (fresh), refresh_token=RT₂ }

    Note over RP,S: Replay of RT₁ → take() finds nothing → invalid_grant
```

The `take`-then-`put` is rotation: each refresh invalidates the previous
token and mints a new one. Because `take` is delete-on-read, a replayed
(already-rotated) token is rejected — the same single-use mechanic as
authorization codes. Claims are snapshotted at authorization; arche re-mints
them verbatim (minus `nonce`), as it has no user model to re-fetch.

## Error paths (server)

All redirectable errors flow back to the partner as `?error=<code>`; the two
non-redirectable ones must render on your page.

| `OidcServerError` | `error_code()` | Redirectable? | Your handler |
|---|---|---|---|
| `UnknownClient`, `UnregisteredRedirectUri` | `invalid_client` / `invalid_request` | **no** | 400 on your page |
| `UnsupportedResponseType`, `MissingPkce`, `Unsupported/MalformedChallenge`, `Malformed/MissingOpenidScope`, `UnsupportedResponseMode`, `Request(Uri)NotSupported` | per RFC | yes | 302 `e.redirect_url(uri, state)` |
| `AccessDenied`, `LoginRequired`, `InteractionRequired`, `ConsentRequired` | per RFC §3.1.2.6 | yes | 302 — construct these yourself when your login flow denies/needs auth |
| `InvalidClient`, `InvalidGrant`, `UnsupportedGrantType` (token endpoint) | `invalid_client` / `invalid_grant` / `unsupported_grant_type` | n/a | 400/401 JSON |
| `Internal(AppError)` | `server_error` | n/a | **5xx** — infra failure; the partner retries |

The `Internal(AppError)` split matters: a registry/store/signer outage returns
a 5xx a well-behaved RP will retry, not a 400 it would treat as permanent.

## What the client sees over the wire (token endpoint)

| | Value |
|---|---|
| Success | `200` + `TokenPayload` JSON, `Cache-Control: no-store` (OIDC §3.1.3.3) |
| `access_token` | whatever your `AccessTokenIssuer` minted |
| `id_token` | RS256 JWT: your claims + `iss`/`sub`/`aud`/`iat`/`exp` (+`nonce` if requested); `Debug` redacts it |
| Error | `400`/`401` + `{ "error": "<code>" }` |
