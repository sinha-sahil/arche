use std::sync::Arc;

use axum::Router;
use axum::routing::MethodRouter;

pub trait Service: Send + Sync {
    fn name(&self) -> &'static str;
    fn build_router(self: Arc<Self>) -> Routes;
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Access {
    Public { reason: &'static str },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub path: &'static str,
    pub access: Access,
}

pub struct Routes<S = ()> {
    router: Router<S>,
    table: Vec<Route>,
}

impl<S> Default for Routes<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Routes<S>
where
    S: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Routes {
            router: Router::new(),
            table: Vec::new(),
        }
    }

    pub fn public(
        mut self,
        path: &'static str,
        method: MethodRouter<S>,
        reason: &'static str,
    ) -> Self {
        self.router = self.router.route(path, method);
        self.table.push(Route {
            path,
            access: Access::Public { reason },
        });
        self
    }

    pub fn with_state(self, state: S) -> Routes {
        Routes {
            router: self.router.with_state(state),
            table: self.table,
        }
    }

    pub fn table(&self) -> &[Route] {
        &self.table
    }
}

impl Routes {
    pub fn into_router(self) -> Router {
        self.router
    }
}

#[cfg(test)]
#[path = "../../tests/service/routes.rs"]
mod tests;
