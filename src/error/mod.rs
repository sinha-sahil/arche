use axum::Json;
use axum::http::StatusCode;
use axum::http::header::WWW_AUTHENTICATE;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    // --- 4xx ---
    #[error("Bad Request")]
    BadRequest {
        error_values: Option<HashMap<String, String>>,
        message: Option<String>,
        description: Option<String>,
    },

    #[error("Authentication Required")]
    Unauthorized,

    #[error("Access Denied")]
    Forbidden,

    #[error("Not Found")]
    NotFound { resource: String },

    #[error("Conflict")]
    Conflict { message: String },

    #[error("Unprocessable Entity")]
    UnprocessableEntity {
        error_values: Option<HashMap<String, String>>,
        message: Option<String>,
        description: Option<String>,
    },

    #[error("Failed Dependency")]
    DependencyFailed {
        upstream: String,
        detail: String,
        retryable: bool,
    },

    // --- 5xx ---
    #[error("Internal Error")]
    InternalError {
        error: String,
        message: Option<String>,
    },

    #[error("Service Unavailable")]
    #[allow(dead_code)]
    Unavailable,
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::UnprocessableEntity { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::DependencyFailed { .. } => StatusCode::from_u16(424).unwrap(),
            Self::InternalError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub fn bad_request(
        errors: Option<HashMap<String, String>>,
        message: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self::BadRequest {
            error_values: errors,
            message,
            description,
        }
    }

    pub fn not_found(resource: impl Into<String>) -> Self {
        Self::NotFound {
            resource: resource.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }

    pub fn unprocessable_entity(
        errors: Option<HashMap<String, String>>,
        message: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self::UnprocessableEntity {
            error_values: errors,
            message,
            description,
        }
    }

    pub fn dependency_failed(upstream: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::DependencyFailed {
            upstream: upstream.into(),
            detail: detail.into(),
            retryable: true,
        }
    }

    pub fn dependency_failed_permanent(
        upstream: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::DependencyFailed {
            upstream: upstream.into(),
            detail: detail.into(),
            retryable: false,
        }
    }

    pub fn internal_error(error: String, message: Option<String>) -> Self {
        Self::InternalError { error, message }
    }

    pub fn is_dependency_error(&self) -> bool {
        matches!(self, Self::DependencyFailed { .. })
    }

    pub fn detailed_error_message(&self) -> String {
        match self {
            Self::BadRequest { message, .. }
            | Self::UnprocessableEntity { message, .. }
            | Self::InternalError { message, .. } => {
                message.clone().unwrap_or_else(|| self.to_string())
            }
            Self::Conflict { message } => message.clone(),
            Self::DependencyFailed { detail, .. } => detail.clone(),
            _ => self.to_string(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest {
                error_values,
                message,
                description,
            } => (
                StatusCode::BAD_REQUEST,
                Json(ErrorDetails {
                    error_values,
                    message,
                    description,
                }),
            )
                .into_response(),

            Self::Unauthorized => (
                self.status_code(),
                [(WWW_AUTHENTICATE, "Token")],
                self.to_string(),
            )
                .into_response(),

            Self::Forbidden => (StatusCode::FORBIDDEN, self.to_string()).into_response(),

            Self::NotFound { resource } => (
                StatusCode::NOT_FOUND,
                Json(NotFoundDetails {
                    error: "not_found".to_string(),
                    resource,
                }),
            )
                .into_response(),

            Self::Conflict { message } => (
                StatusCode::CONFLICT,
                Json(ConflictDetails {
                    error: "conflict".to_string(),
                    message,
                }),
            )
                .into_response(),

            Self::UnprocessableEntity {
                error_values,
                message,
                description,
            } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrorDetails {
                    error_values,
                    message,
                    description,
                }),
            )
                .into_response(),

            Self::DependencyFailed {
                upstream,
                detail,
                retryable,
            } => {
                tracing::warn!(
                    upstream = %upstream,
                    detail = %detail,
                    retryable,
                    "returning 424: dependency failed"
                );
                (
                    StatusCode::from_u16(424).unwrap(),
                    Json(DependencyFailedDetails {
                        error: "dependency_failed".to_string(),
                        upstream,
                        message: "An upstream dependency is currently unavailable".to_string(),
                        retryable,
                    }),
                )
                    .into_response()
            }

            Self::InternalError { error, message } => {
                tracing::error!(internal_error = %error, message = ?message, "returning 500");

                #[cfg(feature = "verbose-errors")]
                let body = InternalErrorDetails { error, message };

                #[cfg(not(feature = "verbose-errors"))]
                let body = InternalErrorDetails {
                    error: "internal_error".to_string(),
                    message: Some("An unexpected error occurred".to_string()),
                };

                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }

            Self::Unavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, self.to_string()).into_response()
            }
        }
    }
}

#[derive(serde::Serialize)]
struct ErrorDetails {
    error_values: Option<HashMap<String, String>>,
    message: Option<String>,
    description: Option<String>,
}

#[derive(serde::Serialize)]
struct NotFoundDetails {
    error: String,
    resource: String,
}

#[derive(serde::Serialize)]
struct ConflictDetails {
    error: String,
    message: String,
}

#[derive(serde::Serialize)]
struct DependencyFailedDetails {
    error: String,
    upstream: String,
    message: String,
    retryable: bool,
}

#[derive(serde::Serialize)]
struct InternalErrorDetails {
    error: String,
    message: Option<String>,
}

/// Bridge for the extracted LLM stack: `?` works on `senno::Error` inside
/// handlers returning `AppError`.
impl From<senno::Error> for AppError {
    fn from(e: senno::Error) -> Self {
        match e {
            senno::Error::Provider {
                provider,
                detail,
                retryable,
            } => Self::DependencyFailed {
                upstream: provider,
                detail,
                retryable,
            },
            senno::Error::BadResponse(detail) => Self::DependencyFailed {
                upstream: "llm".into(),
                detail,
                retryable: true,
            },
            senno::Error::Config(error) => Self::InternalError {
                error,
                message: None,
            },
            senno::Error::Tool { name, detail } => Self::InternalError {
                error: format!("tool {name}: {detail}"),
                message: None,
            },
            senno::Error::Internal(error) => Self::InternalError {
                error,
                message: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senno_provider_error_maps_to_dependency_failed_preserving_retryability() {
        let e: AppError = senno::Error::provider("vertex-ai", "boom").into();
        assert!(matches!(
            e,
            AppError::DependencyFailed { ref upstream, retryable: true, .. } if upstream == "vertex-ai"
        ));

        let e: AppError = senno::Error::provider_permanent("gcp-oauth2", "denied").into();
        assert!(matches!(
            e,
            AppError::DependencyFailed {
                retryable: false,
                ..
            }
        ));
    }

    #[test]
    fn senno_bad_response_maps_to_retryable_llm_dependency_failure() {
        let e: AppError = senno::Error::bad_response("no tool call").into();
        assert!(e.is_dependency_error());
        assert_eq!(e.detailed_error_message(), "no tool call");
    }

    #[test]
    fn senno_config_and_tool_errors_map_to_internal() {
        let e: AppError = senno::Error::config("missing key").into();
        assert!(matches!(e, AppError::InternalError { .. }));

        let e: AppError = senno::Error::tool("search", "db down").into();
        assert!(matches!(
            e,
            AppError::InternalError { ref error, .. } if error == "tool search: db down"
        ));
    }
}
