//! Curated, round-trippable translator workbooks.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use calamine::{Data, Range, Reader, Xlsx, open_workbook};
use rust_xlsxwriter::{Color, Format, FormatAlign, ProtectionOptions, Workbook, XlsxError};
use serde::Serialize;

use crate::csv_workflow::{CsvDocument, CsvRow};

const FORMAT: &str = "trox-translator-handoff";
const VERSION: &str = "1";
const INSTRUCTIONS_SHEET: &str = "Instructions";
const TRANSLATIONS_SHEET: &str = "Translations";
const MANIFEST_SHEET: &str = "Manifest";

const HEADERS: [&str; 13] = [
    "English",
    "Context",
    "Translation",
    "Translator note",
    "Status",
    "Previous translation",
    "Placeholders",
    "entry_id",
    "row_id",
    "kind",
    "source_locations",
    "source_revision",
    "source_row_fingerprint",
];

#[derive(Serialize)]
struct Snapshot<'a> {
    format: &'static str,
    version: &'static str,
    locale: &'a str,
    source_locale: &'a str,
    source_catalog_fingerprint: &'a str,
    rows: Vec<SnapshotRow<'a>>,
}

#[derive(Serialize)]
struct SnapshotRow<'a> {
    english: &'a str,
    description: &'a str,
    translation: &'a str,
    status: &'a str,
    translator_note: &'a str,
    placeholders: &'a str,
    entry_id: &'a str,
    row_id: &'a str,
    kind: &'a str,
    previous_translation: &'a str,
    source_locations: &'a str,
    source_revision: &'a str,
}

fn active_rows(document: &CsvDocument) -> Vec<&CsvRow> {
    document
        .rows
        .iter()
        .filter(|row| row.status != "obsolete")
        .collect()
}

fn snapshot<'a>(
    locale: &'a str,
    source_locale: &'a str,
    source_catalog_fingerprint: &'a str,
    document: &'a CsvDocument,
) -> Snapshot<'a> {
    Snapshot {
        format: FORMAT,
        version: VERSION,
        locale,
        source_locale,
        source_catalog_fingerprint,
        rows: active_rows(document)
            .into_iter()
            .map(|row| SnapshotRow {
                english: &row.english,
                description: &row.description,
                translation: &row.translation,
                status: &row.status,
                translator_note: &row.translator_note,
                placeholders: &row.placeholders,
                entry_id: &row.entry_id,
                row_id: &row.row_id,
                kind: &row.kind,
                previous_translation: &row.previous_translation,
                source_locations: &row.source_locations,
                source_revision: &row.source_revision,
            })
            .collect(),
    }
}

fn snapshot_fingerprint(snapshot: &Snapshot<'_>) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(snapshot)?)
        .to_hex()
        .to_string())
}

fn source_row_fingerprint(row: &CsvRow) -> Result<String> {
    let immutable = (
        &row.english,
        &row.description,
        &row.status,
        &row.placeholders,
        &row.entry_id,
        &row.row_id,
        &row.kind,
        &row.previous_translation,
        &row.source_locations,
        &row.source_revision,
    );
    Ok(blake3::hash(&serde_json::to_vec(&immutable)?)
        .to_hex()
        .to_string())
}

pub fn export_workbook(
    locale: &str,
    source_locale: &str,
    source_catalog_fingerprint: &str,
    document: &CsvDocument,
) -> Result<Vec<u8>> {
    export_workbook_with_edits(
        locale,
        source_locale,
        source_catalog_fingerprint,
        document,
        None,
    )
}

fn export_workbook_with_edits(
    locale: &str,
    source_locale: &str,
    source_catalog_fingerprint: &str,
    document: &CsvDocument,
    edits: Option<&BTreeMap<String, crate::csv_workflow::TranslatorEdit>>,
) -> Result<Vec<u8>> {
    let rows = active_rows(document);
    if rows.is_empty() {
        bail!("cannot export an empty translator handoff");
    }
    let snapshot = snapshot(locale, source_locale, source_catalog_fingerprint, document);
    let package_fingerprint = snapshot_fingerprint(&snapshot)?;
    let mut workbook = Workbook::new();
    write_instructions(&mut workbook, locale, source_locale, rows.len())?;
    write_translations(&mut workbook, &rows, edits)?;
    write_manifest(
        &mut workbook,
        locale,
        source_locale,
        source_catalog_fingerprint,
        rows.len(),
        &package_fingerprint,
    )?;
    workbook
        .save_to_buffer()
        .context("failed to encode translator workbook")
}

pub fn write_workbook(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".trox-handoff-")
        .tempfile_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn write_instructions(
    workbook: &mut Workbook,
    locale: &str,
    source_locale: &str,
    row_count: usize,
) -> std::result::Result<(), XlsxError> {
    let title = Format::new()
        .set_bold()
        .set_font_size(18)
        .set_font_color(Color::RGB(0x17365D));
    let heading = Format::new().set_bold().set_font_size(12);
    let wrap = Format::new().set_text_wrap().set_align(FormatAlign::Top);
    let worksheet = workbook.add_worksheet();
    worksheet.set_name(INSTRUCTIONS_SHEET)?;
    worksheet.set_column_width(0, 24)?;
    worksheet.set_column_width(1, 100)?;
    worksheet.write_string_with_format(0, 0, "Trox translator handoff", &title)?;
    worksheet.write_string(2, 0, "Target locale")?;
    worksheet.write_string(2, 1, locale)?;
    worksheet.write_string(3, 0, "Source locale")?;
    worksheet.write_string(3, 1, source_locale)?;
    worksheet.write_string(4, 0, "Active rows")?;
    worksheet.write_string(4, 1, row_count.to_string())?;
    worksheet.write_string_with_format(6, 0, "What to edit", &heading)?;
    worksheet.write_string_with_format(
        6,
        1,
        "Edit only the Translation and Translator note columns on the Translations sheet. Locked and hidden cells are validated when the workbook is imported.",
        &wrap,
    )?;
    worksheet.write_string_with_format(8, 0, "Placeholders", &heading)?;
    worksheet.write_string_with_format(
        8,
        1,
        "Preserve every {placeholder} listed for a row. Reordering placeholders is allowed; Trox validates removed, renamed, or invented placeholders against the project's lint policy during import.",
        &wrap,
    )?;
    worksheet.write_string_with_format(10, 0, "Return format", &heading)?;
    worksheet.write_string_with_format(
        10,
        1,
        "Return this same .xlsx workbook. Do not add formulas, rows, or columns. Trox imports translations only after verifying the catalog fingerprint and every protected source field.",
        &wrap,
    )?;
    worksheet.protect();
    Ok(())
}

fn write_translations(
    workbook: &mut Workbook,
    rows: &[&CsvRow],
    edits: Option<&BTreeMap<String, crate::csv_workflow::TranslatorEdit>>,
) -> std::result::Result<(), XlsxError> {
    let header = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x17365D))
        .set_text_wrap()
        .set_align(FormatAlign::Top);
    let locked = Format::new()
        .set_num_format("@")
        .set_text_wrap()
        .set_align(FormatAlign::Top);
    let editable = Format::new()
        .set_num_format("@")
        .set_unlocked()
        .set_background_color(Color::RGB(0xFFF2CC))
        .set_text_wrap()
        .set_align(FormatAlign::Top);
    let worksheet = workbook.add_worksheet();
    worksheet.set_name(TRANSLATIONS_SHEET)?.set_active(true);
    for (column, value) in HEADERS.iter().enumerate() {
        worksheet.write_string_with_format(0, column as u16, *value, &header)?;
    }
    worksheet.set_column_width(0, 42)?;
    worksheet.set_column_width(1, 58)?;
    worksheet.set_column_width(2, 42)?;
    worksheet.set_column_width(3, 32)?;
    worksheet.set_column_width(4, 12)?;
    worksheet.set_column_width(5, 42)?;
    worksheet.set_column_width(6, 24)?;
    worksheet.set_column_format(2, &editable)?;
    worksheet.set_column_format(3, &editable)?;
    for column in 7..HEADERS.len() as u16 {
        worksheet.set_column_hidden(column)?;
    }
    for (index, row) in rows.iter().enumerate() {
        let output_row = index as u32 + 1;
        let edit = edits.and_then(|edits| edits.get(&row.row_id));
        let source_row_fingerprint = source_row_fingerprint(row)
            .map_err(|error| XlsxError::ParameterError(error.to_string()))?;
        let values = [
            row.english.as_str(),
            row.description.as_str(),
            edit.map_or(row.translation.as_str(), |edit| edit.translation.as_str()),
            edit.map_or(row.translator_note.as_str(), |edit| {
                edit.translator_note.as_str()
            }),
            row.status.as_str(),
            row.previous_translation.as_str(),
            row.placeholders.as_str(),
            row.entry_id.as_str(),
            row.row_id.as_str(),
            row.kind.as_str(),
            row.source_locations.as_str(),
            row.source_revision.as_str(),
            source_row_fingerprint.as_str(),
        ];
        for (column, value) in values.iter().enumerate() {
            let format = if matches!(column, 2 | 3) {
                &editable
            } else {
                &locked
            };
            worksheet.write_string_with_format(output_row, column as u16, *value, format)?;
        }
    }
    worksheet.set_freeze_panes(1, 0)?;
    worksheet.autofilter(0, 0, rows.len() as u32, 6)?;
    worksheet.protect_with_options(&ProtectionOptions {
        sort: true,
        use_autofilter: true,
        ..ProtectionOptions::default()
    });
    Ok(())
}

fn write_manifest(
    workbook: &mut Workbook,
    locale: &str,
    source_locale: &str,
    source_catalog_fingerprint: &str,
    row_count: usize,
    package_fingerprint: &str,
) -> std::result::Result<(), XlsxError> {
    let worksheet = workbook.add_worksheet();
    worksheet.set_name(MANIFEST_SHEET)?;
    for (row, (key, value)) in [
        ("format", FORMAT.to_owned()),
        ("version", VERSION.to_owned()),
        ("locale", locale.to_owned()),
        ("source_locale", source_locale.to_owned()),
        (
            "source_catalog_fingerprint",
            source_catalog_fingerprint.to_owned(),
        ),
        ("active_row_count", row_count.to_string()),
        ("package_fingerprint", package_fingerprint.to_owned()),
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string(row as u32, 0, key)?;
        worksheet.write_string(row as u32, 1, value)?;
    }
    worksheet.protect().set_hidden(true);
    Ok(())
}

pub fn import_workbook(
    path: &Path,
    locale: &str,
    source_locale: &str,
    source_catalog_fingerprint: &str,
    document: &CsvDocument,
) -> Result<BTreeMap<String, crate::csv_workflow::TranslatorEdit>> {
    let mut workbook: Xlsx<_> = open_workbook(path)
        .with_context(|| format!("failed to open translator workbook {}", path.display()))?;
    reject_formulas(&mut workbook)?;
    let manifest = read_manifest(&mut workbook)?;
    let expected_snapshot = snapshot(locale, source_locale, source_catalog_fingerprint, document);
    let expected_fingerprint = snapshot_fingerprint(&expected_snapshot)?;
    expect_manifest(&manifest, "format", FORMAT)?;
    expect_manifest(&manifest, "version", VERSION)?;
    expect_manifest(&manifest, "locale", locale)?;
    expect_manifest(&manifest, "source_locale", source_locale)?;
    expect_manifest(
        &manifest,
        "source_catalog_fingerprint",
        source_catalog_fingerprint,
    )?;
    expect_manifest(
        &manifest,
        "active_row_count",
        &expected_snapshot.rows.len().to_string(),
    )?;
    expect_manifest(&manifest, "package_fingerprint", &expected_fingerprint)?;

    let range = workbook
        .worksheet_range(TRANSLATIONS_SHEET)
        .context("translator workbook is missing the Translations sheet")?;
    validate_headers(&range)?;
    let current: BTreeMap<_, _> = active_rows(document)
        .into_iter()
        .map(|row| (row.row_id.as_str(), row))
        .collect();
    if range.height().saturating_sub(1) != current.len() {
        bail!(
            "translator workbook row count changed: expected {}, got {}",
            current.len(),
            range.height().saturating_sub(1)
        );
    }
    let mut seen = BTreeSet::new();
    let mut edits = BTreeMap::new();
    for workbook_row in 1..range.height() {
        let row_id = text_cell(&range, workbook_row, 8, "row_id")?;
        if !seen.insert(row_id.clone()) {
            bail!("translator workbook repeats row ID `{row_id}`");
        }
        let expected = current
            .get(row_id.as_str())
            .with_context(|| format!("translator workbook contains unknown row ID `{row_id}`"))?;
        let expected_source_fingerprint = source_row_fingerprint(expected)?;
        for (column, label, value) in [
            (0, "English", expected.english.as_str()),
            (1, "Context", expected.description.as_str()),
            (4, "Status", expected.status.as_str()),
            (
                5,
                "Previous translation",
                expected.previous_translation.as_str(),
            ),
            (6, "Placeholders", expected.placeholders.as_str()),
            (7, "entry_id", expected.entry_id.as_str()),
            (8, "row_id", expected.row_id.as_str()),
            (9, "kind", expected.kind.as_str()),
            (10, "source_locations", expected.source_locations.as_str()),
            (11, "source_revision", expected.source_revision.as_str()),
            (
                12,
                "source_row_fingerprint",
                expected_source_fingerprint.as_str(),
            ),
        ] {
            let actual = text_cell(&range, workbook_row, column, label)?;
            if actual != value {
                bail!("translator workbook changed protected `{label}` for row `{row_id}`");
            }
        }
        edits.insert(
            row_id,
            crate::csv_workflow::TranslatorEdit {
                translation: text_cell(&range, workbook_row, 2, "Translation")?,
                translator_note: text_cell(&range, workbook_row, 3, "Translator note")?,
            },
        );
    }
    Ok(edits)
}

fn reject_formulas(workbook: &mut Xlsx<std::io::BufReader<std::fs::File>>) -> Result<()> {
    for sheet in workbook.sheet_names() {
        let formulas = workbook
            .worksheet_formula(&sheet)
            .with_context(|| format!("failed to inspect formulas on sheet `{sheet}`"))?;
        if formulas.rows().flatten().any(|formula| !formula.is_empty()) {
            bail!("translator workbook contains a formula on sheet `{sheet}`");
        }
    }
    Ok(())
}

fn read_manifest(
    workbook: &mut Xlsx<std::io::BufReader<std::fs::File>>,
) -> Result<BTreeMap<String, String>> {
    let range = workbook
        .worksheet_range(MANIFEST_SHEET)
        .context("translator workbook is missing its manifest")?;
    let mut manifest = BTreeMap::new();
    for row in 0..range.height() {
        let key = text_cell(&range, row, 0, "manifest key")?;
        let value = text_cell(&range, row, 1, "manifest value")?;
        if key.is_empty() || manifest.insert(key.clone(), value).is_some() {
            bail!("translator workbook has an invalid manifest key `{key}`");
        }
    }
    Ok(manifest)
}

fn expect_manifest(manifest: &BTreeMap<String, String>, key: &str, expected: &str) -> Result<()> {
    let actual = manifest
        .get(key)
        .with_context(|| format!("translator workbook manifest is missing `{key}`"))?;
    if actual != expected {
        bail!("translator workbook `{key}` does not match the current project");
    }
    Ok(())
}

fn validate_headers(range: &Range<Data>) -> Result<()> {
    for (column, expected) in HEADERS.iter().enumerate() {
        let actual = text_cell(range, 0, column, "header")?;
        if actual != *expected {
            bail!(
                "translator workbook column {} must be `{expected}`",
                column + 1
            );
        }
    }
    if range.width() != HEADERS.len() {
        bail!(
            "translator workbook column count changed: expected {}, got {}",
            HEADERS.len(),
            range.width()
        );
    }
    Ok(())
}

fn text_cell(range: &Range<Data>, row: usize, column: usize, label: &str) -> Result<String> {
    match range.get((row, column)).unwrap_or(&Data::Empty) {
        Data::Empty => Ok(String::new()),
        Data::String(value) => Ok(value.clone()),
        _ => bail!("translator workbook `{label}` cell must contain text"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::Reader;
    use tempfile::tempdir;

    fn row(row_id: &str, status: &str, english: &str, translation: &str) -> CsvRow {
        let mut row = CsvRow::default();
        row.english = english.into();
        row.description = "A useful translator context sentence.".into();
        row.translation = translation.into();
        row.status = status.into();
        row.translator_note = "Keep concise.".into();
        row.placeholders = "count".into();
        row.entry_id = "tx1_entry".into();
        row.row_id = row_id.into();
        row.kind = "message".into();
        row.source_locations = "src/game.rs:10:5".into();
        row.source_revision = "rev1_source".into();
        row
    }

    #[test]
    fn export_contains_only_active_rows_and_writes_formula_prefixes_as_text() {
        let document = CsvDocument {
            extra_headers: vec!["reviewer".into()],
            rows: vec![
                row(
                    "row_active",
                    "translated",
                    "+{count} cards",
                    "+{count} cartas",
                ),
                row("row_old", "obsolete", "Old", "Antiguo"),
            ],
        };
        let bytes = export_workbook("es", "en-US", "catalog", &document).unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("es.xlsx");
        write_workbook(&path, &bytes).unwrap();

        let mut workbook: Xlsx<_> = open_workbook(&path).unwrap();
        let translations = workbook.worksheet_range(TRANSLATIONS_SHEET).unwrap();
        assert_eq!(translations.height(), 2);
        assert_eq!(
            translations.get((1, 0)),
            Some(&Data::String("+{count} cards".into()))
        );
        assert_eq!(
            translations.get((1, 2)),
            Some(&Data::String("+{count} cartas".into()))
        );
        assert!(
            workbook
                .worksheet_formula(TRANSLATIONS_SHEET)
                .unwrap()
                .rows()
                .flatten()
                .all(String::is_empty)
        );

        let edits = import_workbook(&path, "es", "en-US", "catalog", &document).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits["row_active"].translation, "+{count} cartas");
    }

    #[test]
    fn import_returns_translator_changes_without_weakening_the_snapshot() {
        let document = CsvDocument {
            extra_headers: vec![],
            rows: vec![row("row_active", "missing", "{count} cards", "")],
        };
        let returned = BTreeMap::from([(
            "row_active".into(),
            crate::csv_workflow::TranslatorEdit {
                translation: "{count} cartas".into(),
                translator_note: "Natural plural.".into(),
            },
        )]);
        let bytes =
            export_workbook_with_edits("es", "en-US", "catalog", &document, Some(&returned))
                .unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("es.xlsx");
        write_workbook(&path, &bytes).unwrap();

        let edits = import_workbook(&path, "es", "en-US", "catalog", &document).unwrap();
        assert_eq!(edits, returned);
    }

    #[test]
    fn import_rejects_canonical_csv_changes_after_export() {
        let mut document = CsvDocument {
            extra_headers: vec![],
            rows: vec![row(
                "row_active",
                "translated",
                "{count} cards",
                "{count} cartas",
            )],
        };
        let bytes = export_workbook("es", "en-US", "catalog", &document).unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("es.xlsx");
        write_workbook(&path, &bytes).unwrap();
        document.rows[0].translator_note = "A newer local review note.".into();

        let error = import_workbook(&path, "es", "en-US", "catalog", &document).unwrap_err();
        assert!(error.to_string().contains("package_fingerprint"));
    }

    #[test]
    fn import_rejects_formula_cells() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("formula.xlsx");
        let mut workbook = Workbook::new();
        workbook
            .add_worksheet()
            .set_name(TRANSLATIONS_SHEET)
            .unwrap()
            .write_formula(0, 0, "=1+1")
            .unwrap();
        workbook.save(&path).unwrap();
        let document = CsvDocument {
            extra_headers: vec![],
            rows: vec![row("row_active", "missing", "Cards", "")],
        };

        let error = import_workbook(&path, "es", "en-US", "catalog", &document).unwrap_err();
        assert!(error.to_string().contains("contains a formula"));
    }
}
