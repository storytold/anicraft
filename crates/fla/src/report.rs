//! Why a legacy document couldn't be imported, and what the import kept or left out —
//! the same shape the XFL importer reports, so the engine's open path surfaces both alike.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Container(#[from] crate::cfb::Error),
    #[error("this .fla is from an unsupported Flash version: AniCraft reads the Flash 8–CS4 era")]
    UnsupportedVersion,
    #[error("this legacy Flash document is damaged: {0}")]
    BadStructure(&'static str),
}

/// What the legacy import kept and what it left out, by kind (`skipped`, `kept`, `runs`,
/// `missing_fonts`, `names` — as in the XFL importer's [`crate::Report`] counterpart).
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub skipped: BTreeMap<String, usize>,
    pub kept: BTreeMap<String, usize>,
    pub runs: BTreeMap<String, usize>,
    pub missing_fonts: BTreeSet<String>,
    pub notes: Vec<String>,
    /// The items behind some kinds of `skipped`, by name (ADR 0086's pattern).
    pub names: BTreeMap<String, Vec<String>>,
}

/// Most names kept per kind of skipped item (as in the XFL report).
pub const MAX_NAMED: usize = 64;

impl Report {
    pub fn skip(&mut self, what: &str) {
        *self.skipped.entry(what.to_string()).or_default() += 1;
    }
    /// Counts `what` and keeps the item's `name` for the report.
    pub fn skip_named(&mut self, what: &str, name: String) {
        self.skip(what);
        let names = self.names.entry(what.to_string()).or_default();
        if names.len() < MAX_NAMED {
            names.push(name);
        }
    }
    pub fn keep(&mut self, what: &str) {
        *self.kept.entry(what.to_string()).or_default() += 1;
    }
    /// One sentence for the status line: what opened, and what didn't come across.
    pub fn summary(&self) -> String {
        if self.skipped.is_empty() {
            return "imported fully".into();
        }
        let parts: Vec<String> = self.skipped.iter().map(|(k, n)| format!("{n} {k}")).collect();
        match parts.as_slice() {
            [one] => one.clone(),
            [rest @ .., last] => format!("opened without {} and {last}", rest.join(", ")),
            [] => "imported fully".into(),
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}
