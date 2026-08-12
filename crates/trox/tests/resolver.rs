use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use trox::prelude::*;
use trox::{
    Argument, ArgumentSchema, Bundle, BundleEntry, BundleRow, BundleTerm, BundleTermForm,
    BundleTermSurface, DeserializeError, DiagnosticCode, ExpansionDescriptor, IsolationPolicy,
    NumberFormat, PluralCategory, PluralRules, TextDirection, Version, expansion_row_id,
};

fn plural_rules() -> PluralRules {
    PluralRules {
        cardinal: BTreeMap::from([
            (PluralCategory::One, "i = 1 and v = 0".to_owned()),
            (PluralCategory::Other, String::new()),
        ]),
        ordinal: BTreeMap::from([(PluralCategory::Other, String::new())]),
    }
}

fn bundle(locale: &str) -> Bundle {
    let source_locale = "en-US";
    Bundle {
        cldr_version: "48".to_owned(),
        direction: TextDirection::Ltr,
        entries: BTreeMap::new(),
        fallback_chain: if locale == source_locale {
            vec![]
        } else {
            vec![source_locale.to_owned()]
        },
        fallbacks_flattened: true,
        format: "trox-bundle".to_owned(),
        isolation: IsolationPolicy::None,
        locale: locale.to_owned(),
        message_facets: vec![],
        number_format: NumberFormat::default(),
        plural_rules: plural_rules(),
        source_catalog_fingerprint: "0".repeat(64),
        source_locale: source_locale.to_owned(),
        terms: BTreeMap::new(),
        version: Version::V1,
    }
}

fn source_bundle(values: &[&LocalizedString]) -> Bundle {
    let mut source = bundle("en-US");
    for value in values {
        source.entries.insert(
            value.entry_id().to_owned(),
            BundleEntry {
                arguments: Some(argument_schemas(value)),
                identity: Some(value.identity().clone()),
                rows: BTreeMap::new(),
                source_signature: value.source_signature().to_owned(),
            },
        );
    }
    source
}

fn argument_schemas(value: &LocalizedString) -> BTreeMap<String, ArgumentSchema> {
    value
        .arguments()
        .iter()
        .map(|(name, argument)| {
            let schema = match argument {
                Argument::Text { .. } | Argument::Number { .. } | Argument::Boolean { .. } => {
                    ArgumentSchema::Scalar
                }
                Argument::Term { form, number, .. } => ArgumentSchema::Term {
                    form: form.clone(),
                    number: number.is_some(),
                },
                Argument::Opaque { .. } => ArgumentSchema::Opaque,
            };
            (name.clone(), schema)
        })
        .collect()
}

fn translated_entry(value: &LocalizedString, translation: &str) -> BundleEntry {
    let expansion = ExpansionDescriptor {
        entry_signature: value.source_signature().to_owned(),
        path: vec![],
    };
    let row_id = expansion_row_id(&expansion).expect("valid empty expansion");
    BundleEntry {
        arguments: None,
        identity: None,
        rows: BTreeMap::from([(
            row_id,
            BundleRow {
                expansion,
                origin_locale: "es".to_owned(),
                translation: translation.to_owned(),
            },
        )]),
        source_signature: value.source_signature().to_owned(),
    }
}

fn scalar_term(text: &str) -> BundleTermForm {
    BundleTermForm::Scalar {
        origin_locale: "en-US".to_owned(),
        text: text.to_owned(),
    }
}

#[test]
fn assert_localized_resolves_raw_text_without_catalog_entries() {
    let value = assert_localized("Runtime {name} / } / e\u{301}");
    assert!(value.is_atomic());

    let source = bundle("en-US");
    let localizer = Localizer::new(bundle("es"), source).unwrap();
    assert_eq!(
        localizer.resolve_checked(&value).unwrap(),
        "Runtime {name} / } / é"
    );
    assert_eq!(localizer.resolve(&value), "Runtime {name} / } / é");
    assert_eq!(
        localizer.resolve_outcome(&value),
        trox::ResolveOutcome {
            text: "Runtime {name} / } / é".to_owned(),
            used_source_fallback: false,
        }
    );

    let decoded = localizer
        .localized_string_from_json(&value.to_canonical_json().unwrap())
        .unwrap();
    assert_eq!(decoded, value);
}

#[test]
fn resolves_a_compatible_target_row() {
    let name = "Ada".to_owned();
    let value = txa(
        "Hello {name}.",
        tx_args![name],
        "Greeting containing a person's name.",
    );
    let source = source_bundle(&[&value]);
    let mut target = bundle("es");
    target.entries.insert(
        value.entry_id().to_owned(),
        translated_entry(&value, "Hola {name}."),
    );

    let localizer = Localizer::new_strict(target, source).unwrap();
    assert_eq!(localizer.resolve_checked(&value).unwrap(), "Hola Ada.");
    assert_eq!(
        localizer.resolve_outcome(&value),
        trox::ResolveOutcome {
            text: "Hola Ada.".to_owned(),
            used_source_fallback: false,
        }
    );
}

#[test]
fn missing_target_message_falls_back_to_the_source_pattern() {
    let name = "Ada".to_owned();
    let value = txa(
        "Hello {name}.",
        tx_args![name],
        "Greeting containing a person's name.",
    );
    let source = source_bundle(&[&value]);
    let target = bundle("es");

    let localizer = Localizer::new(target, source).unwrap();
    let outcome = localizer.resolve_outcome(&value);
    assert_eq!(outcome.text, "Hello Ada.");
    assert!(outcome.used_source_fallback);
}

#[test]
fn missing_target_term_form_recovers_inside_the_target_message() {
    let noun_id = TermId::new("card-subtype.event");
    let noun = term(noun_id.clone()).form("indefinite").finish();
    let value = txa(
        "Create {noun}.",
        tx_args![noun],
        "Action using a runtime-selected noun.",
    );
    let mut source = source_bundle(&[&value]);
    source.terms.insert(
        noun_id.as_str().to_owned(),
        BundleTerm {
            facets: BTreeMap::new(),
            forms: BTreeMap::from([
                ("$default".to_owned(), scalar_term("card")),
                ("indefinite".to_owned(), scalar_term("a card")),
            ]),
        },
    );
    let mut target = bundle("es");
    target.entries.insert(
        value.entry_id().to_owned(),
        translated_entry(&value, "Crear {noun}."),
    );
    target.terms.insert(
        noun_id.as_str().to_owned(),
        BundleTerm {
            facets: BTreeMap::new(),
            forms: BTreeMap::new(),
        },
    );

    let localizer = Localizer::new(target, source).unwrap();
    assert!(matches!(
        localizer.resolve_checked(&value),
        Err(trox::ResolveError::MissingTermForm { .. })
    ));
    let outcome = localizer.resolve_outcome(&value);
    assert_eq!(outcome.text, "Crear card.");
    assert!(!outcome.used_source_fallback);
}

#[test]
fn missing_nested_target_row_recovers_inside_the_target_message() {
    let card_name = tx("Radiant Echo", "Proper name of a game card.");
    let outer = txa(
        "Source name: {card_name}.",
        tx_args![card_name => opaque(card_name.clone())],
        "Label containing an opaque localized proper name.",
    );
    let source = source_bundle(&[&outer, &card_name]);
    let mut target = bundle("es");
    target.entries.insert(
        outer.entry_id().to_owned(),
        translated_entry(&outer, "Nombre: {card_name}."),
    );

    let localizer = Localizer::new(target, source).unwrap();
    assert!(matches!(
        localizer.resolve_checked(&outer),
        Err(trox::ResolveError::MissingMessage { .. })
    ));
    let outcome = localizer.resolve_outcome(&outer);
    assert_eq!(outcome.text, "Nombre: Radiant Echo.");
    assert!(!outcome.used_source_fallback);
}

#[test]
fn catalog_mismatch_is_diagnostic_normally_and_an_error_in_strict_mode() {
    let value = tx("Close", "Close button label.");
    let source = source_bundle(&[&value]);
    let mut target = bundle("es");
    target.source_catalog_fingerprint = "1".repeat(64);
    target.entries.insert(
        value.entry_id().to_owned(),
        translated_entry(&value, "Cerrar"),
    );

    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&diagnostics);
    let localizer = Localizer::new(target.clone(), source.clone())
        .unwrap()
        .with_diagnostic_hook(move |diagnostic| captured.lock().unwrap().push(diagnostic));
    assert_eq!(localizer.resolve(&value), "Cerrar");
    assert_eq!(
        diagnostics.lock().unwrap()[0].code,
        DiagnosticCode::CatalogMismatch
    );

    assert!(matches!(
        Localizer::new_strict(target, source),
        Err(DeserializeError::InvalidBundle(message))
            if message.contains("fingerprint mismatch")
    ));
}

#[test]
fn malformed_bundle_json_is_rejected() {
    let source = bundle("en-US");
    let mut value = serde_json::to_value(source).unwrap();
    value["unexpected"] = serde_json::json!(true);
    let canonical = serde_json_canonicalizer::to_string(&value).unwrap();
    assert!(Bundle::from_canonical_json(&canonical).is_err());
}

#[test]
fn bundle_validation_enforces_source_only_argument_schemas() {
    let name = "Ada".to_owned();
    let value = txa("Hello {name}", tx_args![name], "Greeting with a name.");

    let mut missing = source_bundle(&[&value]);
    missing.entries.get_mut(value.entry_id()).unwrap().arguments = None;
    assert!(matches!(
        missing.validate(),
        Err(DeserializeError::InvalidBundle(message)) if message.contains("requires arguments")
    ));

    let mut mismatched = source_bundle(&[&value]);
    mismatched
        .entries
        .get_mut(value.entry_id())
        .unwrap()
        .arguments = Some(BTreeMap::from([(
        "different_name".to_owned(),
        ArgumentSchema::Scalar,
    )]));
    assert!(matches!(
        mismatched.validate(),
        Err(DeserializeError::InvalidBundle(message))
            if message.contains("argument schemas differ")
    ));

    let mut target = bundle("es");
    let mut entry = translated_entry(&value, "Hola {name}");
    entry.arguments = Some(argument_schemas(&value));
    target.entries.insert(value.entry_id().to_owned(), entry);
    assert!(matches!(
        target.validate(),
        Err(DeserializeError::InvalidBundle(message))
            if message.contains("must not contain arguments")
    ));
}

#[test]
fn target_entry_ids_require_canonical_short_id_encoding() {
    for entry_id in [
        format!("tx1_{}", "!".repeat(26)),
        format!("tx1_{}b", "a".repeat(25)),
    ] {
        let mut target = bundle("es");
        target.entries.insert(
            entry_id,
            BundleEntry {
                arguments: None,
                identity: None,
                rows: BTreeMap::new(),
                source_signature: "0".repeat(64),
            },
        );

        assert!(target.validate().is_err());
        assert!(Localizer::new(target, bundle("en-US")).is_err());
    }
}

#[test]
fn locale_validation_rejects_duplicate_or_out_of_order_script_subtags() {
    let mut source = bundle("en-US");
    source.locale = "en-Latn-Cyrl".to_owned();
    source.source_locale = source.locale.clone();
    assert!(source.validate().is_err());
}

#[test]
fn minimum_grouping_digits_does_not_suppress_later_groups() {
    let value = |raw| {
        let number = trox::TroxNumber::new(raw).unwrap();
        txa("{number}", tx_args![number], "A locale-formatted number.")
    };
    let below_threshold = value(9_999_f64);
    let at_threshold = value(10_000_f64);
    let multiple_groups = value(1_234_567_f64);
    let source = source_bundle(&[&below_threshold]);
    let mut target = bundle("es");
    target.number_format.minimum_grouping_digits = 2;
    target.entries.insert(
        below_threshold.entry_id().to_owned(),
        translated_entry(&below_threshold, "{number}"),
    );

    let localizer = Localizer::new(target, source).unwrap();
    assert_eq!(localizer.resolve_checked(&below_threshold).unwrap(), "9999");
    assert_eq!(localizer.resolve_checked(&at_threshold).unwrap(), "10,000");
    assert_eq!(
        localizer.resolve_checked(&multiple_groups).unwrap(),
        "1,234,567"
    );
}

#[test]
fn source_catalog_authorizes_term_forms_and_number_policy() {
    let noun_id = TermId::new("card-subtype.event");
    let source_value = txa(
        "{noun}",
        tx_args![noun => term(noun_id.clone()).finish()],
        "A catalog-authorized term.",
    );
    let mut source = source_bundle(&[&source_value]);
    source.terms.insert(
        noun_id.as_str().to_owned(),
        BundleTerm {
            facets: BTreeMap::new(),
            forms: BTreeMap::from([
                ("$default".to_owned(), scalar_term("card")),
                (
                    "counted".to_owned(),
                    BundleTermForm::Number {
                        values: BTreeMap::from([(
                            PluralCategory::Other,
                            BundleTermSurface {
                                origin_locale: "en-US".to_owned(),
                                text: "cards".to_owned(),
                            },
                        )]),
                    },
                ),
            ]),
        },
    );
    let catalog = source.source_catalog().unwrap();

    let unauthorized_form = txa(
        "{noun}",
        tx_args![noun => term(noun_id.clone()).form("unknown").finish()],
        "A term requesting an unauthorized form.",
    );
    let number_on_scalar = txa(
        "{noun}",
        tx_args![noun => term(noun_id.clone()).number(2_u32)],
        "A scalar term incorrectly carrying a number.",
    );
    let missing_number = txa(
        "{noun}",
        tx_args![noun => term(noun_id).form("counted").finish()],
        "A numbered term missing its required number.",
    );

    for value in [unauthorized_form, number_on_scalar, missing_number] {
        assert!(
            matches!(
                catalog.localized_string_from_json(&value.to_canonical_json().unwrap()),
                Err(DeserializeError::Unauthorized(_))
            ),
            "catalog accepted unauthorized wire value: {}",
            value.to_canonical_json().unwrap()
        );
    }
}

#[test]
fn bundle_validation_rejects_empty_and_non_nfc_runtime_surfaces() {
    let value = tx("Hello", "Greeting.");

    let mut empty_row = bundle("es");
    empty_row
        .entries
        .insert(value.entry_id().to_owned(), translated_entry(&value, ""));
    assert!(matches!(
        empty_row.validate(),
        Err(DeserializeError::InvalidBundle(message)) if message.contains("empty translation")
    ));

    let mut non_nfc_row = bundle("es");
    non_nfc_row.entries.insert(
        value.entry_id().to_owned(),
        translated_entry(&value, "e\u{301}"),
    );
    assert!(matches!(
        non_nfc_row.validate(),
        Err(DeserializeError::InvalidBundle(message)) if message.contains("must be NFC")
    ));

    let mut empty_term = bundle("en-US");
    empty_term.terms.insert(
        "card".into(),
        BundleTerm {
            facets: BTreeMap::new(),
            forms: BTreeMap::from([("$default".into(), scalar_term(""))]),
        },
    );
    assert!(matches!(
        empty_term.validate(),
        Err(DeserializeError::InvalidBundle(message)) if message.contains("is empty")
    ));

    let mut non_nfc_symbol = bundle("es");
    non_nfc_symbol.number_format.group = "e\u{301}".into();
    assert!(matches!(
        non_nfc_symbol.validate(),
        Err(DeserializeError::InvalidBundle(message)) if message.contains("must be NFC")
    ));
}

#[test]
fn bundle_validation_rejects_more_than_4096_rows_before_row_validation() {
    let value = tx("Hello", "Greeting.");
    let mut entry = translated_entry(&value, "Hola");
    let row = entry.rows.values().next().unwrap().clone();
    entry.rows = (0..=4096)
        .map(|index| (format!("deliberately-invalid-row-{index:04}"), row.clone()))
        .collect();
    let mut target = bundle("es");
    target.entries.insert(value.entry_id().to_owned(), entry);

    assert!(matches!(
        target.validate(),
        Err(DeserializeError::InvalidBundle(message)) if message.contains("4,096-row runtime limit")
    ));
}

#[test]
fn source_catalog_rejects_non_nfc_wire_text_arguments() {
    let name = "Ada".to_owned();
    let value = txa("Hello {name}", tx_args![name], "Greeting with a name.");
    let source = source_bundle(&[&value]);
    let catalog = source.source_catalog().unwrap();
    let mut wire: serde_json::Value =
        serde_json::from_str(&value.to_canonical_json().unwrap()).unwrap();
    wire["arguments"]["name"]["value"] = serde_json::json!("e\u{301}");
    let wire = serde_json_canonicalizer::to_string(&wire).unwrap();

    assert!(matches!(
        catalog.localized_string_from_json(&wire),
        Err(DeserializeError::InvalidValue(message)) if message.contains("must be NFC")
    ));
}

#[test]
fn diagnostic_hook_panics_cannot_break_recovery_or_pending_delivery() {
    let value = tx("Source fallback", "Fallback text.");
    let source = source_bundle(&[&value]);
    let target = bundle("es");
    let localizer = Localizer::new(target, source)
        .unwrap()
        .with_diagnostic_hook(|_| panic!("diagnostic sink failed"));
    assert_eq!(localizer.resolve(&value), "Source fallback");

    let source = source_bundle(&[&value]);
    let mut target = bundle("es");
    target.source_catalog_fingerprint = "1".repeat(64);
    target.entries.insert(
        value.entry_id().to_owned(),
        translated_entry(&value, "Destino"),
    );
    let localizer = Localizer::new(target, source)
        .unwrap()
        .with_diagnostic_hook(|_| panic!("pending diagnostic sink failed"));
    assert_eq!(localizer.resolve(&value), "Destino");
}
