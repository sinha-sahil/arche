use syn::Item;

use crate::architecture::{ArchError, Finding, Role, Workspace, is_pub, list_use_paths, parse};

fn list_exports(item: &Item) -> Vec<String> {
    match item {
        Item::Mod(m) if is_pub(&m.vis) && m.content.is_some() => {
            vec![format!("pub mod {} (inline)", m.ident)]
        }
        Item::Mod(m) if is_pub(&m.vis) => vec![format!("pub mod {}", m.ident)],
        Item::Use(u) if is_pub(&u.vis) => list_use_paths(&u.tree)
            .into_iter()
            .map(|p| format!("pub use {p}"))
            .collect(),
        Item::Fn(f) if is_pub(&f.vis) => vec![format!("pub fn {}", f.sig.ident)],
        Item::Struct(s) if is_pub(&s.vis) => vec![format!("pub struct {}", s.ident)],
        Item::Enum(e) if is_pub(&e.vis) => vec![format!("pub enum {}", e.ident)],
        Item::Trait(t) if is_pub(&t.vis) => vec![format!("pub trait {}", t.ident)],
        Item::Type(t) if is_pub(&t.vis) => vec![format!("pub type {}", t.ident)],
        Item::Const(c) if is_pub(&c.vis) => vec![format!("pub const {}", c.ident)],
        Item::Static(s) if is_pub(&s.vis) => vec![format!("pub static {}", s.ident)],
        Item::ExternCrate(e) if is_pub(&e.vis) => vec![format!("pub extern crate {}", e.ident)],
        Item::Macro(m) if m.ident.is_none() => {
            let path: Vec<String> = m
                .mac
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            vec![format!("macro call {}", path.join("::"))]
        }
        Item::Macro(m) if m.attrs.iter().any(|a| a.path().is_ident("macro_export")) => {
            vec![format!(
                "pub macro {}",
                m.ident
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            )]
        }
        _ => vec![],
    }
}

pub(crate) fn list_extra_exports(root_file: &syn::File, allowed: &[&str]) -> Vec<String> {
    root_file
        .items
        .iter()
        .flat_map(list_exports)
        .filter(|name| !allowed.contains(&name.as_str()))
        .collect()
}

pub fn find_extra_exports(ws: &Workspace, allowed: &[&str]) -> Result<Vec<Finding>, ArchError> {
    ws.units(Role::Service)
        .map(|unit| {
            let root_file = parse(&unit.root_file)?;
            Ok(list_extra_exports(&root_file, allowed)
                .into_iter()
                .map(|name| {
                    Finding::new(
                        format!("{}: {name}", unit.name),
                        ws.to_relative(&unit.root_file),
                    )
                })
                .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, ArchError>>()
        .map(|lists| lists.concat())
}

#[cfg(test)]
#[path = "../../../tests/architecture/checks/exports.rs"]
mod tests;
