mod common;

use arche::oidc::server::{
    AuthorizeParams, ClientRegistration, DiscoveryDocument, OidcServer, OidcServerConfig,
    SigningKey, TokenPayload, TokenRequest,
};
use arche::oidc::{OidcClient, OidcConfig, ProviderMetadata, Verifier};
use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{TestRegistry, TestStore, TestTokens, pem};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const VERIFIER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REDIRECT_URI: &str = "https://app.example/cb";

fn challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))
}

type Srv = Arc<OidcServer<TestRegistry, SigningKey, TestTokens, TestStore>>;

fn server_for(issuer: &str) -> Srv {
    let config = OidcServerConfig {
        issuer: issuer.into(),
        code_ttl: None,
        id_token_ttl: None,
        allowed_scopes: None,
    };
    let clients = TestRegistry(vec![ClientRegistration {
        client_id: "e2e-client".into(),
        client_secret: "e2e-secret".into(),
        redirect_uris: vec![REDIRECT_URI.into()],
    }]);
    Arc::new(
        OidcServer::new(
            config,
            clients,
            SigningKey::from_pem("k1", &pem()).unwrap(),
            TestTokens,
            TestStore::default(),
        )
        .unwrap(),
    )
}

async fn discovery_handler(State(server): State<Srv>) -> Response {
    Json(DiscoveryDocument::standard(server.issuer())).into_response()
}

async fn jwks_handler(State(server): State<Srv>) -> Response {
    Json(server.jwks_document()).into_response()
}

async fn authorize_handler(
    State(server): State<Srv>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    let validated = match server.validate_authorize(&params).await {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let claims = serde_json::json!({
        "email": "u1@example.com",
        "phoneNumber": "+1-555-0100",
        "countryCode": "US",
    });
    match server.issue_code(validated, "u1", claims).await {
        Ok(url) => (StatusCode::FOUND, [(header::LOCATION, url)]).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    code: String,
    code_verifier: String,
    redirect_uri: String,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

async fn token_handler(
    State(server): State<Srv>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let basic_auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(TokenRequest::parse_basic_authorization);
    let request = TokenRequest {
        grant_type: form.grant_type,
        code: form.code,
        code_verifier: form.code_verifier,
        redirect_uri: form.redirect_uri,
        client_id: form.client_id,
        client_secret: form.client_secret,
        basic_auth,
    };
    match server.exchange(request).await {
        Ok(payload) => (
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::PRAGMA, "no-cache"),
            ],
            Json::<TokenPayload>(payload),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.error_code()).into_response(),
    }
}

fn app(server: Srv) -> Router {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery_handler))
        .route("/jwks", get(jwks_handler))
        .route("/authorize", get(authorize_handler))
        .route("/token", post(token_handler))
        .with_state(server)
}

async fn serve() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let router = app(server_for(&issuer));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    issuer
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn query_param(url: &str, key: &str) -> Option<String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

#[tokio::test]
async fn end_to_end_interop_via_consumer_built_endpoints() {
    let issuer = serve().await;
    let http = no_redirect_client();

    let provider = ProviderMetadata::discover("selfhosted", &issuer, &http)
        .await
        .expect("discover");
    let client = OidcClient::new(
        provider.clone(),
        OidcConfig {
            client_id: "e2e-client".into(),
            client_secret: "e2e-secret".into(),
            redirect_uri: REDIRECT_URI.into(),
            scopes: None,
        },
    )
    .expect("client");

    let auth_url = client.auth_url("state-1", &challenge());
    let resp = http.get(&auth_url).send().await.unwrap();
    assert_eq!(resp.status(), 302);
    let location = resp.headers()[header::LOCATION].to_str().unwrap();
    assert!(location.starts_with(REDIRECT_URI));
    assert_eq!(query_param(location, "state").as_deref(), Some("state-1"));
    let code = query_param(location, "code").expect("code param");

    let tokens = client
        .exchange_code(&code, VERIFIER)
        .await
        .expect("exchange");

    let claims: serde_json::Value = Verifier::with_http_client(&provider, reqwest::Client::new())
        .verify_id_token(&tokens.id_token, &["e2e-client"])
        .await
        .expect("verify");
    assert_eq!(claims["sub"], "u1");
    assert_eq!(claims["email"], "u1@example.com");
    assert_eq!(claims["iss"], issuer);
    assert_eq!(claims["phoneNumber"], "+1-555-0100");
    assert_eq!(claims["countryCode"], "US");
}
