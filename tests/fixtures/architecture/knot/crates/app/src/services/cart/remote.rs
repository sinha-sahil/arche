use crate::services::plugin as plugin_service;

pub fn fetch() -> String {
    plugin_service::name()
}
