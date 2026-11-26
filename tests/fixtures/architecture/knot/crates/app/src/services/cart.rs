pub mod remote;

use crate::services::auth;

pub fn provision() -> String {
    auth::verify() + &remote::fetch()
}
