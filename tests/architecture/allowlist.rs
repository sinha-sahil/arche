use super::{list_fixed, list_new, parse_entries};
use crate::architecture::Finding;

#[test]
fn should_drop_comments_and_blank_lines_when_parsing_entries() {
    let entries = parse_entries(
        "shop: pub mod storage   # owner: shop-team remove-by: 2027-01-31\n\n# a note\n",
    );
    assert_eq!(
        entries.into_iter().collect::<Vec<_>>(),
        ["shop: pub mod storage"]
    );
}

#[test]
fn should_report_a_finding_when_it_is_not_listed() {
    let found = [Finding::new(
        "shop: pub mod storage",
        "crates/shop/src/lib.rs",
    )];
    assert_eq!(
        list_new(&parse_entries(""), &found),
        ["shop: pub mod storage  (crates/shop/src/lib.rs)"]
    );
}

#[test]
fn should_accept_a_finding_when_it_is_listed() {
    let found = [Finding::new(
        "shop: pub mod storage",
        "crates/shop/src/lib.rs",
    )];
    assert!(list_new(&parse_entries("shop: pub mod storage\n"), &found).is_empty());
}

#[test]
fn should_report_an_entry_when_its_violation_is_fixed() {
    assert_eq!(
        list_fixed(&parse_entries("shop: pub mod storage\n"), &[]),
        ["shop: pub mod storage"]
    );
}
