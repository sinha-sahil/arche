#![cfg(feature = "architecture")]

use std::path::{Path, PathBuf};

use arche::architecture::{Allowlist, ArchError, Role, Workspace, checks};

const ALLOWED: &[&str] = &["pub mod api", "pub mod adapters", "pub use api::build"];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/architecture")
        .join(name)
}

fn list_ids(found: &[arche::architecture::Finding]) -> Vec<&str> {
    found.iter().map(|finding| finding.id.as_str()).collect()
}

#[test]
fn should_read_every_crate_with_its_role_when_loading_a_workspace() -> Result<(), ArchError> {
    let ws = Workspace::load(&fixture("wide"))?;
    let mut names: Vec<&str> = ws
        .units(Role::Service)
        .map(|unit| unit.name.as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["returns", "shop"]);
    Ok(())
}

#[test]
fn should_fail_when_a_crate_declares_no_role() {
    assert!(matches!(
        Workspace::load(&fixture("no_role")),
        Err(ArchError::NoRole(name)) if name == "shop"
    ));
}

#[test]
fn should_flag_a_widened_lib_rs_when_finding_extra_exports() -> Result<(), ArchError> {
    let ws = Workspace::load(&fixture("wide"))?;
    let found = checks::find_extra_exports(&ws, ALLOWED)?;
    assert_eq!(list_ids(&found), ["returns: pub mod storage"]);
    assert_eq!(found[0].at, "crates/returns/src/lib.rs");
    Ok(())
}

#[test]
fn should_flag_a_crate_when_it_skips_the_workspace_lints() -> Result<(), ArchError> {
    let ws = Workspace::load(&fixture("wide"))?;
    let found = checks::find_crates_without_workspace_lints(&ws)?;
    assert_eq!(
        list_ids(&found),
        ["returns: missing [lints] workspace = true"]
    );
    Ok(())
}

#[test]
fn should_pass_when_every_finding_is_listed_in_the_allowlist() -> Result<(), ArchError> {
    let ws = Workspace::load(&fixture("wide"))?;
    let found = checks::find_extra_exports(&ws, ALLOWED)?;
    Allowlist::load(&ws, "exports")?.assert_clean(
        "only api, adapters and build may be public in lib.rs",
        &found,
    );
    Ok(())
}

#[test]
#[should_panic(expected = "exports: these are fixed")]
fn should_fail_when_a_listed_violation_is_fixed() {
    let ws = Workspace::load(&fixture("wide")).expect("fixture loads");
    Allowlist::load(&ws, "exports")
        .expect("allowlist loads")
        .assert_clean("only api, adapters and build may be public in lib.rs", &[]);
}

#[test]
fn should_flag_every_edge_inside_the_knot_when_modules_call_each_other() -> Result<(), ArchError> {
    let ws = Workspace::load(&fixture("knot"))?;
    let found = checks::find_module_cycle_edges(&ws, "crates/app/src/services", "crate::services")?;
    assert_eq!(
        list_ids(&found),
        [
            "auth -> plugin",
            "cart -> auth",
            "cart -> plugin",
            "plugin -> cart"
        ]
    );
    assert_eq!(found[0].at, "crates/app/src/services/auth.rs");
    assert_eq!(found[2].at, "crates/app/src/services/cart/remote.rs");
    Ok(())
}

#[test]
fn should_pass_when_the_knot_has_no_new_edge() -> Result<(), ArchError> {
    let ws = Workspace::load(&fixture("knot"))?;
    let found = checks::find_module_cycle_edges(&ws, "crates/app/src/services", "crate::services")?;
    Allowlist::load(&ws, "cycles")?.assert_clean("no new edge inside the service cycle", &found);
    Ok(())
}
