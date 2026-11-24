use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use tower::ServiceExt;

use super::{Access, Route, Routes, Service};

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
        }]
    );
}

#[test]
fn should_be_usable_as_a_trait_object_when_main_holds_every_service() {
    let services: Vec<Arc<dyn Service>> = vec![Arc::new(DeviceService { info: "ok" })];
    assert_eq!(services[0].name(), "device");
    assert_eq!(services[0].clone().build_router().table().len(), 1);
}
