use std::collections::{BTreeMap, BTreeSet};

use super::{list_cycle_edges, list_references};

const PREFIX: &str = "crate::services";

fn references(source: &str) -> Vec<String> {
    list_references(&syn::parse_str(source).expect("fixture parses"), PREFIX)
        .into_iter()
        .collect()
}

fn graph(edges: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
    edges.iter().fold(BTreeMap::new(), |mut graph, (from, to)| {
        graph
            .entry(from.to_string())
            .or_insert_with(BTreeSet::new)
            .insert(to.to_string());
        graph
    })
}

fn cycle_edges(edges: &[(&str, &str)]) -> Vec<(String, String)> {
    list_cycle_edges(&graph(edges))
}

fn pairs(edges: &[(&str, &str)]) -> Vec<(String, String)> {
    edges
        .iter()
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .collect()
}

#[test]
fn should_read_the_module_when_a_use_names_it() {
    assert_eq!(
        references("use crate::services::plugin::helpers::normalize_shop_url;"),
        ["plugin"]
    );
}

#[test]
fn should_read_every_module_when_a_use_groups_them() {
    assert_eq!(
        references("use crate::services::{cart::helpers, plugin::storage as plugin_storage};"),
        ["cart", "plugin"]
    );
}

#[test]
fn should_read_the_module_when_a_body_calls_it_by_full_path() {
    assert_eq!(
        references("fn f() { crate::services::cart::helpers::provision(); }"),
        ["cart"]
    );
}

#[test]
fn should_read_the_module_when_a_use_renames_it() {
    assert_eq!(
        references("use crate::services::agent as agent_service;"),
        ["agent"]
    );
}

#[test]
fn should_read_the_module_when_a_macro_body_names_it() {
    assert_eq!(
        references(
            "fn f() { let v = json!({ \"a\": crate::services::cart::helpers::provision() }); }"
        ),
        ["cart"]
    );
}

#[test]
fn should_ignore_a_longer_name_that_ends_with_the_prefix_in_a_macro() {
    assert!(references("fn f() { log!(mycrate::services::cart::x); }").is_empty());
}

#[test]
fn should_ignore_paths_outside_the_prefix() {
    assert!(references("use crate::helper::aes_decrypt; use super::types::Shop;").is_empty());
}

#[test]
fn should_flag_both_edges_when_two_modules_call_each_other() {
    assert_eq!(
        cycle_edges(&[("auth", "plugin"), ("plugin", "auth")]),
        pairs(&[("auth", "plugin"), ("plugin", "auth")])
    );
}

#[test]
fn should_flag_nothing_when_calls_only_go_down() {
    assert!(cycle_edges(&[("shop", "auth"), ("returns", "shop"), ("returns", "auth")]).is_empty());
}

#[test]
fn should_flag_only_the_loop_when_a_caller_sits_outside_it() {
    assert_eq!(
        cycle_edges(&[
            ("plugin", "cart"),
            ("cart", "storefront"),
            ("storefront", "plugin"),
            ("returns", "plugin"),
        ]),
        pairs(&[
            ("cart", "storefront"),
            ("plugin", "cart"),
            ("storefront", "plugin"),
        ])
    );
}
