use std::sync::Arc;

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, X_CONTENT_TYPE_OPTIONS};
use axum::http::{Request, StatusCode};
use axum::middleware::{Next, from_fn};
use axum::routing::{get, post};
use axum::{Extension, Router};
use tower::ServiceExt;

use crate::error::AppError;
use crate::service::{Access, Guard, Route, Routes, Service, Transport, Verdict};

struct DeviceService {
    info: &'static str,
}

async fn info(State(svc): State<Arc<DeviceService>>) -> &'static str {
    svc.info
}

impl Service for DeviceService {
    fn name(&self) -> &'static str {
        "device"
    }

    fn build_router(self: Arc<Self>) -> Routes {
        Routes::new()
            .public(
                "/device/info",
                get(info),
                "device network info, no caller data",
            )
            .with_state(self)
    }
}

fn build() -> Routes {
    Arc::new(DeviceService { info: "ok" }).build_router()
}

#[tokio::test]
async fn should_serve_the_handler_with_the_service_state_when_merged() {
    let response = build()
        .into_router()
        .oneshot(
            Request::get("/device/info")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn should_answer_404_when_the_path_is_not_registered() {
    let response = build()
        .into_router()
        .oneshot(
            Request::get("/device/other")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn should_record_the_reason_when_a_route_is_public() {
    assert_eq!(
        build().table(),
        [Route {
            path: "/device/info",
            access: Access::Public {
                reason: "device network info, no caller data"
            },
            layers: vec![],
        }]
    );
}

#[test]
fn should_be_usable_as_a_trait_object_when_main_holds_every_service() {
    let services: Vec<Arc<dyn Service>> = vec![Arc::new(DeviceService { info: "ok" })];
    assert_eq!(services[0].name(), "device");
    assert_eq!(services[0].clone().build_router().table().len(), 1);
}

#[derive(Clone)]
struct Caller(&'static str);

fn credential_guard(name: &'static str, header: &'static str, secret: &'static str) -> Guard {
    Guard::new(name, move |mut req: Request<Body>| async move {
        let Some(value) = req.headers().get(header) else {
            return Verdict::Absent(req);
        };
        if value != secret {
            return Verdict::Deny(AppError::Forbidden);
        }
        req.extensions_mut().insert(Caller(name));
        Verdict::Allow(req)
    })
}

fn api_key() -> Guard {
    credential_guard("api_key", "x-api-key", "key")
}

fn shopify_session() -> Guard {
    credential_guard("shopify_session", "x-shopify-token", "token")
}

fn super_user() -> Guard {
    credential_guard("super_user", "x-super-user", "root")
}

async fn whoami(Extension(caller): Extension<Caller>) -> &'static str {
    caller.0
}

fn guarded(guard: Guard) -> Router {
    Routes::<()>::new()
        .guarded("/shop/list", get(whoami), guard)
        .public(
            "/shop/ping",
            get(|| async { "pong" }),
            "liveness, no caller data",
        )
        .with_state(())
        .into_router()
}

async fn call(router: Router, headers: &[(&str, &str)]) -> (StatusCode, String) {
    let request = headers
        .iter()
        .fold(Request::get("/shop/list"), |request, (name, value)| {
            request.header(*name, *value)
        })
        .body(Body::empty())
        .expect("request builds");
    let response = router.oneshot(request).await.expect("router answers");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn should_run_the_handler_with_the_caller_when_the_guard_allows() {
    assert_eq!(
        call(guarded(api_key()), &[("x-api-key", "key")]).await,
        (StatusCode::OK, "api_key".to_string())
    );
}

#[tokio::test]
async fn should_answer_401_when_the_credential_is_absent() {
    assert_eq!(
        call(guarded(api_key()), &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn should_answer_the_guards_error_when_it_denies() {
    assert_eq!(
        call(guarded(api_key()), &[("x-api-key", "wrong")]).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn should_leave_public_routes_open_when_a_sibling_route_is_guarded() {
    let response = guarded(api_key())
        .oneshot(
            Request::get("/shop/ping")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn should_try_the_second_guard_when_the_first_credential_is_absent() {
    assert_eq!(
        call(
            guarded(shopify_session().or(api_key())),
            &[("x-api-key", "key")]
        )
        .await,
        (StatusCode::OK, "api_key".to_string())
    );
}

#[tokio::test]
async fn should_not_fall_back_when_the_first_credential_is_present_but_wrong() {
    assert_eq!(
        call(
            guarded(shopify_session().or(api_key())),
            &[("x-shopify-token", "forged"), ("x-api-key", "key")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn should_require_both_credentials_when_guards_are_joined_with_and() {
    let both = || super_user().and(api_key());
    assert_eq!(
        call(guarded(both()), &[("x-super-user", "root")]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            guarded(both()),
            &[("x-super-user", "root"), ("x-api-key", "key")]
        )
        .await,
        (StatusCode::OK, "api_key".to_string())
    );
}

#[tokio::test]
async fn should_try_the_other_branch_when_a_joined_branch_has_no_credential() {
    assert_eq!(
        call(
            guarded(super_user().and(shopify_session()).or(api_key())),
            &[("x-api-key", "key")]
        )
        .await,
        (StatusCode::OK, "api_key".to_string())
    );
}

#[tokio::test]
async fn should_deny_when_a_joined_branch_has_only_part_of_its_credentials() {
    assert_eq!(
        call(
            guarded(super_user().and(shopify_session()).or(api_key())),
            &[("x-super-user", "root"), ("x-api-key", "key")]
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn should_record_the_guard_name_when_a_route_is_guarded() {
    let routes = Routes::<()>::new()
        .guarded("/a", get(|| async {}), shopify_session().or(api_key()))
        .guarded(
            "/b",
            get(|| async {}),
            shopify_session()
                .or(api_key())
                .and(super_user())
                .and(api_key()),
        )
        .guarded(
            "/c",
            get(|| async {}),
            super_user().and(shopify_session()).or(api_key()),
        );
    assert_eq!(
        routes.table(),
        [
            Route {
                path: "/a",
                access: Access::Guarded {
                    guard: "shopify_session | api_key".to_string()
                },
                layers: vec![],
            },
            Route {
                path: "/b",
                access: Access::Guarded {
                    guard: "(shopify_session | api_key) & super_user & api_key".to_string()
                },
                layers: vec![],
            },
            Route {
                path: "/c",
                access: Access::Guarded {
                    guard: "super_user & shopify_session | api_key".to_string()
                },
                layers: vec![],
            },
        ]
    );
}

async fn echo(body: Bytes) -> String {
    body.len().to_string()
}

async fn teapot(_req: Request<Body>, _next: Next) -> StatusCode {
    StatusCode::IM_A_TEAPOT
}

async fn send(router: Router, path: &str, headers: &[(&str, &str)], body: usize) -> StatusCode {
    let request = headers
        .iter()
        .fold(Request::post(path), |request, (name, value)| {
            request.header(*name, *value)
        })
        .body(Body::from(vec![b'x'; body]))
        .expect("request builds");
    router
        .oneshot(request)
        .await
        .expect("router answers")
        .status()
}

#[tokio::test]
async fn should_answer_413_when_the_body_is_over_the_route_limit() {
    let router = || {
        Routes::<()>::new()
            .public("/upload", post(echo), "test upload")
            .with(Transport::body_limit(8))
            .into_router()
    };
    assert_eq!(send(router(), "/upload", &[], 8).await, StatusCode::OK);
    assert_eq!(
        send(router(), "/upload", &[], 9).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn should_let_the_route_limit_win_when_the_service_sets_a_smaller_one() {
    let router = Routes::<()>::new()
        .public("/upload", post(echo), "test upload")
        .with(Transport::body_limit(64))
        .with_all(Transport::body_limit(8))
        .into_router();
    assert_eq!(send(router, "/upload", &[], 32).await, StatusCode::OK);
}

#[tokio::test]
async fn should_run_the_guard_before_the_route_layers() {
    let router = || {
        Routes::<()>::new()
            .guarded("/brew", post(echo), api_key())
            .with(Transport::custom("teapot", from_fn(teapot)))
            .into_router()
    };
    assert_eq!(
        send(router(), "/brew", &[], 0).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(router(), "/brew", &[("x-api-key", "key")], 0).await,
        StatusCode::IM_A_TEAPOT
    );
}

#[tokio::test]
async fn should_set_the_header_presets_only_on_the_route_they_are_added_to() {
    let router = || {
        Routes::<()>::new()
            .public("/secret", get(|| async { "s" }), "test secret")
            .with(Transport::no_store())
            .with(Transport::nosniff())
            .public("/plain", get(|| async { "p" }), "test plain")
            .into_router()
    };
    let secret = router()
        .oneshot(
            Request::get("/secret")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(secret.headers()[CACHE_CONTROL], "no-store");
    assert_eq!(secret.headers()[X_CONTENT_TYPE_OPTIONS], "nosniff");
    let plain = router()
        .oneshot(
            Request::get("/plain")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert!(plain.headers().get(CACHE_CONTROL).is_none());
}

#[tokio::test]
async fn should_keep_the_handlers_cache_control_when_no_store_is_added() {
    let response = Routes::<()>::new()
        .public(
            "/cached",
            get(|| async { ([(CACHE_CONTROL, "max-age=60")], "c") }),
            "test cached",
        )
        .with(Transport::no_store())
        .into_router()
        .oneshot(
            Request::get("/cached")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.headers()[CACHE_CONTROL], "max-age=60");
}

#[test]
fn should_record_the_route_layers_then_the_service_layers_in_the_table() {
    let routes = Routes::<()>::new()
        .public("/a", get(|| async {}), "test a")
        .with(Transport::body_limit(1024))
        .guarded("/b", get(|| async {}), api_key())
        .with_all(Transport::nosniff())
        .with_all(Transport::custom("latency", from_fn(teapot)));
    assert_eq!(
        routes.table(),
        [
            Route {
                path: "/a",
                access: Access::Public { reason: "test a" },
                layers: vec![
                    "body_limit(1024)".to_string(),
                    "nosniff".to_string(),
                    "custom(latency)".to_string(),
                ],
            },
            Route {
                path: "/b",
                access: Access::Guarded {
                    guard: "api_key".to_string()
                },
                layers: vec!["nosniff".to_string(), "custom(latency)".to_string()],
            },
        ]
    );
}

#[tokio::test]
async fn should_serve_two_methods_on_one_path_with_their_own_guards() {
    let router = || {
        Routes::<()>::new()
            .public("/item", get(|| async { "read" }), "test read")
            .guarded("/item", post(echo), api_key())
            .into_router()
    };
    let read = router()
        .oneshot(
            Request::get("/item")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(
        send(router(), "/item", &[], 0).await,
        StatusCode::UNAUTHORIZED
    );
}
