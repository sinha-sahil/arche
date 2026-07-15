use axum::{
    body::to_bytes,
    extract::Request,
    http::{StatusCode, header::CONTENT_TYPE},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::error::AppError;

const REJECTION_BODY_LIMIT: usize = 65_536;

pub async fn extractor_rejection(request: Request, next: Next) -> Response {
    let response = next.run(request).await;

    let rejection_status = matches!(
        response.status(),
        StatusCode::BAD_REQUEST
            | StatusCode::PAYLOAD_TOO_LARGE
            | StatusCode::UNSUPPORTED_MEDIA_TYPE
            | StatusCode::UNPROCESSABLE_ENTITY
    );
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false);

    if !rejection_status || is_json {
        return response;
    }

    let status = response.status();
    match to_bytes(response.into_body(), REJECTION_BODY_LIMIT).await {
        Ok(bytes) => AppError::bad_request(
            None,
            Some(String::from_utf8_lossy(&bytes).into_owned()),
            None,
        )
        .into_response(),
        Err(_) => status.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, body::Body, routing::post};
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Deserialize)]
    struct TestReq {
        name: String,
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/echo",
                post(|Json(req): Json<TestReq>| async move { req.name }),
            )
            .route(
                "/fail",
                post(|| async {
                    AppError::bad_request(None, Some("handler error".to_string()), None)
                }),
            )
            .layer(axum::middleware::from_fn(extractor_rejection))
    }

    fn request(uri: &str, content_type: Option<&str>, body: &str) -> Request {
        let mut builder = Request::builder().method("POST").uri(uri);
        if let Some(ct) = content_type {
            builder = builder.header(CONTENT_TYPE, ct);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    async fn read(response: Response) -> (StatusCode, String, bool) {
        let status = response.status();
        let is_json = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("application/json"))
            .unwrap_or(false);
        let bytes = to_bytes(response.into_body(), REJECTION_BODY_LIMIT)
            .await
            .unwrap();
        (
            status,
            String::from_utf8_lossy(&bytes).into_owned(),
            is_json,
        )
    }

    #[tokio::test]
    async fn rewrites_missing_field_rejection_as_app_error_json() {
        let response = app()
            .oneshot(request("/echo", Some("application/json"), "{}"))
            .await
            .unwrap();
        let (status, body, is_json) = read(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(is_json);
        assert!(body.contains("error_values"));
        assert!(body.contains("missing field `name`"));
    }

    #[tokio::test]
    async fn rewrites_syntax_error_rejection() {
        let response = app()
            .oneshot(request("/echo", Some("application/json"), "not json"))
            .await
            .unwrap();
        let (status, body, is_json) = read(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(is_json);
        assert!(body.contains("error_values"));
    }

    #[tokio::test]
    async fn rewrites_missing_content_type_rejection() {
        let response = app()
            .oneshot(request("/echo", None, "{\"name\":\"a\"}"))
            .await
            .unwrap();
        let (status, body, is_json) = read(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(is_json);
        assert!(body.contains("Content-Type"));
    }

    #[tokio::test]
    async fn passes_through_handler_app_errors_untouched() {
        let response = app()
            .oneshot(request("/fail", Some("application/json"), "{}"))
            .await
            .unwrap();
        let (status, body, is_json) = read(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(is_json);
        assert!(body.contains("handler error"));
    }

    #[tokio::test]
    async fn passes_through_success_untouched() {
        let response = app()
            .oneshot(request(
                "/echo",
                Some("application/json"),
                "{\"name\":\"a\"}",
            ))
            .await
            .unwrap();
        let (status, body, _) = read(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "a");
    }
}
