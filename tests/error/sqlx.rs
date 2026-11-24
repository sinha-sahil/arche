use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

use axum::response::IntoResponse;
use sqlx::error::{DatabaseError, ErrorKind};

use super::AppError;

#[derive(Debug)]
struct PgError(&'static str);

impl fmt::Display for PgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pg error {}", self.0)
    }
}

impl StdError for PgError {}

impl DatabaseError for PgError {
    fn message(&self) -> &str {
        "pg error"
    }

    fn code(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self.0))
    }

    fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

fn status(e: sqlx::Error) -> u16 {
    AppError::from_sqlx(e, "shop")
        .into_response()
        .status()
        .as_u16()
}

fn pg(code: &'static str) -> sqlx::Error {
    sqlx::Error::Database(Box::new(PgError(code)))
}

#[test]
fn should_answer_404_when_no_row_is_found() {
    assert_eq!(status(sqlx::Error::RowNotFound), 404);
}

#[test]
fn should_answer_409_when_a_unique_index_is_violated() {
    assert_eq!(status(pg("23505")), 409);
}

#[test]
fn should_answer_422_when_a_foreign_key_null_or_check_constraint_fails() {
    assert_eq!(status(pg("23503")), 422);
    assert_eq!(status(pg("23502")), 422);
    assert_eq!(status(pg("23514")), 422);
}

#[test]
fn should_answer_503_when_the_pool_is_unavailable() {
    assert_eq!(status(sqlx::Error::PoolTimedOut), 503);
    assert_eq!(status(sqlx::Error::PoolClosed), 503);
}

#[test]
fn should_answer_500_when_the_database_error_is_unknown() {
    assert_eq!(status(pg("40001")), 500);
    assert_eq!(status(sqlx::Error::WorkerCrashed), 500);
}
