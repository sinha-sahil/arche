mod guard;
mod routes;
mod shutdown;
mod transport;

use std::sync::Arc;

pub use guard::{Guard, Verdict};
pub use routes::{Access, Route, Routes};
pub use shutdown::{Shutdown, Trigger, spawn};
pub use transport::Transport;

pub trait Service: Send + Sync {
    fn name(&self) -> &'static str;
    fn build_router(self: Arc<Self>) -> Routes;
    fn spawn_workers(self: Arc<Self>, _shutdown: Shutdown) {}
}
