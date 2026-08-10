//! Data exchanged between per-file scanning and catalog aggregation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use trox::ArgumentSchema;
use trox::IdentityDescriptor;

use crate::diagnostic::Diagnostic;

#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

impl SourceLocation {
    pub fn display(&self, root: &Path) -> String {
        let relative = self.path.strip_prefix(root).unwrap_or(&self.path);
        format!(
            "{}:{}:{}",
            relative.to_string_lossy().replace('\\', "/"),
            self.line,
            self.column
        )
    }
}

#[derive(Debug, Clone)]
pub struct ExtractedMessage {
    pub identity: IdentityDescriptor,
    pub entry_id: String,
    pub source_signature: String,
    pub description: Option<String>,
    pub arguments: BTreeMap<String, ArgumentSchema>,
    /// Per-term-argument literal reachability. `None` means the term ID is dynamic.
    pub term_ids: BTreeMap<String, Option<String>>,
    pub selector_labels: BTreeMap<Vec<usize>, String>,
    pub predicate_labels: BTreeMap<Vec<usize>, Vec<String>>,
    pub location: SourceLocation,
}

#[derive(Debug)]
pub struct ScanResult {
    pub messages: Vec<ExtractedMessage>,
    pub diagnostics: Vec<Diagnostic>,
    pub bytes_scanned: u64,
}
