#![allow(dead_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use arche::error::AppError;
use arche::oidc::server::{
    AccessTokenIssuer, ClientRegistration, ClientRegistry, CodeStore, IssuedAccessToken,
    PendingGrant, RefreshTokenStore,
};
use tokio::sync::Mutex;

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

pub fn pem() -> String {
    TEST_PRIVATE_KEY_PEM
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct TestRegistry(pub Vec<ClientRegistration>);

impl ClientRegistry for TestRegistry {
    async fn find(&self, client_id: &str) -> Result<Option<ClientRegistration>, AppError> {
        Ok(self.0.iter().find(|c| c.client_id == client_id).cloned())
    }
}

#[derive(Default)]
pub struct TestStore(Mutex<HashMap<String, (PendingGrant, Instant)>>);

impl CodeStore for TestStore {
    async fn put(&self, code: String, grant: PendingGrant, ttl: Duration) -> Result<(), AppError> {
        let expires_at = Instant::now()
            .checked_add(ttl)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(31_536_000));
        self.0.lock().await.insert(code, (grant, expires_at));
        Ok(())
    }

    async fn take(&self, code: &str) -> Result<Option<PendingGrant>, AppError> {
        Ok(self
            .0
            .lock()
            .await
            .remove(code)
            .filter(|(_, expires_at)| *expires_at > Instant::now())
            .map(|(grant, _)| grant))
    }
}

pub struct TestTokens;

impl AccessTokenIssuer for TestTokens {
    async fn issue(&self, _grant: &PendingGrant) -> Result<IssuedAccessToken, AppError> {
        Ok(IssuedAccessToken {
            token: arche::utils::nano_id_of(43),
            expires_in: 3600,
        })
    }
}

#[derive(Default)]
pub struct TestRefreshStore(Mutex<HashMap<String, (PendingGrant, Instant)>>);

impl RefreshTokenStore for TestRefreshStore {
    async fn put(&self, token: String, grant: PendingGrant, ttl: Duration) -> Result<(), AppError> {
        let expires_at = Instant::now()
            .checked_add(ttl)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(31_536_000));
        self.0.lock().await.insert(token, (grant, expires_at));
        Ok(())
    }

    async fn take(&self, token: &str) -> Result<Option<PendingGrant>, AppError> {
        Ok(self
            .0
            .lock()
            .await
            .remove(token)
            .filter(|(_, expires_at)| *expires_at > Instant::now())
            .map(|(grant, _)| grant))
    }
}
