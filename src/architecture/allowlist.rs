use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;

use crate::architecture::{ArchError, Finding, Workspace, to_source_error};

pub struct Allowlist {
    rule: String,
    entries: BTreeSet<String>,
}

impl Allowlist {
    pub fn load(ws: &Workspace, rule: &str) -> Result<Allowlist, ArchError> {
        let path = ws
            .root()
            .join(format!("architecture/allowlists/{rule}.txt"));
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(to_source_error(&path, e)),
        };
        Ok(Allowlist {
            rule: rule.to_string(),
            entries: parse_entries(&text),
        })
    }

    #[track_caller]
    pub fn assert_clean(&self, text: &str, found: &[Finding]) {
        let new = list_new(&self.entries, found);
        let fixed = list_fixed(&self.entries, found);
        assert!(
            new.is_empty(),
            "\n{}: {text}\n\n{}\n",
            self.rule,
            new.join("\n")
        );
        assert!(
            fixed.is_empty(),
            "\n{}: these are fixed; delete them from architecture/allowlists/{}.txt\n\n{}\n",
            self.rule,
            self.rule,
            fixed.join("\n")
        );
    }
}

pub(crate) fn parse_entries(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|line| {
            line.split('#')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

pub(crate) fn list_new(entries: &BTreeSet<String>, found: &[Finding]) -> Vec<String> {
    found
        .iter()
        .filter(|finding| !entries.contains(&finding.id))
        .map(Finding::to_line)
        .collect()
}

pub(crate) fn list_fixed(entries: &BTreeSet<String>, found: &[Finding]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| !found.iter().any(|finding| &finding.id == *entry))
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "../../tests/architecture/allowlist.rs"]
mod tests;
