use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::de::DeserializeOwned;

use crate::error::AppError;

use super::jwks::JwksCache;
use super::{ProviderMetadata, default_http_client, dependency_label, reject_token};

const CLOCK_SKEW_SECS: u64 = 60;

#[derive(Clone)]
pub struct Verifier {
    http: reqwest::Client,
    keys: JwksCache,
    issuers: Vec<String>,
    dependency: String,
}

impl Verifier {
    pub fn new(provider: &ProviderMetadata) -> Result<Self, AppError> {
        let http = default_http_client("OIDC verifier")?;
        Ok(Self::with_http_client(provider, http))
    }

    pub fn with_http_client(provider: &ProviderMetadata, http: reqwest::Client) -> Self {
        let dependency = dependency_label(&provider.key);
        Self {
            http,
            keys: JwksCache::new(provider.jwks_endpoint.clone(), dependency.clone()),
            issuers: provider.issuers.clone(),
            dependency,
        }
    }

    pub async fn verify_id_token<C: DeserializeOwned>(
        &self,
        id_token: &str,
        audiences: &[&str],
    ) -> Result<C, AppError> {
        let claims = self.validated_claims(id_token, audiences).await?;
        serde_json::from_value(claims).map_err(|e| {
            AppError::internal_error(
                format!("verified id token does not fit requested type: {e}"),
                None,
            )
        })
    }

    async fn validated_claims(
        &self,
        id_token: &str,
        audiences: &[&str],
    ) -> Result<serde_json::Value, AppError> {
        let header =
            decode_header(id_token).map_err(|e| reject_token(format!("header decode: {e}")))?;
        if header.alg != Algorithm::RS256 {
            return Err(reject_token(format!(
                "unsupported algorithm {:?}",
                header.alg
            )));
        }
        let kid = header.kid.ok_or_else(|| reject_token("missing kid"))?;

        let jwk = self.keys.lookup(&self.http, &kid).await?;

        let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|e| {
            AppError::dependency_failed_permanent(&self.dependency, format!("jwk components: {e}"))
        })?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud"]);
        validation.set_issuer(&self.issuers);
        validation.set_audience(audiences);
        validation.leeway = CLOCK_SKEW_SECS;
        validation.validate_nbf = true;

        let token_data = decode::<serde_json::Value>(id_token, &decoding_key, &validation)
            .map_err(|e| reject_token(format!("validation failed: {e}")))?;

        Ok(token_data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    use std::sync::LazyLock;
    use std::time::{SystemTime, UNIX_EPOCH};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ACME_ISSUER: &str = "https://id.acme.example";

    const TEST_KID: &str = "test-key-1";

    // Throwaway key generated for these tests — has never signed anything real.
    const TEST_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
        MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQCYyK/iTxWeNWSb
        aS/yPPBIQ+jDbGyb8pt2ycklw7wEgaIdUvZgDeC57QENCg5lgs+g6OPRRsQY9fFG
        lE6wKlZ4Zgq9kzRFjIAjXZkvD/H7bqlUdX0DlJXlYwMnGj2BR3DDbNQuRSlK4gWa
        ZN5SYSVaBHsr3c89nN3GJH6IgikypsRc0mOEwbUdULs09ahSai7D7bSCCvCS5Jed
        jAk10NqIMYMGjhlKpOWA+WcnHXpATSHtfampJq0s77bJej4MyQbGFims5XuuDmJ7
        AF5rR4BndaRfaN323DLiKgR+UQKRMrhmBpyR35Yj/0pmo1XM713N4SLny1lq/oRH
        9jAr2TPvAgMBAAECggEAL0w9iulhr2MnHKeBJNQxrKV9SvZnXxXJhAou36aLL7fz
        +HEE/bJ+JgDViPRahZlr7ov6bwCh13pX8boa7BWHRGmOnKaUEY3P42Ln97ZPer+E
        4zUl+PRIPUWcJcBNVxbHNXCc9SALCvgStPvSCZ2yYv4tJWTa8d98lokYtOjamSel
        19RwTIA7rCFQqVpz2yR9tdAyZPl/+KOma1A+GwG6H+/G7tx7AqWyaMQiziEyGYKp
        hD0yzlRBfUEU+HxN+SkhvpwONBQSPRcCKHO/6/eF2l2zQWwhA5xE9oLF/g3FL27W
        AqRoLBgOO0HHmhBU7hjTooNwG5d2fSAhObIlRcxshQKBgQDQVNJNErEQWwaUYtde
        2MT2EH5duWyPYaGJr3AhToTHACrV30eO5yuy9mdaBuyUSo/PrI7F72qX24ZNO9xk
        1uxvWMtYwG15UZiU6l7zNgta86xdz0JCeADPbf1n2i8T7QAHsm9gZ0/jEGg+6/03
        XMwtuA4LiR70QwvBFXXmP7IPRQKBgQC7viGPww84zRp6KrBWaemmkxIqSL/Uo5EV
        UyBmHjmyUGNoWIZOpAsZNg3/MS/7B4PVWxMtpcwpL1YEALbiEZPMrzRvJS7j6xJj
        mYCM7t8XRwK5SmMuS+VK7V319bEf17kpLAlq4mPPX+2+q0kn7Xs1PeEjz48wlqG7
        TpJLhpG/owKBgQC/G8BbUXU6MrZDcrRs3l839oNlSM6sbPw5iMVM2HF299FTpmJH
        VgrBPcYrUMS/d/KaqInES082BPwbZ3lSy9HShtrrDIKgUtisap81bnNWOMf6ukDn
        JpxfrF9UYFLlbXikluwSvFMNUaS/a846dhcbLYc8z8mkesiSlDQ2RmH6HQKBgQCm
        8fJcKUMO6mvB+NXncbUAl8VObnSOvIhV4x5rUDNUGeHbtuRvZ7YqzAN0SqP04IDd
        p2gNbmJ2uQ4O7yexLZo1KBNDRlhE+hLXGHfUWtFsnIuSgtBhKcISd7LW9Yx02Vpg
        fzU8o2XH0PDTXPLnm2i1NnpOYtJcjYXxznOOz3IpawKBgQCN8jXNUlVpc7XIA3mY
        zX3MzefeYsmqNGe18oRDMVOCMMY55p8f9t48pXNUKpcm5eFKMKGeWxHPR05r+guL
        /ICg2hMWnxEem3Foq/KGLFjGYnE1I1gM/4CPOYtGkLcx1FfaE4Y7Cv3fonA5CcLL
        /XuwcOxx4KaoCDZK4kU0/Lsw8A==
        -----END PRIVATE KEY-----
        ";

    fn test_key_pem() -> String {
        TEST_PRIVATE_KEY_PEM
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn test_jwk_components() -> &'static (String, String) {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use rsa::RsaPrivateKey;
        use rsa::pkcs8::DecodePrivateKey;
        use rsa::traits::PublicKeyParts;

        static COMPONENTS: LazyLock<(String, String)> = LazyLock::new(|| {
            let priv_key =
                RsaPrivateKey::from_pkcs8_pem(&test_key_pem()).expect("test private key parses");
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
        let key = EncodingKey::from_rsa_pem(test_key_pem().as_bytes()).unwrap();
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
    async fn verify_happy_path_returns_caller_type() {
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
    async fn verify_accepts_any_allowed_issuer() {
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
    async fn verify_rejects_wrong_audience() {
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
    async fn verify_rejects_expired_token() {
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
    async fn verify_rejects_issuer_not_in_allow_list() {
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
    async fn verify_rejects_unknown_kid() {
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
    async fn verify_jwks_unavailable_when_cold_cache_and_5xx() {
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
    async fn verify_concurrent_cold_cache_fetches_once() {
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

    #[tokio::test]
    async fn generic_provider_end_to_end() {
        let server = MockServer::start().await;
        let issuer = server.uri();

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{issuer}/authorize"),
                "token_endpoint": format!("{issuer}/token"),
                "jwks_uri": format!("{issuer}/certs"),
            })))
            .mount(&server)
            .await;
        mount_jwks(&server, jwks_response(), 200).await;

        let id_token = sign_test_token(
            happy_claims(&issuer, "e2e-client"),
            TEST_KID,
            Algorithm::RS256,
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "atk",
                "id_token": id_token,
                "expires_in": 3600u64,
                "token_type": "Bearer",
            })))
            .mount(&server)
            .await;

        let provider = ProviderMetadata::discover("e2e", &issuer, &reqwest::Client::new())
            .await
            .expect("discover");

        let client = crate::oidc::OidcClient::new(
            provider.clone(),
            crate::oidc::OidcConfig {
                client_id: "e2e-client".into(),
                client_secret: "sec".into(),
                redirect_uri: "https://app.example/cb".into(),
                scopes: None,
            },
        )
        .expect("client");

        let url = client.auth_url("st", "ch");
        assert!(url.starts_with(&format!("{issuer}/authorize?")));

        let tokens = client
            .exchange_code("code", "verifier")
            .await
            .expect("exchange");

        let claims: serde_json::Value =
            Verifier::with_http_client(&provider, reqwest::Client::new())
                .verify_id_token(&tokens.id_token, &["e2e-client"])
                .await
                .expect("verify");
        assert_eq!(claims["sub"], "1234567890");
        assert_eq!(claims["iss"], issuer);
    }
}
