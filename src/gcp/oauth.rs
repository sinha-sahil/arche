use crate::oidc::ProviderMetadata;

pub fn google() -> ProviderMetadata {
    ProviderMetadata {
        key: "google".into(),
        issuers: vec![
            "https://accounts.google.com".into(),
            "accounts.google.com".into(),
        ],
        auth_endpoint: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        token_endpoint: "https://oauth2.googleapis.com/token".into(),
        jwks_endpoint: "https://www.googleapis.com/oauth2/v3/certs".into(),
        extra_auth_params: vec![
            ("prompt".into(), "select_account".into()),
            ("access_type".into(), "online".into()),
        ],
    }
}
