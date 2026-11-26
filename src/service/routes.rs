use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::middleware::{Next, from_fn};
use axum::routing::MethodRouter;

use super::guard::{Check, Guard, admit};
use super::transport::Transport;

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Access {
    Public { reason: &'static str },
    Guarded { guard: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Route {
    pub path: &'static str,
    pub access: Access,
    pub layers: Vec<String>,
}

struct Entry<S> {
    path: &'static str,
    method: MethodRouter<S>,
    access: Access,
    check: Option<Check>,
    layers: Vec<Transport>,
}

pub struct Routes<S = ()> {
    entries: Vec<Entry<S>>,
    shared: Vec<Transport>,
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
            entries: Vec::new(),
            shared: Vec::new(),
        }
    }

    pub fn public(
        mut self,
        path: &'static str,
        method: MethodRouter<S>,
        reason: &'static str,
    ) -> Self {
        self.entries.push(Entry {
            path,
            method,
            access: Access::Public { reason },
            check: None,
            layers: Vec::new(),
        });
        self
    }

    pub fn guarded(mut self, path: &'static str, method: MethodRouter<S>, guard: Guard) -> Self {
        let (name, check) = guard.into_parts();
        self.entries.push(Entry {
            path,
            method,
            access: Access::Guarded { guard: name },
            check: Some(check),
            layers: Vec::new(),
        });
        self
    }

    pub fn with(mut self, transport: Transport) -> Self {
        if let Some(entry) = self.entries.last_mut() {
            entry.layers.push(transport);
        }
        self
    }

    pub fn with_all(mut self, transport: Transport) -> Self {
        self.shared.push(transport);
        self
    }

    pub fn with_state(self, state: S) -> Routes {
        Routes {
            entries: self
                .entries
                .into_iter()
                .map(|entry| Entry {
                    path: entry.path,
                    method: entry.method.with_state(state.clone()),
                    access: entry.access,
                    check: entry.check,
                    layers: entry.layers,
                })
                .collect(),
            shared: self.shared,
        }
    }

    pub fn table(&self) -> Vec<Route> {
        self.entries
            .iter()
            .map(|entry| Route {
                path: entry.path,
                access: entry.access.clone(),
                layers: entry
                    .layers
                    .iter()
                    .chain(&self.shared)
                    .map(|transport| transport.name().to_string())
                    .collect(),
            })
            .collect()
    }
}

impl Routes {
    pub fn into_router(self) -> Router {
        let shared = self.shared;
        self.entries
            .into_iter()
            .fold(Router::new(), |router, entry| {
                let path = entry.path;
                router.route(path, build_method(entry, &shared))
            })
    }
}

fn build_method(entry: Entry<()>, shared: &[Transport]) -> MethodRouter {
    let method = entry
        .layers
        .iter()
        .chain(shared)
        .fold(entry.method, |method, transport| {
            method.layer(transport.to_layer())
        });
    match entry.check {
        Some(check) => method.route_layer(from_fn(move |req: Request, next: Next| {
            admit(Arc::clone(&check), req, next)
        })),
        None => method,
    }
}

#[cfg(test)]
#[path = "../../tests/service/routes.rs"]
mod tests;
