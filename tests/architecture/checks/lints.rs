use super::inherits_workspace_lints;

#[test]
fn should_pass_when_the_manifest_inherits_workspace_lints() {
    assert!(inherits_workspace_lints(
        "[package]\nname = \"shop\"\n[lints]\nworkspace = true\n"
    ));
}

#[test]
fn should_flag_when_the_manifest_has_no_lints_table() {
    assert!(!inherits_workspace_lints("[package]\nname = \"shop\"\n"));
}

#[test]
fn should_flag_when_the_manifest_sets_its_own_lints() {
    assert!(!inherits_workspace_lints(
        "[package]\nname = \"shop\"\n[lints.rust]\nunsafe_code = \"forbid\"\n"
    ));
}

#[test]
fn should_flag_when_the_manifest_does_not_parse() {
    assert!(!inherits_workspace_lints("[package"));
}
