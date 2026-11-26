use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{ItemUse, Macro, UseTree};

use crate::architecture::{ArchError, Finding, Workspace, parse, to_source_error};

type Graph = BTreeMap<String, BTreeSet<String>>;

struct References<'a> {
    prefix: &'a str,
    heads: Vec<&'a str>,
    found: BTreeSet<String>,
}

impl References<'_> {
    fn record(&mut self, segments: &[String]) {
        let matches_prefix = segments.len() > self.heads.len()
            && segments
                .iter()
                .zip(&self.heads)
                .all(|(segment, head)| segment == head);
        if matches_prefix {
            self.found.insert(segments[self.heads.len()].clone());
        }
    }
}

impl<'ast> Visit<'ast> for References<'_> {
    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        for segments in list_use_segments(&item.tree) {
            self.record(&segments);
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.record(&segments);
        visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        let names = list_macro_references(&mac.tokens.to_string(), self.prefix);
        self.found.extend(names);
        visit::visit_macro(self, mac);
    }
}

fn list_use_segments(tree: &UseTree) -> Vec<Vec<String>> {
    match tree {
        UseTree::Path(p) => list_use_segments(&p.tree)
            .into_iter()
            .map(|tail| [vec![p.ident.to_string()], tail].concat())
            .collect(),
        UseTree::Name(n) => vec![vec![n.ident.to_string()]],
        UseTree::Rename(r) => vec![vec![r.ident.to_string()]],
        UseTree::Glob(_) => vec![vec!["*".to_string()]],
        UseTree::Group(g) => g.items.iter().flat_map(list_use_segments).collect(),
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn list_macro_references(tokens: &str, prefix: &str) -> Vec<String> {
    let tokens = tokens.replace(" :: ", "::");
    let needle = format!("{prefix}::");
    tokens
        .match_indices(&needle)
        .filter(|(at, _)| !tokens[..*at].ends_with(is_ident_char))
        .map(|(at, _)| {
            tokens[at + needle.len()..]
                .chars()
                .take_while(|c| is_ident_char(*c))
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .collect()
}

pub(crate) fn list_references(file: &syn::File, prefix: &str) -> BTreeSet<String> {
    let mut references = References {
        prefix,
        heads: prefix.split("::").collect(),
        found: BTreeSet::new(),
    };
    references.visit_file(file);
    references.found
}

fn reaches(graph: &Graph, from: &str, to: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(node) = stack.pop() {
        if node == to {
            return true;
        }
        if !seen.insert(node) {
            continue;
        }
        stack.extend(graph.get(node).into_iter().flatten().map(String::as_str));
    }
    false
}

pub(crate) fn list_cycle_edges(graph: &Graph) -> Vec<(String, String)> {
    graph
        .iter()
        .flat_map(|(from, targets)| targets.iter().map(move |to| (from, to)))
        .filter(|(from, to)| reaches(graph, to, from))
        .map(|(from, to)| (from.clone(), to.clone()))
        .collect()
}

fn list_rust_files(path: &Path) -> Result<Vec<PathBuf>, ArchError> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let entries = fs::read_dir(path).map_err(|e| to_source_error(path, e))?;
    entries
        .map(|entry| {
            let child = entry.map_err(|e| to_source_error(path, e))?.path();
            if child.is_dir() {
                return list_rust_files(&child);
            }
            Ok(child
                .extension()
                .is_some_and(|ext| ext == "rs")
                .then_some(child)
                .into_iter()
                .collect())
        })
        .collect::<Result<Vec<_>, ArchError>>()
        .map(|lists| lists.concat())
}

fn to_module_name(path: &Path) -> Option<String> {
    let is_module = path.is_dir() || path.extension().is_some_and(|ext| ext == "rs");
    let name = path.file_stem()?.to_string_lossy().into_owned();
    (is_module && name != "mod").then_some(name)
}

fn list_modules(dir: &Path) -> Result<BTreeMap<String, Vec<PathBuf>>, ArchError> {
    let entries = fs::read_dir(dir).map_err(|e| to_source_error(dir, e))?;
    let paths = entries
        .map(|entry| Ok(entry.map_err(|e| to_source_error(dir, e))?.path()))
        .collect::<Result<Vec<PathBuf>, ArchError>>()?;
    let mut modules: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for path in paths {
        if let Some(name) = to_module_name(&path) {
            modules.entry(name).or_default().push(path);
        }
    }
    Ok(modules)
}

fn read_references(
    paths: &[PathBuf],
    prefix: &str,
) -> Result<BTreeMap<String, PathBuf>, ArchError> {
    let mut files = paths
        .iter()
        .map(|path| list_rust_files(path))
        .collect::<Result<Vec<_>, ArchError>>()?
        .concat();
    files.sort();
    let mut first_seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for file in files {
        for target in list_references(&parse(&file)?, prefix) {
            first_seen.entry(target).or_insert_with(|| file.clone());
        }
    }
    Ok(first_seen)
}

pub fn find_module_cycle_edges(
    ws: &Workspace,
    modules_dir: &str,
    prefix: &str,
) -> Result<Vec<Finding>, ArchError> {
    let modules = list_modules(&ws.root().join(modules_dir))?;
    let references = modules
        .iter()
        .map(|(name, paths)| {
            let targets: BTreeMap<String, PathBuf> = read_references(paths, prefix)?
                .into_iter()
                .filter(|(target, _)| target != name && modules.contains_key(target))
                .collect();
            Ok((name.clone(), targets))
        })
        .collect::<Result<BTreeMap<String, BTreeMap<String, PathBuf>>, ArchError>>()?;
    let graph: Graph = references
        .iter()
        .map(|(name, targets)| (name.clone(), targets.keys().cloned().collect()))
        .collect();
    Ok(list_cycle_edges(&graph)
        .into_iter()
        .map(|(from, to)| {
            let at = references
                .get(&from)
                .and_then(|targets| targets.get(&to))
                .map(|file| ws.to_relative(file))
                .unwrap_or_default();
            Finding::new(format!("{from} -> {to}"), at)
        })
        .collect())
}

#[cfg(test)]
#[path = "../../../tests/architecture/checks/cycles.rs"]
mod tests;
