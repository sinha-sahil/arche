mod allowlist;
pub mod checks;

use std::fs;
use std::path::{Path, PathBuf};

use cargo_metadata::{MetadataCommand, Package};
use syn::{UseTree, Visibility};

pub use allowlist::Allowlist;

#[derive(Debug, thiserror::Error)]
pub enum ArchError {
    #[error("cargo metadata failed: {0}")]
    Metadata(String),
    #[error("{0}: no [package.metadata.arche] role")]
    NoRole(String),
    #[error("{path}: {reason}")]
    Source { path: String, reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Service,
    Core,
    Fakes,
    App,
}

pub struct Unit {
    pub name: String,
    pub role: Role,
    pub manifest: PathBuf,
    pub root_file: PathBuf,
}

pub struct Workspace {
    root: PathBuf,
    units: Vec<Unit>,
}

impl Workspace {
    pub fn load(manifest_dir: &Path) -> Result<Workspace, ArchError> {
        let metadata = MetadataCommand::new()
            .current_dir(manifest_dir)
            .no_deps()
            .exec()
            .map_err(|e| ArchError::Metadata(e.to_string()))?;
        let units = metadata
            .workspace_packages()
            .into_iter()
            .map(to_unit)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Workspace {
            root: metadata.workspace_root.into_std_path_buf(),
            units,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn units(&self, role: Role) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(move |unit| unit.role == role)
    }

    pub fn to_relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

fn find_role(package: &Package) -> Result<Role, ArchError> {
    match package
        .metadata
        .get("arche")
        .and_then(|arche| arche.get("role"))
        .and_then(|role| role.as_str())
    {
        Some("service") => Ok(Role::Service),
        Some("core") => Ok(Role::Core),
        Some("fakes") => Ok(Role::Fakes),
        Some("app") => Ok(Role::App),
        _ => Err(ArchError::NoRole(package.name.to_string())),
    }
}

fn to_unit(package: &Package) -> Result<Unit, ArchError> {
    let manifest = package.manifest_path.clone().into_std_path_buf();
    let dir = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
    let root_file = package
        .targets
        .iter()
        .find(|target| target.is_lib())
        .or_else(|| package.targets.first())
        .map(|target| target.src_path.clone().into_std_path_buf())
        .unwrap_or_else(|| dir.join("src/lib.rs"));
    Ok(Unit {
        name: package.name.to_string(),
        role: find_role(package)?,
        manifest,
        root_file,
    })
}

pub(crate) fn parse(path: &Path) -> Result<syn::File, ArchError> {
    let source = fs::read_to_string(path).map_err(|e| to_source_error(path, e))?;
    syn::parse_file(&source).map_err(|e| to_source_error(path, e))
}

pub(crate) fn to_source_error(path: &Path, reason: impl ToString) -> ArchError {
    ArchError::Source {
        path: path.display().to_string(),
        reason: reason.to_string(),
    }
}

pub(crate) fn list_use_paths(tree: &UseTree) -> Vec<String> {
    match tree {
        UseTree::Path(p) => list_use_paths(&p.tree)
            .into_iter()
            .map(|rest| format!("{}::{rest}", p.ident))
            .collect(),
        UseTree::Name(n) => vec![n.ident.to_string()],
        UseTree::Rename(r) => vec![format!("{} as {}", r.ident, r.rename)],
        UseTree::Glob(_) => vec!["*".to_string()],
        UseTree::Group(g) => g.items.iter().flat_map(list_use_paths).collect(),
    }
}

pub(crate) fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub at: String,
}

impl Finding {
    pub fn new(id: impl Into<String>, at: impl Into<String>) -> Finding {
        Finding {
            id: id.into(),
            at: at.into(),
        }
    }

    pub(crate) fn to_line(&self) -> String {
        if self.at.is_empty() || self.id.contains(&self.at) {
            return self.id.clone();
        }
        format!("{}  ({})", self.id, self.at)
    }
}
