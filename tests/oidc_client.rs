mod common;

use arche::error::AppError;
use arche::oidc::{OidcClient, OidcConfig, ProviderMetadata, Verifier};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACME_ISSUER: &str = "https://id.acme.example";
const TEST_KID: &str = "test-key-1";

fn acme_provider() -> ProviderMetadata {
    ProviderMetadata {
        key: "acme".into(),
        issuers: vec![ACME_ISSUER.into()],
        auth_endpoint: format!("{ACME_ISSUER}/authorize"),
        token_endpoint: format!("{ACME_ISSUER}/token"),
        jwks_endpoint: format!("{ACME_ISSUER}/jwks"),
        extra_auth_params: Vec::new(),
    }
}

fn config() -> OidcConfig {
    OidcConfig {
        client_id: "client-id".into(),
        client_secret: "secret".into(),
        redirect_uri: "https://app.example/cb?path=x".into(),
        scopes: None,
    }
}

mod auth_url {
    use super::*;

    #[test]
    fn contains_required_params_and_encodes_redirect() {
        let client = OidcClient::new(acme_provider(), config()).unwrap();
        let url = client.auth_url("state-abc", "challenge-xyz");
        assert!(url.starts_with("https://id.acme.example/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-id"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fapp.example%2Fcb%3Fpath%3Dx"));
        assert!(url.contains("scope=openid+email+profile"));
        assert!(url.contains("state=state-abc"));
        assert!(url.contains("code_challenge=challenge-xyz"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(!url.contains("prompt="));
    }

    #[test]
    fn appends_provider_extra_params() {
        let mut provider = acme_provider();
        provider.extra_auth_params = vec![("prompt".into(), "consent".into())];
        let client = OidcClient::new(provider, config()).unwrap();
        assert!(client.auth_url("s", "c").contains("prompt=consent"));
    }

    #[test]
    fn custom_scopes() {
        let client = OidcClient::new(
            acme_provider(),
            OidcConfig {
                scopes: Some("openid email".into()),
                ..config()
            },
        )
        .unwrap();
        assert!(client.auth_url("s", "c").contains("scope=openid+email"));
    }

    #[test]
    fn encodes_special_chars_in_state() {
        let client = OidcClient::new(acme_provider(), config()).unwrap();
        assert!(
            client
                .auth_url("a b&c=d", "x")
                .contains("state=a+b%26c%3Dd")
        );
    }

    #[test]
    fn new_rejects_invalid_endpoints() {
        let mut provider = acme_provider();
        provider.auth_endpoint = "not a url".into();
        let err = OidcClient::new(provider, config()).unwrap_err();
        assert!(format!("{err:?}").contains("auth_endpoint"));

        let mut provider = acme_provider();
        provider.token_endpoint = "not a url".into();
        let err = OidcClient::new(provider, config()).unwrap_err();
        assert!(format!("{err:?}").contains("token_endpoint"));
    }
}

mod exchange {
    use super::*;

    fn mock_token_client(server: &MockServer) -> OidcClient {
        let mut provider = acme_provider();
        provider.token_endpoint = format!("{}/token", server.uri());
        OidcClient::new(provider, config()).unwrap()
    }

    #[tokio::test]
    async fn happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "atk",
                "id_token": "itk",
                "expires_in": 3600u64,
                "token_type": "Bearer",
                "scope": "openid email profile",
            })))
            .mount(&server)
            .await;

        let resp = mock_token_client(&server)
            .exchange_code("code-123", "verifier-456")
            .await
            .unwrap();
        assert_eq!(resp.access_token, "atk");
        assert_eq!(resp.id_token, "itk");
        assert_eq!(resp.expires_in, 3600);
        assert!(resp.refresh_token.is_none());
    }

    #[tokio::test]
    async fn http_4xx_is_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad code"))
            .mount(&server)
            .await;
        let err = mock_token_client(&server)
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn http_5xx_is_dependency_failed_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream"))
            .mount(&server)
            .await;
        let err = mock_token_client(&server)
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn malformed_json_is_dependency_failed_permanent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let err = mock_token_client(&server)
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: false,
                ..
            }
        ));
    }
}

mod discovery {
    use super::*;

    fn discovery_doc(issuer: &str) -> serde_json::Value {
        json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "jwks_uri": format!("{issuer}/jwks"),
        })
    }

    async fn mount_discovery(server: &MockServer, body: serde_json::Value, status: u16) {
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn happy_path() {
        let server = MockServer::start().await;
        let issuer = server.uri();
        mount_discovery(&server, discovery_doc(&issuer), 200).await;

        let p = ProviderMetadata::discover("acme", &issuer, &reqwest::Client::new())
            .await
            .unwrap();
        assert_eq!(p.key, "acme");
        assert_eq!(p.issuers, vec![issuer.clone()]);
        assert_eq!(p.auth_endpoint, format!("{issuer}/authorize"));
        assert_eq!(p.token_endpoint, format!("{issuer}/token"));
        assert_eq!(p.jwks_endpoint, format!("{issuer}/jwks"));
        assert!(p.extra_auth_params.is_empty());
    }

    #[tokio::test]
    async fn trailing_slash_issuer_matches() {
        let server = MockServer::start().await;
        let issuer = server.uri();
        mount_discovery(&server, discovery_doc(&issuer), 200).await;

        let p = ProviderMetadata::discover("acme", &format!("{issuer}/"), &reqwest::Client::new())
            .await
            .unwrap();
        assert_eq!(p.issuers, vec![issuer]);
    }

    #[tokio::test]
    async fn rejects_issuer_mismatch() {
        let server = MockServer::start().await;
        mount_discovery(&server, discovery_doc("https://evil.example"), 200).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn http_404_is_permanent() {
        let server = MockServer::start().await;
        mount_discovery(&server, json!({}), 404).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn http_503_is_retryable() {
        let server = MockServer::start().await;
        mount_discovery(&server, json!({}), 503).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn malformed_doc_is_permanent() {
        let server = MockServer::start().await;
        mount_discovery(&server, json!({"issuer": "x"}), 200).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: false,
                ..
            }
        ));
    }
}

mod verify {
    use super::*;

    fn test_jwk_components() -> &'static (String, String) {
        use rsa::RsaPrivateKey;
        use rsa::pkcs8::DecodePrivateKey;
        use rsa::traits::PublicKeyParts;

        static COMPONENTS: LazyLock<(String, String)> = LazyLock::new(|| {
            let priv_key =
                RsaPrivateKey::from_pkcs8_pem(&common::pem()).expect("test private key parses");
            let n = URL_SAFE_NO_PAD.encode(priv_key.n().to_bytes_be());
            let e = URL_SAFE_NO_PAD.encode(priv_key.e().to_bytes_be());
            (n, e)
        });
        &COMPONENTS
    }

    fn jwks_response() -> serde_json::Value {
        let (n, e) = test_jwk_components();
        json!({
            "keys": [{
                "kid": TEST_KID,
                "kty": "RSA",
                "alg": "RS256",
                "use": "sig",
                "n": n,
                "e": e,
            }]
        })
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn sign_test_token(claims: serde_json::Value, kid: &str, alg: Algorithm) -> String {
        let mut header = Header::new(alg);
        header.kid = Some(kid.to_string());
        let key = EncodingKey::from_rsa_pem(common::pem().as_bytes()).unwrap();
        encode(&header, &claims, &key).unwrap()
    }

    fn happy_claims(iss: &str, aud: &str) -> serde_json::Value {
        let now = now_secs();
        json!({
            "iss": iss,
            "aud": aud,
            "sub": "1234567890",
            "email": "user@example.com",
            "email_verified": true,
            "name": "Test User",
            "hd": "example.com",
            "exp": now + 600,
            "iat": now - 5,
        })
    }

    async fn mount_jwks(server: &MockServer, body: serde_json::Value, status: u16) {
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(body)
                    .insert_header("cache-control", "public, max-age=600"),
            )
            .mount(server)
            .await;
    }

    fn verifier_for(issuers: &[&str], server: &MockServer) -> Verifier {
        let provider = ProviderMetadata {
            key: "acme".into(),
            issuers: issuers.iter().map(|s| s.to_string()).collect(),
            auth_endpoint: format!("{ACME_ISSUER}/authorize"),
            token_endpoint: format!("{ACME_ISSUER}/token"),
            jwks_endpoint: format!("{}/certs", server.uri()),
            extra_auth_params: Vec::new(),
        };
        Verifier::with_http_client(&provider, reqwest::Client::new())
    }

    fn acme_verifier_pointing_at(server: &MockServer) -> Verifier {
        verifier_for(&[ACME_ISSUER], server)
    }

    #[tokio::test]
    async fn happy_path_returns_caller_type() {
        #[derive(serde::Deserialize)]
        struct MyClaims {
            sub: String,
            email: String,
            email_verified: bool,
            hd: String,
        }
        #[derive(Debug, serde::Deserialize)]
        struct WrongShape {
            #[allow(dead_code)]
            sub: u64,
        }

        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let token = sign_test_token(
            happy_claims(ACME_ISSUER, "client-id"),
            TEST_KID,
            Algorithm::RS256,
        );
        let verifier = acme_verifier_pointing_at(&server);

        let claims: MyClaims = verifier
            .verify_id_token(&token, &["client-id"])
            .await
            .expect("verify");
        assert_eq!(claims.sub, "1234567890");
        assert_eq!(claims.email, "user@example.com");
        assert!(claims.email_verified);
        assert_eq!(claims.hd, "example.com");

        let err = verifier
            .verify_id_token::<WrongShape>(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(!matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn accepts_any_allowed_issuer() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let issuers = ["https://a.example", "https://b.example"];
        for iss in issuers {
            let token = sign_test_token(happy_claims(iss, "client-id"), TEST_KID, Algorithm::RS256);
            verifier_for(&issuers, &server)
                .verify_id_token::<serde_json::Value>(&token, &["client-id"])
                .await
                .expect("verify");
        }
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let token = sign_test_token(
            happy_claims(ACME_ISSUER, "not-our-client"),
            TEST_KID,
            Algorithm::RS256,
        );
        let err = acme_verifier_pointing_at(&server)
            .verify_id_token::<serde_json::Value>(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let now = now_secs();
        let claims = json!({
            "iss": ACME_ISSUER,
            "aud": "client-id",
            "sub": "x",
            "exp": now - 600,
            "iat": now - 1200,
        });
        let token = sign_test_token(claims, TEST_KID, Algorithm::RS256);
        let err = acme_verifier_pointing_at(&server)
            .verify_id_token::<serde_json::Value>(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn rejects_issuer_not_in_allow_list() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let token = sign_test_token(
            happy_claims("https://evil.example", "client-id"),
            TEST_KID,
            Algorithm::RS256,
        );
        let err = acme_verifier_pointing_at(&server)
            .verify_id_token::<serde_json::Value>(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn rejects_unknown_kid() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let token = sign_test_token(
            happy_claims(ACME_ISSUER, "client-id"),
            "rotated-kid",
            Algorithm::RS256,
        );
        let err = acme_verifier_pointing_at(&server)
            .verify_id_token::<serde_json::Value>(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn jwks_unavailable_when_cold_cache_and_5xx() {
        let server = MockServer::start().await;
        mount_jwks(&server, json!({"keys": []}), 503).await;
        let token = sign_test_token(
            happy_claims(ACME_ISSUER, "client-id"),
            TEST_KID,
            Algorithm::RS256,
        );
        let err = acme_verifier_pointing_at(&server)
            .verify_id_token::<serde_json::Value>(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn concurrent_cold_cache_fetches_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(jwks_response())
                    .insert_header("cache-control", "max-age=600"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let verifier = acme_verifier_pointing_at(&server);
        let token = sign_test_token(
            happy_claims(ACME_ISSUER, "client-id"),
            TEST_KID,
            Algorithm::RS256,
        );

        let mut handles = Vec::new();
        for _ in 0..32 {
            let v = verifier.clone();
            let t = token.clone();
            handles.push(tokio::spawn(async move {
                v.verify_id_token::<serde_json::Value>(&t, &["client-id"])
                    .await
            }));
        }
        for h in handles {
            h.await.unwrap().expect("verify");
        }
    }
}
