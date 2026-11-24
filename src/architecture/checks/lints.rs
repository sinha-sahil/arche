use std::fs;

use crate::architecture::{ArchError, Finding, Role, Workspace, to_source_error};

const ROLES: [Role; 4] = [Role::Service, Role::Core, Role::Fakes, Role::App];

pub(crate) fn inherits_workspace_lints(manifest: &str) -> bool {
    manifest
        .parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("lints")?.get("workspace")?.as_bool())
        .unwrap_or(false)
}

pub fn find_crates_without_workspace_lints(ws: &Workspace) -> Result<Vec<Finding>, ArchError> {
    ROLES
        .into_iter()
        .flat_map(|role| ws.units(role))
        .map(|unit| {
            let text = fs::read_to_string(&unit.manifest)
                .map_err(|e| to_source_error(&unit.manifest, e))?;
            Ok((!inherits_workspace_lints(&text)).then(|| {
                Finding::new(
                    format!("{}: missing [lints] workspace = true", unit.name),
                    ws.to_relative(&unit.manifest),
                )
            }))
        })
        .collect::<Result<Vec<_>, ArchError>>()
        .map(|found| found.into_iter().flatten().collect())
}

#[cfg(test)]
#[path = "../../../tests/architecture/checks/lints.rs"]
mod tests;
