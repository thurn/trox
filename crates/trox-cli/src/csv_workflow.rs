//! Deterministic translator CSV synchronization.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use csv::{ReaderBuilder, StringRecord, Terminator, WriterBuilder};

use crate::diagnostic::{Diagnostic, DiagnosticResultExt, Diagnostics, Span};
use crate::extract::ExpectedRow;
use crate::scanner::parse_placeholders;

pub const MANAGED_COLUMNS: [&str; 12] = [
    "english",
    "description",
    "translation",
    "status",
    "translator_note",
    "placeholders",
    "entry_id",
    "row_id",
    "kind",
    "previous_translation",
    "source_locations",
    "source_revision",
];

const LEGACY_MANAGED_COLUMNS: [&str; 13] = [
    "english",
    "description",
    "translation",
    "conditions",
    "status",
    "translator_note",
    "placeholders",
    "entry_id",
    "row_id",
    "kind",
    "previous_translation",
    "source_locations",
    "source_revision",
];

#[derive(Debug, Clone, Default)]
pub struct CsvRow {
    pub english: String,
    pub description: String,
    pub translation: String,
    pub conditions: String,
    pub status: String,
    pub translator_note: String,
    pub placeholders: String,
    pub entry_id: String,
    pub row_id: String,
    pub kind: String,
    pub previous_translation: String,
    pub source_locations: String,
    pub source_revision: String,
    pub extras: Vec<String>,
    source_line: Option<usize>,
}

impl CsvRow {
    fn from_record(
        record: &StringRecord,
        extras: usize,
        source_line: usize,
        legacy_conditions_column: bool,
    ) -> Self {
        let get = |index| record.get(index).unwrap_or("").to_owned();
        let offset = usize::from(legacy_conditions_column);
        let conditions = if legacy_conditions_column {
            get(3)
        } else {
            String::new()
        };
        let mut description = get(1);
        if !conditions.is_empty() {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Conditions: ");
            description.push_str(&conditions);
        }
        Self {
            english: get(0),
            description,
            translation: get(2),
            conditions,
            status: get(3 + offset),
            translator_note: get(4 + offset),
            placeholders: get(5 + offset),
            entry_id: get(6 + offset),
            row_id: get(7 + offset),
            kind: get(8 + offset),
            previous_translation: get(9 + offset),
            source_locations: get(10 + offset),
            source_revision: get(11 + offset),
            extras: (0..extras)
                .map(|index| get(MANAGED_COLUMNS.len() + offset + index))
                .collect(),
            source_line: Some(source_line),
        }
    }

    fn record(&self) -> Vec<&str> {
        let mut result = vec![
            self.english.as_str(),
            self.description.as_str(),
            self.translation.as_str(),
            self.status.as_str(),
            self.translator_note.as_str(),
            self.placeholders.as_str(),
            self.entry_id.as_str(),
            self.row_id.as_str(),
            self.kind.as_str(),
            self.previous_translation.as_str(),
            self.source_locations.as_str(),
            self.source_revision.as_str(),
        ];
        result.extend(self.extras.iter().map(String::as_str));
        result
    }
}

#[derive(Debug, Clone)]
pub struct CsvDocument {
    pub extra_headers: Vec<String>,
    pub rows: Vec<CsvRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslatorEdit {
    pub translation: String,
    pub translator_note: String,
}

impl CsvDocument {
    pub fn empty() -> Self {
        Self {
            extra_headers: vec![],
            rows: vec![],
        }
    }
}

pub fn read_csv(path: &Path) -> Result<CsvDocument> {
    read_csv_impl(path).diagnostic(
        "trox.invalid-csv",
        Some(path.to_path_buf()),
        "Restore the canonical CSV shape or rerun trox extract after preserving editable cells.",
    )
}

fn read_csv_impl(path: &Path) -> Result<CsvDocument> {
    if !path.exists() {
        return Ok(CsvDocument::empty());
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        bail!("{} contains a UTF-8 BOM", path.display());
    }
    if bytes.windows(2).any(|window| window == b"\r\n") || bytes.contains(&b'\r') {
        bail!("{} must use LF line endings", path.display());
    }
    let mut reader = ReaderBuilder::new()
        .flexible(false)
        .from_reader(bytes.as_slice());
    let headers = reader
        .headers()
        .with_context(|| format!("invalid CSV header in {}", path.display()))?
        .clone();
    let legacy_conditions_column = headers.get(3) == Some("conditions");
    let managed_columns: &[&str] = if legacy_conditions_column {
        &LEGACY_MANAGED_COLUMNS
    } else {
        &MANAGED_COLUMNS
    };
    if headers.len() < managed_columns.len() {
        bail!("{} is missing managed columns", path.display());
    }
    for (index, expected) in managed_columns.iter().enumerate() {
        if headers.get(index) != Some(expected) {
            bail!(
                "{} column {} must be `{expected}`",
                path.display(),
                index + 1
            );
        }
    }
    let extra_headers: Vec<_> = headers
        .iter()
        .skip(managed_columns.len())
        .map(str::to_owned)
        .collect();
    let mut unique = BTreeSet::new();
    for header in &extra_headers {
        if !unique.insert(header) {
            bail!("{} repeats extra column `{header}`", path.display());
        }
    }
    let rows = reader
        .records()
        .enumerate()
        .map(|(index, record)| {
            record.map(|record| {
                CsvRow::from_record(
                    &record,
                    extra_headers.len(),
                    index + 2,
                    legacy_conditions_column,
                )
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("malformed CSV in {}", path.display()))?;
    validate_document(&rows)?;
    Ok(CsvDocument {
        extra_headers,
        rows,
    })
}

fn validate_document(rows: &[CsvRow]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for row in rows {
        if !ids.insert(&row.row_id) {
            bail!("duplicate row ID `{}`", row.row_id);
        }
        if !["missing", "translated", "stale", "obsolete"].contains(&row.status.as_str()) {
            bail!("row `{}` has invalid status `{}`", row.row_id, row.status);
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ResolvedCell {
    text: Option<String>,
    predecessor: Option<String>,
}

fn resolve_carets(
    rows: &[CsvRow],
    active_ids: &BTreeSet<&str>,
) -> Result<BTreeMap<String, ResolvedCell>> {
    let mut result = BTreeMap::new();
    let mut previous: Option<&CsvRow> = None;
    for row in rows
        .iter()
        .filter(|row| active_ids.contains(row.row_id.as_str()))
    {
        let cell = if row.translation == "^" {
            let prior = previous
                .filter(|prior| prior.entry_id == row.entry_id)
                .with_context(|| {
                    format!(
                        "caret in row `{}` has no predecessor in the same entry",
                        row.row_id
                    )
                })?;
            let resolved = result
                .get(&prior.row_id)
                .and_then(|cell: &ResolvedCell| cell.text.clone());
            ResolvedCell {
                text: resolved,
                predecessor: Some(prior.row_id.clone()),
            }
        } else if row.translation.is_empty() {
            ResolvedCell {
                text: None,
                predecessor: None,
            }
        } else {
            ResolvedCell {
                text: Some(row.translation.clone()),
                predecessor: None,
            }
        };
        result.insert(row.row_id.clone(), cell);
        previous = Some(row);
    }
    Ok(result)
}

pub struct SyncResult {
    pub document: CsvDocument,
    pub bytes: Vec<u8>,
    pub changed: bool,
}

pub fn synchronize(
    path: &Path,
    expected: &[ExpectedRow],
    source_report: bool,
    diagnostics: &mut Diagnostics,
) -> Result<SyncResult> {
    synchronize_impl(path, expected, source_report, diagnostics).diagnostic(
        "trox.invalid-csv-workflow",
        Some(path.to_path_buf()),
        "Correct the CSV workflow violation and rerun trox extract.",
    )
}

fn synchronize_impl(
    path: &Path,
    expected: &[ExpectedRow],
    source_report: bool,
    diagnostics: &mut Diagnostics,
) -> Result<SyncResult> {
    let old = read_csv(path)?;
    let old_bytes = if path.exists() {
        fs::read(path)?
    } else {
        vec![]
    };
    let expected_ids: BTreeSet<_> = expected.iter().map(|row| row.row_id.as_str()).collect();
    let old_active_ids: BTreeSet<_> = old
        .rows
        .iter()
        .filter(|row| row.status != "obsolete" || expected_ids.contains(row.row_id.as_str()))
        .map(|row| row.row_id.as_str())
        .collect();
    let old_resolved = resolve_carets(&old.rows, &old_active_ids)?;
    let expected_rank: BTreeMap<_, _> = expected
        .iter()
        .enumerate()
        .map(|(index, row)| (row.row_id.as_str(), index))
        .collect();
    let mut prior_ranks = BTreeMap::new();
    for row in &old.rows {
        let Some(rank) = expected_rank.get(row.row_id.as_str()).copied() else {
            continue;
        };
        if prior_ranks
            .insert(row.entry_id.as_str(), rank)
            .is_some_and(|prior| prior >= rank)
        {
            bail!("active CSV rows are not in canonical expansion order");
        }
    }
    let active_old: BTreeMap<_, _> = old
        .rows
        .iter()
        .filter(|row| expected_ids.contains(row.row_id.as_str()))
        .map(|row| (row.row_id.clone(), row))
        .collect();
    let mut rows = Vec::new();
    let mut prior_new_id: Option<String> = None;
    for expected in expected {
        let prior = active_old.get(&expected.row_id).copied();
        let mut row = CsvRow {
            conditions: expected.conditions.clone(),
            english: expected.english.clone(),
            description: expected.description.clone(),
            placeholders: expected.placeholders.clone(),
            entry_id: expected.entry_id.clone(),
            row_id: expected.row_id.clone(),
            kind: expected.kind.clone(),
            source_locations: expected.source_locations.clone(),
            source_revision: expected.source_revision.clone(),
            extras: vec![String::new(); old.extra_headers.len()],
            source_line: prior.and_then(|row| row.source_line),
            ..CsvRow::default()
        };
        if source_report {
            row.translation = expected.english.clone();
            row.status = "translated".into();
            if let Some(prior) = prior {
                row.translator_note = prior.translator_note.clone();
                row.extras = prior.extras.clone();
            }
        } else if let Some(prior) = prior {
            row.translator_note = prior.translator_note.clone();
            row.extras = prior.extras.clone();
            if prior.source_revision == expected.source_revision {
                row.translation = prior.translation.clone();
                row.previous_translation = prior.previous_translation.clone();
                if row.translation == "^" {
                    let old_predecessor = old_resolved
                        .get(&row.row_id)
                        .and_then(|cell| cell.predecessor.as_deref());
                    if old_predecessor != prior_new_id.as_deref() {
                        row.translation = old_resolved
                            .get(&row.row_id)
                            .and_then(|cell| cell.text.clone())
                            .unwrap_or_default();
                    }
                }
            } else {
                row.previous_translation = old_resolved
                    .get(&row.row_id)
                    .and_then(|cell| cell.text.clone())
                    .or_else(|| {
                        (!prior.previous_translation.is_empty())
                            .then(|| prior.previous_translation.clone())
                    })
                    .unwrap_or_default();
                row.translation.clear();
            }
        }
        rows.push(row);
        prior_new_id = Some(expected.row_id.clone());
    }
    if !source_report {
        let mut obsolete: Vec<_> = old
            .rows
            .into_iter()
            .filter(|row| !expected_ids.contains(row.row_id.as_str()))
            .collect();
        for row in &mut obsolete {
            row.status = "obsolete".into();
        }
        obsolete.sort_by(|left, right| {
            (&left.entry_id, &left.row_id).cmp(&(&right.entry_id, &right.row_id))
        });
        rows.extend(obsolete);
    }
    derive_statuses(&mut rows, source_report)?;
    lint_rows(path, &rows, diagnostics, source_report)?;
    let document = CsvDocument {
        extra_headers: old.extra_headers,
        rows,
    };
    let bytes = write_csv(&document)?;
    let changed = bytes != old_bytes;
    Ok(SyncResult {
        document,
        bytes,
        changed,
    })
}

fn derive_statuses(rows: &mut [CsvRow], source_report: bool) -> Result<()> {
    #[derive(Clone)]
    enum PreviousState {
        Translated(String),
        Missing,
        Stale(String),
    }

    let mut previous: Option<(String, PreviousState)> = None;
    for row in rows.iter_mut().filter(|row| row.status != "obsolete") {
        if source_report {
            row.status = "translated".into();
            previous = Some((
                row.entry_id.clone(),
                PreviousState::Translated(row.translation.clone()),
            ));
            continue;
        }
        if row.translation == "^" {
            let state = previous
                .as_ref()
                .filter(|(entry_id, _)| entry_id == &row.entry_id)
                .map(|(_, state)| state.clone())
                .with_context(|| {
                    format!(
                        "caret in row `{}` has no predecessor in the same entry",
                        row.row_id
                    )
                })?;
            match state {
                PreviousState::Translated(text) => {
                    row.status = "translated".into();
                    previous = Some((row.entry_id.clone(), PreviousState::Translated(text)));
                }
                PreviousState::Missing => {
                    row.status = "missing".into();
                    row.previous_translation.clear();
                    previous = Some((row.entry_id.clone(), PreviousState::Missing));
                }
                PreviousState::Stale(inherited) => {
                    row.status = "stale".into();
                    if row.previous_translation.is_empty() {
                        row.previous_translation = inherited.clone();
                    }
                    previous = Some((row.entry_id.clone(), PreviousState::Stale(inherited)));
                }
            }
        } else if row.translation.is_empty() {
            row.status = if row.previous_translation.is_empty() {
                "missing"
            } else {
                "stale"
            }
            .into();
            let state = if row.status == "stale" {
                PreviousState::Stale(row.previous_translation.clone())
            } else {
                PreviousState::Missing
            };
            previous = Some((row.entry_id.clone(), state));
        } else {
            row.status = "translated".into();
            previous = Some((
                row.entry_id.clone(),
                PreviousState::Translated(row.translation.clone()),
            ));
        }
    }
    Ok(())
}

fn lint_rows(
    path: &Path,
    rows: &[CsvRow],
    diagnostics: &mut Diagnostics,
    source_report: bool,
) -> Result<()> {
    for (index, row) in rows
        .iter()
        .filter(|row| row.status != "obsolete")
        .enumerate()
    {
        let at_row = |diagnostic: Diagnostic| {
            let line = row.source_line.unwrap_or(index + 2);
            diagnostic.at(
                path,
                Span {
                    line,
                    column: 1,
                    end_line: line,
                    end_column: 2,
                },
            )
        };
        if !row.translation.is_empty() && row.translation != "^" {
            let declared: BTreeSet<_> = row
                .placeholders
                .split(';')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            let translated = match parse_placeholders(&row.translation) {
                Ok(translated) => translated,
                Err(message) => {
                    diagnostics.push(at_row(Diagnostic::error(
                        "trox.invalid-placeholder",
                        format!("row `{}`: {message}", row.row_id),
                    )));
                    continue;
                }
            };
            if !translated.is_subset(&declared) {
                diagnostics.push(at_row(Diagnostic::error(
                    "trox.unknown-placeholder",
                    format!(
                        "row `{}` translation contains unknown placeholders",
                        row.row_id
                    ),
                )));
            }
            if translated != declared {
                diagnostics.push(at_row(Diagnostic::warning(
                    "trox.omitted-placeholder",
                    format!(
                        "row `{}` omits one or more declared placeholders",
                        row.row_id
                    ),
                )));
            }
            if !source_report
                && row.translation == row.english
                && row.translator_note.trim().is_empty()
            {
                diagnostics.push(at_row(Diagnostic::warning(
                    "trox.unchanged-translation",
                    format!("row `{}` is unchanged from English", row.row_id),
                )));
            }
            if row.translation.starts_with(['=', '+', '-', '@']) {
                diagnostics.push(at_row(Diagnostic::warning(
                    "trox.formula-injection",
                    format!(
                        "row `{}` begins with a spreadsheet formula character",
                        row.row_id
                    ),
                )));
            }
            if row
                .translation
                .chars()
                .any(|ch| ch.is_control() && !['\n', '\t'].contains(&ch))
            {
                diagnostics.push(at_row(Diagnostic::error(
                    "trox.control-character",
                    format!(
                        "row `{}` contains a disallowed control character",
                        row.row_id
                    ),
                )));
            }
        }
    }
    Ok(())
}

pub fn write_csv(document: &CsvDocument) -> Result<Vec<u8>> {
    let mut writer = WriterBuilder::new()
        .terminator(Terminator::Any(b'\n'))
        .from_writer(vec![]);
    let mut headers: Vec<_> = MANAGED_COLUMNS
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    headers.extend(document.extra_headers.clone());
    writer.write_record(&headers)?;
    for row in &document.rows {
        writer.write_record(row.record())?;
    }
    writer.flush()?;
    Ok(writer.into_inner().map_err(|error| error.into_error())?)
}

pub fn apply_translator_edits(
    path: &Path,
    document: &CsvDocument,
    edits: &BTreeMap<String, TranslatorEdit>,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    let mut updated = document.clone();
    let active_ids: BTreeSet<_> = updated
        .rows
        .iter()
        .filter(|row| row.status != "obsolete")
        .map(|row| row.row_id.clone())
        .collect();
    let edit_ids: BTreeSet<_> = edits.keys().cloned().collect();
    if active_ids != edit_ids {
        bail!("translator handoff rows do not match the active catalog");
    }
    for row in updated
        .rows
        .iter_mut()
        .filter(|row| row.status != "obsolete")
    {
        let edit = &edits[&row.row_id];
        row.translation.clone_from(&edit.translation);
        row.translator_note.clone_from(&edit.translator_note);
    }
    derive_statuses(&mut updated.rows, false)?;
    lint_rows(path, &updated.rows, diagnostics, false)?;
    write_csv(&updated)
}

pub fn prune(
    path: &Path,
    expected: &[ExpectedRow],
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    let synchronized = synchronize(path, expected, false, diagnostics)?;
    if synchronized.changed {
        bail!(
            "{} is out of date; run trox extract before pruning",
            path.display()
        );
    }
    let mut document = synchronized.document;
    document.rows.retain(|row| row.status != "obsolete");
    write_csv(&document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn expected(revision: &str) -> ExpectedRow {
        ExpectedRow {
            conditions: String::new(),
            english: "Hello {name}".into(),
            description: "Greeting.".into(),
            placeholders: "name".into(),
            entry_id: "tx1_a".into(),
            row_id: "row1_a".into(),
            kind: "message".into(),
            source_locations: "a.ts:1:1".into(),
            source_revision: revision.into(),
            expansion: trox::ExpansionDescriptor {
                entry_signature: "a".into(),
                path: vec![],
            },
            term: None,
        }
    }

    fn expected_id(id: &str, revision: &str) -> ExpectedRow {
        let mut row = expected(revision);
        row.row_id = id.into();
        row
    }

    #[test]
    fn legacy_conditions_column_migrates_into_description() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        fs::write(
            &path,
            concat!(
                "english,description,translation,conditions,status,translator_note,placeholders,entry_id,row_id,kind,previous_translation,source_locations,source_revision,reviewer\n",
                "Hello {name},Greeting.,Hola {name},count.plural=one,translated,Keep it.,name,tx1_a,row1_a,message,,a.ts:1:1,rev1_same,Ada\n"
            ),
        )
        .unwrap();

        let document = read_csv(&path).unwrap();
        let row = &document.rows[0];
        assert_eq!(row.description, "Greeting.\n\nConditions: count.plural=one");
        assert_eq!(row.translation, "Hola {name}");
        assert_eq!(row.translator_note, "Keep it.");
        assert_eq!(row.extras, ["Ada"]);

        let output = String::from_utf8(write_csv(&document).unwrap()).unwrap();
        assert!(output.starts_with("english,description,translation,status,"));
        assert!(!output.lines().next().unwrap().contains("conditions"));
    }

    #[test]
    fn source_revision_stales_without_losing_work() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let mut diagnostics = Diagnostics::default();
        let first = synchronize(&path, &[expected("rev1_old")], false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut doc = read_csv(&path).unwrap();
        doc.rows[0].translation = "Hola {name}".into();
        doc.rows[0].status = "translated".into();
        fs::write(&path, write_csv(&doc).unwrap()).unwrap();
        let second = synchronize(&path, &[expected("rev1_new")], false, &mut diagnostics).unwrap();
        assert_eq!(second.document.rows[0].status, "stale");
        assert_eq!(second.document.rows[0].previous_translation, "Hola {name}");
        assert!(second.document.rows[0].translation.is_empty());
    }

    #[test]
    fn translator_edits_update_active_rows_without_pruning_translation_memory() {
        let path = Path::new("es.csv");
        let mut obsolete = CsvRow {
            row_id: "old".into(),
            entry_id: "old_entry".into(),
            status: "obsolete".into(),
            translation: "Trabajo anterior".into(),
            ..CsvRow::default()
        };
        obsolete.extras.push("reviewed".into());
        let document = CsvDocument {
            extra_headers: vec!["reviewer".into()],
            rows: vec![
                CsvRow {
                    english: "Hello {name}".into(),
                    translation: String::new(),
                    status: "missing".into(),
                    placeholders: "name".into(),
                    entry_id: "entry".into(),
                    row_id: "active".into(),
                    extras: vec![String::new()],
                    ..CsvRow::default()
                },
                obsolete,
            ],
        };
        let edits = BTreeMap::from([(
            "active".into(),
            TranslatorEdit {
                translation: "Hola {name}".into(),
                translator_note: "Friendly.".into(),
            },
        )]);
        let mut diagnostics = Diagnostics::default();

        let bytes = apply_translator_edits(path, &document, &edits, &mut diagnostics).unwrap();
        let directory = tempdir().unwrap();
        let output = directory.path().join("es.csv");
        fs::write(&output, bytes).unwrap();
        let updated = read_csv(&output).unwrap();

        assert_eq!(updated.rows[0].translation, "Hola {name}");
        assert_eq!(updated.rows[0].translator_note, "Friendly.");
        assert_eq!(updated.rows[0].status, "translated");
        assert_eq!(updated.rows[1].status, "obsolete");
        assert_eq!(updated.rows[1].translation, "Trabajo anterior");
        assert_eq!(updated.rows[1].extras, ["reviewed"]);
        assert!(!diagnostics.has_errors());
    }

    #[test]
    fn source_report_drops_obsolete_rows_during_synchronization() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("en-US.csv");
        let mut diagnostics = Diagnostics::default();
        let initial = [
            expected_id("row1_a", "rev1_same"),
            expected_id("row1_b", "rev1_same"),
        ];
        let first = synchronize(&path, &initial, true, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();

        let current = [expected_id("row1_a", "rev1_same")];
        let synchronized = synchronize(&path, &current, true, &mut diagnostics).unwrap();

        assert_eq!(synchronized.document.rows.len(), 1);
        assert_eq!(synchronized.document.rows[0].row_id, "row1_a");
        assert!(
            synchronized
                .document
                .rows
                .iter()
                .all(|row| row.status != "obsolete")
        );
    }

    #[test]
    fn caret_chains_survive_stable_order_and_materialize_when_predecessors_change() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let mut diagnostics = Diagnostics::default();
        let expected = [
            expected_id("row1_a", "rev1_same"),
            expected_id("row1_b", "rev1_same"),
            expected_id("row1_c", "rev1_same"),
        ];
        let first = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut doc = read_csv(&path).unwrap();
        doc.rows[0].translation = "Hola {name}".into();
        doc.rows[0].status = "translated".into();
        doc.rows[1].translation = "^".into();
        doc.rows[1].status = "translated".into();
        doc.rows[2].translation = "^".into();
        doc.rows[2].status = "translated".into();
        fs::write(&path, write_csv(&doc).unwrap()).unwrap();
        let stable = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        assert_eq!(
            stable
                .document
                .rows
                .iter()
                .map(|row| row.translation.as_str())
                .collect::<Vec<_>>(),
            vec!["Hola {name}", "^", "^"]
        );
        fs::write(&path, stable.bytes).unwrap();
        let inserted = [
            expected_id("row1_a", "rev1_same"),
            expected_id("row1_x", "rev1_same"),
            expected_id("row1_b", "rev1_same"),
            expected_id("row1_c", "rev1_same"),
        ];
        let changed = synchronize(&path, &inserted, false, &mut diagnostics).unwrap();
        assert_eq!(changed.document.rows[2].translation, "Hola {name}");
        assert_eq!(changed.document.rows[3].translation, "^");
    }

    #[test]
    fn stale_inherited_row_cascades_through_caret_dependents() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let mut diagnostics = Diagnostics::default();
        let old = [
            expected_id("row1_a", "rev1_old"),
            expected_id("row1_b", "rev1_old"),
            expected_id("row1_c", "rev1_old"),
        ];
        let first = synchronize(&path, &old, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut doc = read_csv(&path).unwrap();
        doc.rows[0].translation = "Hola {name}".into();
        doc.rows[0].status = "translated".into();
        for row in &mut doc.rows[1..] {
            row.translation = "^".into();
            row.status = "translated".into();
        }
        fs::write(&path, write_csv(&doc).unwrap()).unwrap();
        let changed = [
            expected_id("row1_a", "rev1_new"),
            expected_id("row1_b", "rev1_old"),
            expected_id("row1_c", "rev1_old"),
        ];
        let result = synchronize(&path, &changed, false, &mut diagnostics).unwrap();
        assert!(result.document.rows.iter().all(|row| row.status == "stale"));
        assert!(
            result
                .document
                .rows
                .iter()
                .all(|row| row.previous_translation == "Hola {name}")
        );
    }

    #[test]
    fn spreadsheet_reordering_within_an_entry_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let expected = [
            expected_id("row1_a", "rev1_same"),
            expected_id("row1_b", "rev1_same"),
        ];
        let mut diagnostics = Diagnostics::default();
        let first = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut document = read_csv(&path).unwrap();
        document.rows.swap(0, 1);
        fs::write(&path, write_csv(&document).unwrap()).unwrap();
        assert!(
            synchronize(&path, &expected, false, &mut diagnostics)
                .err()
                .unwrap()
                .to_string()
                .contains("canonical expansion order")
        );
    }

    #[test]
    fn changed_entry_order_is_normalized_without_losing_translations() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let mut second = expected_id("row1_b", "rev1_same");
        second.entry_id = "tx1_b".into();
        let expected = [expected_id("row1_a", "rev1_same"), second];
        let mut diagnostics = Diagnostics::default();
        let first = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut document = read_csv(&path).unwrap();
        document.rows[0].translation = "Primero".into();
        document.rows[1].translation = "Segundo".into();
        document.rows.swap(0, 1);
        fs::write(&path, write_csv(&document).unwrap()).unwrap();

        let normalized = synchronize(&path, &expected, false, &mut diagnostics).unwrap();

        assert_eq!(
            normalized
                .document
                .rows
                .iter()
                .map(|row| (row.entry_id.as_str(), row.translation.as_str()))
                .collect::<Vec<_>>(),
            [("tx1_a", "Primero"), ("tx1_b", "Segundo")]
        );
    }

    #[test]
    fn prune_rejects_an_obsolete_status_for_a_current_source_row() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let expected = [expected("rev1_same")];
        let mut diagnostics = Diagnostics::default();
        let first = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut document = read_csv(&path).unwrap();
        document.rows[0].translation = "Hola {name}".into();
        document.rows[0].status = "obsolete".into();
        fs::write(&path, write_csv(&document).unwrap()).unwrap();

        assert!(prune(&path, &expected, &mut diagnostics).is_err());
        assert_eq!(read_csv(&path).unwrap().rows[0].translation, "Hola {name}");
    }

    #[test]
    fn synchronization_preserves_work_when_managed_status_is_corrupted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let expected = [expected("rev1_same")];
        let mut diagnostics = Diagnostics::default();
        let first = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut document = read_csv(&path).unwrap();
        document.extra_headers.push("reviewer".into());
        document.rows[0].translation = "Hola {name}".into();
        document.rows[0].translator_note = "Keep this wording.".into();
        document.rows[0].extras = vec!["Ada".into()];
        document.rows[0].status = "obsolete".into();
        fs::write(&path, write_csv(&document).unwrap()).unwrap();

        let synchronized = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        let row = &synchronized.document.rows[0];
        assert_eq!(row.translation, "Hola {name}");
        assert_eq!(row.translator_note, "Keep this wording.");
        assert_eq!(row.extras, ["Ada"]);
        assert_eq!(row.status, "translated");
    }

    #[test]
    fn carets_inherit_missing_and_stale_statuses() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let expected = [
            expected_id("row1_a", "rev1_same"),
            expected_id("row1_b", "rev1_same"),
        ];
        let mut diagnostics = Diagnostics::default();
        let first = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut document = read_csv(&path).unwrap();
        document.rows[1].translation = "^".into();
        document.rows[1].status = "translated".into();
        fs::write(&path, write_csv(&document).unwrap()).unwrap();

        let missing = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        assert_eq!(missing.document.rows[1].translation, "^");
        assert_eq!(missing.document.rows[1].status, "missing");

        let mut stale = missing.document;
        stale.rows[0].previous_translation = "Hola {name}".into();
        stale.rows[0].status = "stale".into();
        fs::write(&path, write_csv(&stale).unwrap()).unwrap();
        let stale = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        assert_eq!(stale.document.rows[1].translation, "^");
        assert_eq!(stale.document.rows[1].status, "stale");
        assert_eq!(stale.document.rows[1].previous_translation, "Hola {name}");
    }

    #[test]
    fn csv_diagnostics_include_the_input_path_and_record_line() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("es.csv");
        let expected = [expected("rev1_same")];
        let mut diagnostics = Diagnostics::default();
        let first = synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        fs::write(&path, first.bytes).unwrap();
        let mut document = read_csv(&path).unwrap();
        document.rows[0].translation = "Hola {unknown}".into();
        document.rows[0].status = "translated".into();
        fs::write(&path, write_csv(&document).unwrap()).unwrap();

        let mut diagnostics = Diagnostics::default();
        synchronize(&path, &expected, false, &mut diagnostics).unwrap();
        let diagnostic = diagnostics
            .iter()
            .find(|item| item.rule_id == "trox.unknown-placeholder")
            .unwrap();
        assert_eq!(diagnostic.path.as_deref(), Some(path.as_path()));
        assert_eq!(diagnostic.span.unwrap().line, 2);
    }
}
