use std::fs;

use assert_cmd::Command;
use calamine::{Reader, Xlsx, open_workbook};
use predicates::prelude::PredicateBooleanExt;
use tempfile::tempdir;
use trox::prelude::*;

fn copy_fixture() -> tempfile::TempDir {
    let target = tempdir().unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project");
    copy_dir(&source, target.path());
    target
}

fn copy_dir(source: &std::path::Path, target: &std::path::Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let output = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &output);
        } else {
            fs::copy(entry.path(), output).unwrap();
        }
    }
}

#[test]
fn handoff_export_and_import_round_trip_the_current_active_catalog() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "extract"])
        .assert()
        .success();
    let csv_path = fixture.path().join("locales/es.csv");
    let before = fs::read(&csv_path).unwrap();

    Command::cargo_bin("trox")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "handoff",
            "export",
            "--locale",
            "es",
            "--output",
            "handoff/es.xlsx",
        ])
        .assert()
        .success();

    let workbook_path = fixture.path().join("handoff/es.xlsx");
    let mut workbook: Xlsx<_> = open_workbook(&workbook_path).unwrap();
    let translations = workbook.worksheet_range("Translations").unwrap();
    let csv_rows = csv::Reader::from_reader(before.as_slice())
        .records()
        .count();
    assert_eq!(translations.height().saturating_sub(1), csv_rows);

    Command::cargo_bin("trox")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "handoff",
            "import",
            "--locale",
            "es",
            "--input",
            "handoff/es.xlsx",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(csv_path).unwrap(), before);
}

#[test]
fn extract_is_deterministic_and_bundle_allow_missing_is_loadable() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "extract"])
        .assert()
        .success();
    let es_path = fixture.path().join("locales/es.csv");
    let first = fs::read(&es_path).unwrap();
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "extract"])
        .assert()
        .success();
    assert_eq!(first, fs::read(&es_path).unwrap());
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "check"])
        .assert()
        .success();
    Command::cargo_bin("trox")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "bundle",
            "--allow-missing",
        ])
        .assert()
        .success();
    let source_json = fs::read_to_string(fixture.path().join("out/en-US.trox.json")).unwrap();
    let target_json = fs::read_to_string(fixture.path().join("out/es.trox.json")).unwrap();
    let source = trox::Bundle::from_canonical_json(&source_json).unwrap();
    let target = trox::Bundle::from_canonical_json(&target_json).unwrap();
    let fallback_localizer = trox::Localizer::new(target, source).unwrap();
    let close = tx("Close deck browser", "Any runtime-only description.");
    assert_eq!(fallback_localizer.resolve(&close), "Close deck browser");

    translate_all(&es_path);
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "bundle"])
        .assert()
        .success();
    let source = trox::Bundle::from_canonical_json(
        &fs::read_to_string(fixture.path().join("out/en-US.trox.json")).unwrap(),
    )
    .unwrap();
    let target = trox::Bundle::from_canonical_json(
        &fs::read_to_string(fixture.path().join("out/es.trox.json")).unwrap(),
    )
    .unwrap();
    let mut unsupported_cldr = target.clone();
    unsupported_cldr.cldr_version = "49".into();
    assert!(unsupported_cldr.validate().is_err());
    let mut invalid_source_chain = source.clone();
    invalid_source_chain.fallback_chain.push("en-US".into());
    assert!(invalid_source_chain.validate().is_err());
    let mut target_with_identity = target.clone();
    target_with_identity
        .entries
        .get_mut(close.entry_id())
        .unwrap()
        .identity = source
        .entries
        .get(close.entry_id())
        .unwrap()
        .identity
        .clone();
    assert!(target_with_identity.validate().is_err());
    let mut corrupt_expansion = target.clone();
    let row = corrupt_expansion
        .entries
        .values_mut()
        .find_map(|entry| entry.rows.values_mut().next())
        .unwrap();
    row.expansion
        .path
        .push(serde_json::json!({"kind":"select","branch":0}));
    assert!(corrupt_expansion.validate().is_err());
    let localizer = trox::Localizer::new_strict(target, source).unwrap();
    assert_eq!(
        localizer.resolve_checked(&close).unwrap(),
        "[es] Close deck browser"
    );
    let card_count = 3_u32;
    let remaining = txa(
        plural(
            card_count,
            [
                exact(0_u32, "No cards remain."),
                one("{card_count} card remains."),
                other("{card_count} cards remain."),
            ],
        ),
        tx_args![card_count],
        "Runtime description does not affect identity.",
    );
    assert_eq!(
        localizer.resolve_checked(&remaining).unwrap(),
        "[es] \u{2068}3\u{2069} cards remain."
    );
    let creature_noun = counted(TermId::new("card-subtype.warrior"), card_count);
    let creation = txa(
        "Create {card_count} {creature_noun}.",
        tx_args![card_count, creature_noun],
        "Runtime-selected creature noun.",
    );
    assert_eq!(
        localizer.resolve_checked(&creation).unwrap(),
        "[es] Create \u{2068}3\u{2069} \u{2068}[es] Warriors\u{2069}."
    );
    let ron_template: LocalizedString = ron::from_str(
        r#"Tx(text:"Deck: {deck_name} ({count})",placeholders:{"deck_name":Opaque,"count":Scalar})"#,
    )
    .unwrap();
    let deck_summary = ron_template
        .bind_ron_template(tx_args![
            deck_name => opaque(tx("Radiant Echo", "Deck name.")),
            count => 40_u32,
        ])
        .unwrap();
    assert_eq!(
        localizer.resolve_checked(&deck_summary).unwrap(),
        "[es] Deck: \u{2068}[es] Radiant Echo\u{2069} (\u{2068}40\u{2069})"
    );
    let unknown_noun = counted(TermId::new("card-subtype.unknown"), card_count);
    let recovery = txa(
        "Create {card_count} {creature_noun}.",
        tx_args![card_count, creature_noun => unknown_noun],
        "Dynamic terms are checked again during resolution.",
    );
    assert_eq!(
        localizer.resolve(&recovery),
        "Create \u{2068}3\u{2069} \u{2068}⟦term:card-subtype.unknown⟧\u{2069}."
    );
    let wire = close.to_canonical_json().unwrap();
    assert_eq!(localizer.localized_string_from_json(&wire).unwrap(), close);
    let tampered = wire.replacen(close.entry_id(), "tx1_aaaaaaaaaaaaaaaaaaaaaaaaaa", 1);
    assert!(localizer.localized_string_from_json(&tampered).is_err());
    let mut tampered_signature: serde_json::Value = serde_json::from_str(&wire).unwrap();
    tampered_signature["source_signature"] = serde_json::Value::String("0".repeat(64));
    let tampered_signature = serde_json::to_string(&tampered_signature).unwrap();
    assert!(
        localizer
            .localized_string_from_json(&tampered_signature)
            .is_err()
    );
}

#[test]
fn locale_init_atomically_scaffolds_profile_and_target_csv() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    let profile = fixture.path().join("locales/es.ron");
    fs::remove_file(&profile).unwrap();
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "locale", "init", "es"])
        .assert()
        .success();
    let profile_text = fs::read_to_string(profile).unwrap();
    assert!(profile_text.contains("Advisory:"));
    let csv = fs::read_to_string(fixture.path().join("locales/es.csv")).unwrap();
    assert!(csv.starts_with("english,description,translation,status,"));
    assert!(csv.lines().count() > 1);
}

#[test]
fn numbered_default_alias_resolves_the_translated_target_default() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    let profile = fixture.path().join("locales/es.ron");
    let profile_text = fs::read_to_string(&profile).unwrap().replace(
        "    term_facets: {",
        "    term_form_aliases: {\n        \"card-subtype.warrior\": { \"counted\": Default },\n    },\n    term_facets: {",
    );
    fs::write(profile, profile_text).unwrap();
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "extract"])
        .assert()
        .success();
    let csv = fixture.path().join("locales/es.csv");
    translate_all(&csv);
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "bundle"])
        .assert()
        .success();

    let source = trox::Bundle::from_canonical_json(
        &fs::read_to_string(fixture.path().join("out/en-US.trox.json")).unwrap(),
    )
    .unwrap();
    let target = trox::Bundle::from_canonical_json(
        &fs::read_to_string(fixture.path().join("out/es.trox.json")).unwrap(),
    )
    .unwrap();
    let localizer = trox::Localizer::new_strict(target, source).unwrap();
    let card_count = 3_u32;
    let creature_noun = counted(TermId::new("card-subtype.warrior"), card_count);
    let message = txa(
        "Create {card_count} {creature_noun}.",
        tx_args![card_count, creature_noun],
        "Numbered locale aliases preserve the number contract.",
    );
    assert_eq!(
        localizer.resolve_checked(&message).unwrap(),
        "[es] Create \u{2068}3\u{2069} \u{2068}[es] Warrior\u{2069}."
    );
}

#[test]
fn check_rejects_an_unconfigured_fallback_parent() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    let profile = fixture.path().join("locales/es.ron");
    let input = fs::read_to_string(&profile).unwrap();
    fs::write(
        profile,
        input.replace("fallbacks: [\"en-US\"]", "fallbacks: [\"fr\", \"en-US\"]"),
    )
    .unwrap();

    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "check"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "references unconfigured fallback locale `fr`",
        ));
}

#[test]
fn check_runs_fallback_profile_compatibility() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    let config_text = fs::read_to_string(&config).unwrap().replace(
        "locales: {\n        \"es\": (profile: \"locales/es.ron\", csv: \"locales/es.csv\", bundle: \"out/es.trox.json\"),",
        "locales: {\n        \"es\": (profile: \"locales/es.ron\", csv: \"locales/es.csv\", bundle: \"out/es.trox.json\"),\n        \"fr\": (profile: \"locales/fr.ron\", csv: \"locales/fr.csv\", bundle: \"out/fr.trox.json\"),",
    );
    assert!(config_text.contains("locales/fr.ron"));
    fs::write(&config, config_text).unwrap();
    let es_profile = fixture.path().join("locales/es.ron");
    let es_text = fs::read_to_string(&es_profile)
        .unwrap()
        .replace("fallbacks: [\"en-US\"]", "fallbacks: [\"fr\", \"en-US\"]");
    fs::write(es_profile, es_text).unwrap();
    fs::write(
        fixture.path().join("locales/fr.ron"),
        r#"(
            locale: "fr",
            direction: Ltr,
            isolation: Isolate,
            fallbacks: ["en-US"],
            facets: {
                "gender": (scope: Message, values: ["masculine"]),
            },
            term_facets: {
                "card-subtype.warrior": { "gender": "masculine" },
                "card-subtype.relic": { "gender": "masculine" },
            },
        )"#,
    )
    .unwrap();

    Command::cargo_bin("trox")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "check",
            "--locale",
            "es",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "parent locale `fr` has incompatible message facet `gender`",
        ));
}

#[test]
fn configured_benchmark_applies_the_regression_baseline() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    Command::cargo_bin("trox")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "benchmark",
            "--iterations",
            "1",
            "--baseline-mib-s",
            "1000000000",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("regressed more than 15%"));
}

#[test]
fn json_diagnostics_classify_config_and_csv_failures_at_their_source() {
    let fixture = copy_fixture();
    let config = fixture.path().join("trox.ron");
    let invalid_config = fixture.path().join("invalid.ron");
    fs::write(&invalid_config, "(unknown: true)").unwrap();
    Command::cargo_bin("trox")
        .unwrap()
        .args([
            "--json",
            "--config",
            invalid_config.to_str().unwrap(),
            "check",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("\"rule_id\":\"trox.invalid-config\"")
                .and(predicates::str::contains("\"path\":")),
        );

    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "extract"])
        .assert()
        .success();
    let csv = fixture.path().join("locales/es.csv");
    fs::write(&csv, "translation\n").unwrap();
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--json", "--config", config.to_str().unwrap(), "check"])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("\"rule_id\":\"trox.invalid-csv\"")
                .and(predicates::str::contains(csv.to_str().unwrap()))
                .and(predicates::str::contains("trox.command").not()),
        );
}

fn translate_all(path: &std::path::Path) {
    let input = fs::read(path).unwrap();
    let mut reader = csv::Reader::from_reader(input.as_slice());
    let headers = reader.headers().unwrap().clone();
    let english = headers
        .iter()
        .position(|header| header == "english")
        .unwrap();
    let translation = headers
        .iter()
        .position(|header| header == "translation")
        .unwrap();
    let status = headers
        .iter()
        .position(|header| header == "status")
        .unwrap();
    let records: Vec<_> = reader.records().map(Result::unwrap).collect();
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(vec![]);
    writer.write_record(&headers).unwrap();
    for record in records {
        let mut fields: Vec<_> = record.iter().map(str::to_owned).collect();
        fields[translation] = format!("[es] {}", fields[english]);
        fields[status] = "translated".into();
        writer.write_record(fields).unwrap();
    }
    writer.flush().unwrap();
    fs::write(path, writer.into_inner().unwrap()).unwrap();
}
