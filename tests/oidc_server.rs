mod common;

use arche::error::AppError;
use arche::oidc::server::{
    AuthorizeParams, ClientRegistration, CodeStore, DiscoveryDocument, OidcServer,
    OidcServerConfig, OidcServerError, PendingGrant, SigningKey, TokenPayload, TokenRequest,
    ValidatedAuthorizeRequest, rsa_public_jwk,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{TestRefreshStore, TestRegistry, TestStore, TestTokens, pem};
use reqwest::Url;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;

const ISSUER: &str = "https://id.example.com";
const VERIFIER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

type TestServer = OidcServer<TestRegistry, SigningKey, TestTokens, TestStore, TestRefreshStore>;

fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn config() -> OidcServerConfig {
    OidcServerConfig {
        issuer: ISSUER.into(),
        code_ttl: None,
        id_token_ttl: None,
        refresh_token_ttl: None,
        allowed_scopes: None,
    }
}

fn clients() -> TestRegistry {
    TestRegistry(vec![ClientRegistration {
        client_id: "cid".into(),
        client_secret: "secret".into(),
        redirect_uris: vec!["https://app.example/cb".into()],
    }])
}

fn signer() -> SigningKey {
    SigningKey::from_pem("k1", &pem()).unwrap()
}

fn server_with(config: OidcServerConfig) -> Result<TestServer, AppError> {
    OidcServer::new(
        config,
        clients(),
        signer(),
        TestTokens,
        TestStore::default(),
        TestRefreshStore::default(),
    )
}

fn offline_config() -> OidcServerConfig {
    OidcServerConfig {
        allowed_scopes: Some(vec![
            "openid".into(),
            "email".into(),
            "profile".into(),
            "offline_access".into(),
        ]),
        ..config()
    }
}

fn server() -> TestServer {
    server_with(config()).unwrap()
}

fn authorize_params() -> AuthorizeParams {
    AuthorizeParams {
        response_type: "code".into(),
        client_id: "cid".into(),
        redirect_uri: "https://app.example/cb".into(),
        scope: Some("openid email profile".into()),
        state: Some("st".into()),
        code_challenge: Some(challenge_of(VERIFIER)),
        code_challenge_method: Some("S256".into()),
        nonce: Some("n-123".into()),
        prompt: None,
        login_hint: None,
        max_age: None,
        id_token_hint: None,
        acr_values: None,
        display: None,
        ui_locales: None,
        claims: None,
        response_mode: None,
        request: None,
        request_uri: None,
    }
}

fn token_request(code: &str) -> TokenRequest {
    TokenRequest {
        grant_type: "authorization_code".into(),
        code: code.into(),
        code_verifier: VERIFIER.into(),
        redirect_uri: "https://app.example/cb".into(),
        refresh_token: None,
        client_id: Some("cid".into()),
        client_secret: Some("secret".into()),
        basic_auth: None,
    }
}

fn refresh_request(refresh_token: &str) -> TokenRequest {
    TokenRequest {
        grant_type: "refresh_token".into(),
        code: String::new(),
        code_verifier: String::new(),
        redirect_uri: String::new(),
        refresh_token: Some(refresh_token.into()),
        client_id: Some("cid".into()),
        client_secret: Some("secret".into()),
        basic_auth: None,
    }
}

fn code_from(url: &str) -> String {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .expect("code param")
}

async fn issue(server: &TestServer) -> String {
    let validated = server
        .validate_authorize(&authorize_params())
        .await
        .unwrap();
    let url = server
        .issue_code(
            validated,
            "u1",
            json!({ "email": "u@e.co", "email_verified": true }),
        )
        .await
        .unwrap();
    code_from(&url)
}

fn jwt_payload(token: &str) -> serde_json::Value {
    let payload_b64 = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_b64).unwrap()).unwrap()
}

mod construction {
    use super::*;

    #[test]
    fn new_trims_issuer() {
        let mut c = config();
        c.issuer = format!("{ISSUER}/");
        assert_eq!(server_with(c).unwrap().issuer(), ISSUER);
    }

    #[test]
    fn new_rejects_invalid_issuer() {
        for bad in [
            "not a url",
            "foo:bar",
            "https://id.example.com?x=1",
            "https://id.example.com#frag",
            "http://id.example.com",
        ] {
            let mut c = config();
            c.issuer = bad.into();
            assert!(server_with(c).is_err(), "issuer {bad:?} should be rejected");
        }
    }

    #[test]
    fn new_allows_loopback_http_issuer() {
        let mut c = config();
        c.issuer = "http://127.0.0.1:8080".into();
        assert!(server_with(c).is_ok());
    }
}

mod validate {
    use super::*;

    #[tokio::test]
    async fn rejects_unknown_client_before_redirect_checks() {
        let mut p = authorize_params();
        p.client_id = "nope".into();
        let err = server().validate_authorize(&p).await.unwrap_err();
        assert!(matches!(err, OidcServerError::UnknownClient(_)));
        assert!(!err.redirectable());
    }

    #[tokio::test]
    async fn rejects_unregistered_redirect_uri() {
        let mut p = authorize_params();
        p.redirect_uri = "https://evil.example/cb".into();
        let err = server().validate_authorize(&p).await.unwrap_err();
        assert!(matches!(err, OidcServerError::UnregisteredRedirectUri(_)));
        assert!(!err.redirectable());
    }

    #[tokio::test]
    async fn rejects_non_code_response_type() {
        let mut p = authorize_params();
        p.response_type = "token".into();
        let err = server().validate_authorize(&p).await.unwrap_err();
        assert!(matches!(err, OidcServerError::UnsupportedResponseType(_)));
        assert!(err.redirectable());
    }

    #[tokio::test]
    async fn requires_pkce_s256() {
        let mut p = authorize_params();
        p.code_challenge = None;
        assert!(matches!(
            server().validate_authorize(&p).await.unwrap_err(),
            OidcServerError::MissingPkce
        ));

        let mut p = authorize_params();
        p.code_challenge_method = Some("plain".into());
        assert!(matches!(
            server().validate_authorize(&p).await.unwrap_err(),
            OidcServerError::UnsupportedChallengeMethod(_)
        ));
    }

    #[tokio::test]
    async fn rejects_malformed_code_challenge() {
        for bad in ["short", "has spaces has spaces has spaces has spaces ok"] {
            let mut p = authorize_params();
            p.code_challenge = Some(bad.into());
            assert!(matches!(
                server().validate_authorize(&p).await.unwrap_err(),
                OidcServerError::MalformedCodeChallenge
            ));
        }
    }

    #[tokio::test]
    async fn requires_scope_param() {
        let mut p = authorize_params();
        p.scope = None;
        assert!(matches!(
            server().validate_authorize(&p).await.unwrap_err(),
            OidcServerError::MissingOpenidScope
        ));
    }

    #[tokio::test]
    async fn rejects_scope_without_openid() {
        let mut p = authorize_params();
        p.scope = Some("email profile".into());
        let err = server().validate_authorize(&p).await.unwrap_err();
        assert!(matches!(err, OidcServerError::MissingOpenidScope));
        assert!(err.redirectable());
    }

    #[tokio::test]
    async fn rejects_malformed_scope_tokens() {
        for bad in ["openid a\"b", "openid  double-space", "openid back\\slash"] {
            let mut p = authorize_params();
            p.scope = Some(bad.into());
            assert!(matches!(
                server().validate_authorize(&p).await.unwrap_err(),
                OidcServerError::MalformedScope
            ));
        }
    }

    #[tokio::test]
    async fn narrows_scope_to_allowed_set() {
        let mut p = authorize_params();
        p.scope = Some("openid admin email".into());
        assert_eq!(
            server().validate_authorize(&p).await.unwrap().scope,
            "openid email"
        );
    }

    #[tokio::test]
    async fn rejects_unsupported_response_mode_and_jar() {
        let mut p = authorize_params();
        p.response_mode = Some("form_post".into());
        assert!(matches!(
            server().validate_authorize(&p).await.unwrap_err(),
            OidcServerError::UnsupportedResponseMode(_)
        ));

        let mut p = authorize_params();
        p.response_mode = Some("query".into());
        assert!(server().validate_authorize(&p).await.is_ok());

        let mut p = authorize_params();
        p.request = Some("eyJhbGciOi...".into());
        assert!(matches!(
            server().validate_authorize(&p).await.unwrap_err(),
            OidcServerError::RequestNotSupported
        ));

        let mut p = authorize_params();
        p.request_uri = Some("https://rp.example/jar".into());
        assert!(matches!(
            server().validate_authorize(&p).await.unwrap_err(),
            OidcServerError::RequestUriNotSupported
        ));
    }

    #[tokio::test]
    async fn passes_through_optional_oidc_params() {
        let mut p = authorize_params();
        p.prompt = Some("none".into());
        p.login_hint = Some("+15550100".into());
        p.max_age = Some(300);
        let v = server().validate_authorize(&p).await.unwrap();
        assert_eq!(v.prompt.as_deref(), Some("none"));
        assert_eq!(v.login_hint.as_deref(), Some("+15550100"));
        assert_eq!(v.max_age, Some(300));
    }
}

mod issue_code {
    use super::*;

    #[tokio::test]
    async fn builds_redirect_with_state() {
        let server = server();
        let validated = server
            .validate_authorize(&authorize_params())
            .await
            .unwrap();
        let url = server.issue_code(validated, "u1", json!({})).await.unwrap();
        assert!(url.starts_with("https://app.example/cb?code="));
        assert!(url.contains("state=st"));
    }

    #[tokio::test]
    async fn revalidates_client_and_redirect() {
        let server = server();

        let mut validated = server
            .validate_authorize(&authorize_params())
            .await
            .unwrap();
        validated.redirect_uri = "https://evil.example/cb".into();
        assert!(matches!(
            server
                .issue_code(validated, "u1", json!({}))
                .await
                .unwrap_err(),
            OidcServerError::UnregisteredRedirectUri(_)
        ));

        let mut validated = server
            .validate_authorize(&authorize_params())
            .await
            .unwrap();
        validated.client_id = "not-registered".into();
        assert!(matches!(
            server
                .issue_code(validated, "u1", json!({}))
                .await
                .unwrap_err(),
            OidcServerError::UnknownClient(_)
        ));
    }

    #[tokio::test]
    async fn rejects_tampered_stash_fields() {
        let server = server();

        let mut validated = server
            .validate_authorize(&authorize_params())
            .await
            .unwrap();
        validated.code_challenge = "short".into();
        assert!(matches!(
            server
                .issue_code(validated, "u1", json!({}))
                .await
                .unwrap_err(),
            OidcServerError::MalformedCodeChallenge
        ));

        let mut validated = server
            .validate_authorize(&authorize_params())
            .await
            .unwrap();
        validated.scope = "email".into();
        assert!(matches!(
            server
                .issue_code(validated, "u1", json!({}))
                .await
                .unwrap_err(),
            OidcServerError::MalformedScope
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_subject() {
        let server = server();
        for bad in ["", "über-sub", &"x".repeat(256)] {
            let validated = server
                .validate_authorize(&authorize_params())
                .await
                .unwrap();
            assert!(matches!(
                server
                    .issue_code(validated, bad, json!({}))
                    .await
                    .unwrap_err(),
                OidcServerError::Internal(_)
            ));
        }
    }
}

mod exchange {
    use super::*;

    #[tokio::test]
    async fn happy_path_mints_verifiable_id_token() {
        let server = server();
        let code = issue(&server).await;
        let payload = server.exchange(token_request(&code)).await.unwrap();

        assert_eq!(payload.token_type, "Bearer");
        assert_eq!(payload.expires_in, 3600);
        assert_eq!(payload.scope, "openid email profile");
        assert!(!payload.access_token.is_empty());

        let header = jsonwebtoken::decode_header(&payload.id_token).unwrap();
        assert_eq!(header.kid.as_deref(), Some("k1"));

        let jwk = &server.jwks_document()["keys"][0];
        let key = jsonwebtoken::DecodingKey::from_rsa_components(
            jwk["n"].as_str().unwrap(),
            jwk["e"].as_str().unwrap(),
        )
        .unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&["cid"]);
        let data = jsonwebtoken::decode::<serde_json::Value>(&payload.id_token, &key, &validation)
            .unwrap();
        assert_eq!(data.claims["sub"], "u1");
        assert_eq!(data.claims["email"], "u@e.co");
        assert_eq!(data.claims["email_verified"], true);
        assert_eq!(data.claims["nonce"], "n-123");
    }

    #[tokio::test]
    async fn accepts_basic_auth() {
        let server = server();
        let code = issue(&server).await;
        let mut req = token_request(&code);
        req.client_id = None;
        req.client_secret = None;
        req.basic_auth = Some(("cid".into(), "secret".into()));
        assert!(server.exchange(req).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_mixed_auth_methods() {
        let server = server();
        let code = issue(&server).await;

        let mut req = token_request(&code);
        req.basic_auth = Some(("cid".into(), "secret".into()));
        assert!(matches!(
            server.exchange(req).await.unwrap_err(),
            OidcServerError::InvalidClient
        ));

        let code = issue(&server).await;
        let mut req = token_request(&code);
        req.client_secret = None;
        req.client_id = Some("other-cid".into());
        req.basic_auth = Some(("cid".into(), "secret".into()));
        assert!(matches!(
            server.exchange(req).await.unwrap_err(),
            OidcServerError::InvalidClient
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_secret() {
        let server = server();
        let code = issue(&server).await;
        let mut req = token_request(&code);
        req.client_secret = Some("wrong".into());
        assert!(matches!(
            server.exchange(req).await.unwrap_err(),
            OidcServerError::InvalidClient
        ));
    }

    #[tokio::test]
    async fn rejects_reused_code() {
        let server = server();
        let code = issue(&server).await;
        server.exchange(token_request(&code)).await.unwrap();
        assert!(matches!(
            server.exchange(token_request(&code)).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[tokio::test]
    async fn rejects_expired_code() {
        let mut c = config();
        c.code_ttl = Some(Duration::ZERO);
        let server = server_with(c).unwrap();
        let code = issue(&server).await;
        assert!(matches!(
            server.exchange(token_request(&code)).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_verifier() {
        let server = server();
        let code = issue(&server).await;
        let mut req = token_request(&code);
        req.code_verifier = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        assert!(matches!(
            server.exchange(req).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[tokio::test]
    async fn rejects_redirect_mismatch() {
        let server = server();
        let code = issue(&server).await;
        let mut req = token_request(&code);
        req.redirect_uri = "https://app.example/other".into();
        assert!(matches!(
            server.exchange(req).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[tokio::test]
    async fn rejects_unsupported_grant_type() {
        let mut req = token_request("c");
        req.grant_type = "client_credentials".into();
        assert!(matches!(
            server().exchange(req).await.unwrap_err(),
            OidcServerError::UnsupportedGrantType(_)
        ));
    }

    #[tokio::test]
    async fn protocol_claims_win_over_consumer_collisions() {
        let server = server();
        let mut p = authorize_params();
        p.nonce = None;
        let validated = server.validate_authorize(&p).await.unwrap();
        let url = server
            .issue_code(
                validated,
                "real-sub",
                json!({
                    "sub": "forged-sub",
                    "iss": "https://evil.example",
                    "nonce": "forged-nonce",
                    "nbf": 99_999_999_999u64,
                    "auth_time": 1_700_000_000u64,
                    "phoneNumber": "+1-555-0100",
                }),
            )
            .await
            .unwrap();
        let payload = server
            .exchange(token_request(&code_from(&url)))
            .await
            .unwrap();

        let claims = jwt_payload(&payload.id_token);
        assert_eq!(claims["sub"], "real-sub");
        assert_eq!(claims["iss"], ISSUER);
        assert!(claims.get("nonce").is_none());
        assert!(claims.get("nbf").is_none());
        assert_eq!(claims["auth_time"], 1_700_000_000u64);
        assert_eq!(claims["phoneNumber"], "+1-555-0100");
    }
}

mod store_contract {
    use super::*;

    fn grant() -> PendingGrant {
        PendingGrant {
            request: ValidatedAuthorizeRequest {
                client_id: "cid".into(),
                redirect_uri: "https://app.example/cb".into(),
                scope: "openid".into(),
                state: None,
                code_challenge: "ch".into(),
                nonce: None,
                prompt: None,
                login_hint: None,
                max_age: None,
                id_token_hint: None,
                acr_values: None,
                display: None,
                ui_locales: None,
                claims_param: None,
            },
            subject: "u1".into(),
            claims: serde_json::Map::new(),
        }
    }

    #[tokio::test]
    async fn take_is_single_use() {
        let store = TestStore::default();
        store
            .put("c1".into(), grant(), Duration::from_secs(60))
            .await
            .unwrap();
        assert!(store.take("c1").await.unwrap().is_some());
        assert!(store.take("c1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn take_expired_is_none() {
        let store = TestStore::default();
        store
            .put("c1".into(), grant(), Duration::ZERO)
            .await
            .unwrap();
        assert!(store.take("c1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn extreme_ttl_does_not_panic() {
        let store = TestStore::default();
        store
            .put("c1".into(), grant(), Duration::MAX)
            .await
            .unwrap();
        assert!(store.take("c1").await.unwrap().is_some());
    }
}

mod registry_contract {
    use super::*;
    use arche::oidc::server::ClientRegistry;

    #[tokio::test]
    async fn default_verify_secret_compares_plaintext() {
        let registry = clients();
        assert!(registry.verify_secret("cid", "secret").await.unwrap());
        assert!(!registry.verify_secret("cid", "wrong").await.unwrap());
        assert!(!registry.verify_secret("nope", "secret").await.unwrap());
    }
}

mod signing_key {
    use super::*;
    use arche::oidc::server::TokenSigner;

    #[test]
    fn from_pem_builds_jwk_components() {
        let key = signer();
        let jwk = &key.jwks()["keys"][0];
        assert_eq!(jwk["kid"], "k1");
        assert_eq!(jwk["alg"], "RS256");
        assert!(!jwk["n"].as_str().unwrap().is_empty());
        assert_eq!(jwk["e"], "AQAB");
    }

    #[test]
    fn from_pem_rejects_garbage_and_empty_kid() {
        assert!(SigningKey::from_pem("k1", "not a pem").is_err());
        assert!(SigningKey::from_pem("", &pem()).is_err());
    }

    #[test]
    fn debug_redacts_key_material() {
        let out = format!("{:?}", signer());
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("MIIE"));
    }

    #[tokio::test]
    async fn sign_produces_verifiable_rs256_signature() {
        let key = signer();
        assert_eq!(key.kid(), "k1");

        let message = b"header.payload";
        let signature = key.sign(message).await.unwrap();

        let jwk = &key.jwks()["keys"][0];
        let decoding = jsonwebtoken::DecodingKey::from_rsa_components(
            jwk["n"].as_str().unwrap(),
            jwk["e"].as_str().unwrap(),
        )
        .unwrap();
        let sig_b64 = URL_SAFE_NO_PAD.encode(&signature);
        assert!(
            jsonwebtoken::crypto::verify(
                &sig_b64,
                message,
                &decoding,
                jsonwebtoken::Algorithm::RS256
            )
            .unwrap()
        );
    }

    #[test]
    fn rsa_public_jwk_matches_private_derivation() {
        use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};

        let private = rsa::RsaPrivateKey::from_pkcs8_pem(&pem()).unwrap();
        let public_pem = private
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();

        let helper_jwk = rsa_public_jwk("k1", &public_pem).unwrap();
        assert_eq!(helper_jwk, signer().jwks()["keys"][0]);
    }
}

mod types {
    use super::*;

    fn basic_header(id: &str, secret: &str) -> String {
        use base64::engine::general_purpose::STANDARD;
        format!("Basic {}", STANDARD.encode(format!("{id}:{secret}")))
    }

    #[test]
    fn parse_basic_form_urldecodes_credentials() {
        let (id, secret) =
            TokenRequest::parse_basic_authorization(&basic_header("client%3A1", "a%2Fb%2Bc+d"))
                .unwrap();
        assert_eq!(id, "client:1");
        assert_eq!(secret, "a/b+c d");
    }

    #[test]
    fn parse_basic_scheme_is_case_insensitive() {
        use base64::engine::general_purpose::STANDARD;
        let payload = STANDARD.encode("c:s");
        for scheme in ["Basic", "basic", "BASIC", "BaSiC"] {
            let got = TokenRequest::parse_basic_authorization(&format!("{scheme} {payload}"));
            assert_eq!(got, Some(("c".into(), "s".into())), "scheme {scheme}");
        }
    }

    #[test]
    fn parse_basic_rejects_non_basic_and_malformed() {
        use base64::engine::general_purpose::STANDARD;
        assert!(TokenRequest::parse_basic_authorization("Bearer abc").is_none());
        assert!(TokenRequest::parse_basic_authorization("Basic !!!not-base64!!!").is_none());
        assert!(
            TokenRequest::parse_basic_authorization(&format!(
                "Basic {}",
                STANDARD.encode("nocolon")
            ))
            .is_none()
        );
        assert!(TokenRequest::parse_basic_authorization("Basic").is_none());
    }

    #[test]
    fn redirect_url_encodes_and_refuses_unsafe() {
        let err = OidcServerError::MissingPkce;
        let url = err
            .redirect_url("https://app.example/cb", Some("a b&c"))
            .unwrap();
        assert!(url.starts_with("https://app.example/cb?"));
        assert!(url.contains("error=invalid_request"));
        assert!(url.contains("error_description="));
        assert!(url.contains("state=a+b%26c"));

        let url = err.redirect_url("https://app.example/cb", None).unwrap();
        assert!(!url.contains("state="));

        assert!(err.redirect_url("not a url", None).is_err());
        assert!(
            OidcServerError::UnknownClient("cid".into())
                .redirect_url("https://app.example/cb", None)
                .is_err()
        );
    }

    #[test]
    fn user_phase_errors_are_redirectable_with_spec_codes() {
        for (err, code) in [
            (OidcServerError::AccessDenied, "access_denied"),
            (OidcServerError::LoginRequired, "login_required"),
            (OidcServerError::InteractionRequired, "interaction_required"),
            (OidcServerError::ConsentRequired, "consent_required"),
        ] {
            assert!(err.redirectable());
            assert_eq!(err.error_code(), code);
            let url = err
                .redirect_url("https://app.example/cb", Some("st"))
                .unwrap();
            assert!(url.contains(&format!("error={code}")));
        }
    }

    #[test]
    fn token_payload_debug_redacts_tokens() {
        let payload = TokenPayload {
            access_token: "atk-secret".into(),
            id_token: "idt-secret".into(),
            token_type: "Bearer".into(),
            expires_in: 3600,
            scope: "openid".into(),
            refresh_token: Some("rt-secret".into()),
        };
        let out = format!("{payload:?}");
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("atk-secret"));
        assert!(!out.contains("rt-secret"));
        assert!(!out.contains("idt-secret"));
    }

    #[test]
    fn client_registration_debug_redacts_secret() {
        let c = ClientRegistration {
            client_id: "cid".into(),
            client_secret: "topsecret".into(),
            redirect_uris: vec!["https://a/cb".into()],
        };
        let out = format!("{c:?}");
        assert!(out.contains("<redacted>"));
        assert!(!out.contains("topsecret"));
    }

    #[test]
    fn discovery_standard_derives_endpoints_from_issuer() {
        let doc = DiscoveryDocument::standard("https://id.example.com/");
        assert_eq!(doc.issuer, "https://id.example.com");
        assert_eq!(
            doc.authorization_endpoint,
            "https://id.example.com/authorize"
        );
        assert_eq!(doc.token_endpoint, "https://id.example.com/token");
        assert_eq!(doc.jwks_uri, "https://id.example.com/jwks");
        assert_eq!(doc.code_challenge_methods_supported, ["S256"]);
    }

    #[test]
    fn discovery_reserves_optional_endpoints_omitted_until_set() {
        // Reserved endpoints are omitted by default; each emits exactly its key when set.
        let v = serde_json::to_value(DiscoveryDocument::standard(ISSUER)).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "userinfo_endpoint",
            "end_session_endpoint",
            "revocation_endpoint",
            "introspection_endpoint",
        ] {
            assert!(!obj.contains_key(key), "{key} should be omitted by default");
        }

        let mut doc = DiscoveryDocument::standard(ISSUER);
        doc.userinfo_endpoint = Some(format!("{ISSUER}/userinfo"));
        doc.end_session_endpoint = Some(format!("{ISSUER}/logout"));
        doc.revocation_endpoint = Some(format!("{ISSUER}/revoke"));
        doc.introspection_endpoint = Some(format!("{ISSUER}/introspect"));
        let v = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["userinfo_endpoint"], format!("{ISSUER}/userinfo"));
        assert_eq!(v["end_session_endpoint"], format!("{ISSUER}/logout"));
        assert_eq!(v["revocation_endpoint"], format!("{ISSUER}/revoke"));
        assert_eq!(v["introspection_endpoint"], format!("{ISSUER}/introspect"));
    }

    #[test]
    fn discovery_advertises_only_what_arche_actually_supports() {
        let doc = DiscoveryDocument::standard(ISSUER);
        // arche accepts only response_mode=query and rejects request_uri (JAR),
        // so it overrides the spec's optimistic defaults instead of omitting these.
        assert_eq!(doc.response_modes_supported, ["query"]);
        assert!(!doc.request_uri_parameter_supported);
    }
}

mod refresh {
    use super::*;

    async fn issue_offline_code(server: &TestServer) -> String {
        let mut p = authorize_params();
        p.scope = Some("openid offline_access".into());
        let validated = server.validate_authorize(&p).await.unwrap();
        let url = server
            .issue_code(validated, "u1", json!({ "email": "u@e.co" }))
            .await
            .unwrap();
        code_from(&url)
    }

    #[tokio::test]
    async fn offline_access_scope_issues_a_refresh_token() {
        let server = server_with(offline_config()).unwrap();
        let code = issue_offline_code(&server).await;
        let payload = server.exchange(token_request(&code)).await.unwrap();
        assert!(payload.refresh_token.is_some());
        assert!(payload.scope.split(' ').any(|s| s == "offline_access"));
    }

    #[tokio::test]
    async fn no_offline_access_means_no_refresh_token() {
        // Even on an offline-capable server, a code without the scope gets none.
        let server = server_with(offline_config()).unwrap();
        let code = issue(&server).await; // default scope: openid email profile
        let payload = server.exchange(token_request(&code)).await.unwrap();
        assert!(payload.refresh_token.is_none());
    }

    #[tokio::test]
    async fn refresh_grant_rotates_and_mints_fresh_tokens() {
        let server = server_with(offline_config()).unwrap();
        let code = issue_offline_code(&server).await;
        let first = server.exchange(token_request(&code)).await.unwrap();
        let rt1 = first.refresh_token.clone().unwrap();

        let second = server.exchange(refresh_request(&rt1)).await.unwrap();
        let rt2 = second.refresh_token.clone().unwrap();

        // rotated: a new refresh token, and a fresh, verifiable ID token
        assert_ne!(rt1, rt2);
        assert!(!second.access_token.is_empty());
        let claims = jwt_payload(&second.id_token);
        assert_eq!(claims["sub"], "u1");
        assert_eq!(claims["iss"], ISSUER);
        // nonce is one-time — not replayed into refresh-minted ID tokens
        assert!(claims.get("nonce").is_none());
        // consumer claims are re-minted from the stored grant
        assert_eq!(claims["email"], "u@e.co");
    }

    #[tokio::test]
    async fn reused_refresh_token_is_rejected() {
        let server = server_with(offline_config()).unwrap();
        let code = issue_offline_code(&server).await;
        let rt = server
            .exchange(token_request(&code))
            .await
            .unwrap()
            .refresh_token
            .unwrap();

        server.exchange(refresh_request(&rt)).await.unwrap(); // rotates rt away
        assert!(matches!(
            server.exchange(refresh_request(&rt)).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[tokio::test]
    async fn refresh_token_is_bound_to_its_client() {
        let clients = TestRegistry(vec![
            ClientRegistration {
                client_id: "cid".into(),
                client_secret: "secret".into(),
                redirect_uris: vec!["https://app.example/cb".into()],
            },
            ClientRegistration {
                client_id: "cid2".into(),
                client_secret: "secret2".into(),
                redirect_uris: vec!["https://app.example/cb".into()],
            },
        ]);
        let server = OidcServer::new(
            offline_config(),
            clients,
            signer(),
            TestTokens,
            TestStore::default(),
            TestRefreshStore::default(),
        )
        .unwrap();

        let code = issue_offline_code(&server).await; // issued to cid
        let rt = server
            .exchange(token_request(&code))
            .await
            .unwrap()
            .refresh_token
            .unwrap();

        // cid2 authenticates fine but the token isn't theirs → invalid_grant
        let mut req = refresh_request(&rt);
        req.client_id = Some("cid2".into());
        req.client_secret = Some("secret2".into());
        assert!(matches!(
            server.exchange(req).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[tokio::test]
    async fn expired_refresh_token_is_rejected() {
        let mut c = offline_config();
        c.refresh_token_ttl = Some(Duration::ZERO);
        let server = server_with(c).unwrap();
        let code = issue_offline_code(&server).await;
        let rt = server
            .exchange(token_request(&code))
            .await
            .unwrap()
            .refresh_token
            .unwrap();
        assert!(matches!(
            server.exchange(refresh_request(&rt)).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[tokio::test]
    async fn refresh_without_a_token_is_rejected() {
        let server = server_with(offline_config()).unwrap();
        let mut req = refresh_request("");
        req.refresh_token = None;
        assert!(matches!(
            server.exchange(req).await.unwrap_err(),
            OidcServerError::InvalidGrant(_)
        ));
    }

    #[test]
    fn discovery_advertises_the_refresh_grant() {
        let doc = DiscoveryDocument::standard(ISSUER);
        assert!(
            doc.grant_types_supported
                .iter()
                .any(|g| g == "refresh_token")
        );
    }
}
