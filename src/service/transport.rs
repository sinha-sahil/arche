use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, X_CONTENT_TYPE_OPTIONS};
use axum::response::{IntoResponse, Response};
use axum::routing::Route;
use tower::util::BoxCloneSyncService;
use tower::{Layer, Service, ServiceExt};
use tower_http::set_header::SetResponseHeaderLayer;

type Wrapped = BoxCloneSyncService<Request, Response, Infallible>;

type Wrap = Arc<dyn Fn(Route) -> Wrapped + Send + Sync>;

#[derive(Clone)]
pub struct Transport {
    name: String,
    wrap: Wrap,
}

impl Transport {
    pub fn body_limit(bytes: usize) -> Transport {
        Transport::from_layer(format!("body_limit({bytes})"), DefaultBodyLimit::max(bytes))
    }

    pub fn no_store() -> Transport {
        Transport::from_layer(
            "no_store".to_string(),
            SetResponseHeaderLayer::if_not_present(
                CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            ),
        )
    }

    pub fn nosniff() -> Transport {
        Transport::from_layer(
            "nosniff".to_string(),
            SetResponseHeaderLayer::if_not_present(
                X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        )
    }

    pub fn custom<L>(name: &str, layer: L) -> Transport
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        Transport::from_layer(format!("custom({name})"), layer)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn to_layer(&self) -> Applied {
        Applied(Arc::clone(&self.wrap))
    }

    fn from_layer<L>(name: String, layer: L) -> Transport
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        Transport {
            name,
            wrap: Arc::new(move |route| {
                BoxCloneSyncService::new(
                    layer
                        .layer(route)
                        .map_response(IntoResponse::into_response)
                        .map_err(Into::into),
                )
            }),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Applied(Wrap);

impl Layer<Route> for Applied {
    type Service = Wrapped;

    fn layer(&self, route: Route) -> Wrapped {
        (self.0)(route)
    }
}
