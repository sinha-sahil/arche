use std::future::Future;
use std::sync::Arc;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;

use crate::error::AppError;

pub enum Verdict {
    Allow(Request),
    Deny(AppError),
    Absent(Request),
}

pub(crate) type Check = Arc<dyn Fn(Request) -> BoxFuture<'static, Verdict> + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    One,
    Any,
    All,
}

#[derive(Clone)]
pub struct Guard {
    name: String,
    shape: Shape,
    check: Check,
}

impl Guard {
    pub fn new<F, Fut>(name: impl Into<String>, check: F) -> Guard
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Verdict> + Send + 'static,
    {
        Guard {
            name: name.into(),
            shape: Shape::One,
            check: Arc::new(move |req| Box::pin(check(req))),
        }
    }

    pub fn or(self, other: Guard) -> Guard {
        let (first, second) = (self.check, other.check);
        Guard {
            name: format!("{} | {}", self.name, other.name),
            shape: Shape::Any,
            check: Arc::new(move |req| {
                let (first, second) = (Arc::clone(&first), Arc::clone(&second));
                Box::pin(async move {
                    match first(req).await {
                        Verdict::Absent(req) => second(req).await,
                        verdict => verdict,
                    }
                })
            }),
        }
    }

    pub fn and(self, other: Guard) -> Guard {
        let name = format!("{} & {}", self.to_operand(), other.to_operand());
        let (first, second) = (self.check, other.check);
        Guard {
            name,
            shape: Shape::All,
            check: Arc::new(move |req| {
                let (first, second) = (Arc::clone(&first), Arc::clone(&second));
                Box::pin(async move {
                    match first(req).await {
                        Verdict::Allow(req) => require(second(req).await),
                        verdict => verdict,
                    }
                })
            }),
        }
    }

    pub(crate) fn into_parts(self) -> (String, Check) {
        (self.name, self.check)
    }

    fn to_operand(&self) -> String {
        match self.shape {
            Shape::Any => format!("({})", self.name),
            Shape::One | Shape::All => self.name.clone(),
        }
    }
}

fn require(verdict: Verdict) -> Verdict {
    match verdict {
        Verdict::Absent(_) => Verdict::Deny(AppError::Unauthorized),
        verdict => verdict,
    }
}

pub(crate) async fn admit(check: Check, req: Request, next: Next) -> Response {
    match check(req).await {
        Verdict::Allow(req) => next.run(req).await,
        Verdict::Deny(error) => error.into_response(),
        Verdict::Absent(_) => AppError::Unauthorized.into_response(),
    }
}
