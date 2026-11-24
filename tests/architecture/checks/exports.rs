use super::list_extra_exports;

const ALLOWED: &[&str] = &["pub mod api", "pub mod adapters", "pub use api::build"];

fn lib(source: &str) -> syn::File {
    syn::parse_str(source).expect("fixture parses")
}

#[test]
fn should_pass_when_lib_rs_exposes_the_three_names() {
    assert!(
        list_extra_exports(
            &lib("pub mod api; pub mod adapters; pub use api::build; mod storage;"),
            ALLOWED
        )
        .is_empty()
    );
}

#[test]
fn should_flag_a_module_when_lib_rs_widens() {
    assert_eq!(
        list_extra_exports(&lib("pub mod api; pub mod storage;"), ALLOWED),
        ["pub mod storage"]
    );
}

#[test]
fn should_flag_a_re_export_when_it_bypasses_api() {
    assert_eq!(
        list_extra_exports(
            &lib("pub use api::{build, ShopApi}; pub use storage::get_shop;"),
            ALLOWED
        ),
        ["pub use api::ShopApi", "pub use storage::get_shop"]
    );
}

#[test]
fn should_flag_a_rename_when_build_is_re_exported_under_another_name() {
    assert_eq!(
        list_extra_exports(&lib("pub use api::build as make;"), ALLOWED),
        ["pub use api::build as make"]
    );
}

#[test]
fn should_flag_an_inline_module_when_api_has_a_body() {
    assert_eq!(
        list_extra_exports(&lib("pub mod api { pub fn x() {} }"), ALLOWED),
        ["pub mod api (inline)"]
    );
}

#[test]
fn should_flag_a_macro_call_when_it_sits_at_the_crate_root() {
    assert_eq!(
        list_extra_exports(&lib("include!(\"generated.rs\");"), ALLOWED),
        ["macro call include"]
    );
}

#[test]
fn should_flag_an_exported_macro_when_lib_rs_defines_one() {
    assert_eq!(
        list_extra_exports(
            &lib("#[macro_export] macro_rules! shop_key { () => {} }"),
            ALLOWED
        ),
        ["pub macro shop_key"]
    );
}

#[test]
fn should_flag_public_items_when_lib_rs_defines_them() {
    assert_eq!(
        list_extra_exports(
            &lib(
                "pub fn f() {} pub struct S; pub enum E {} pub trait T {} pub type A = u8; pub const C: u8 = 0; pub static X: u8 = 0;"
            ),
            ALLOWED
        ),
        [
            "pub fn f",
            "pub struct S",
            "pub enum E",
            "pub trait T",
            "pub type A",
            "pub const C",
            "pub static X"
        ]
    );
}

#[test]
fn should_ignore_crate_visibility_when_items_stay_inside() {
    assert!(
        list_extra_exports(
            &lib("pub(crate) mod storage; pub(crate) fn helper() {}"),
            ALLOWED
        )
        .is_empty()
    );
}
