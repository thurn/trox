use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use trox::prelude::*;
use trox::{
    Argument, ArgumentSchema, Bundle, BundleEntry, BundleRow, BundleTerm, BundleTermForm,
    BundleTermSurface, ExpansionDescriptor, IsolationPolicy, NumberFormat, PluralCategory,
    PluralRules, ResolvedLocalizedPart, TextDirection, Version, expansion_row_id,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Annotation {
    kind: &'static str,
    id: &'static str,
}

#[test]
fn source_parts_preserve_literals_escaping_annotations_and_unannotated_placeholders() {
    let value = txa(
        "Gain {{shown}} {card} for {count} turns ({plain}).",
        tx_args![card => "Radiant Echo", count => 2_u32, plain => true],
        "Effect granting a displayed card.",
    );
    let annotated = value
        .clone()
        .annotate([(
            "card",
            Annotation {
                kind: "entity",
                id: "card-7",
            },
        )])
        .unwrap();
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&diagnostics);
    let localizer = Localizer::new(bundle("es", &[], false), source_bundle(&[&value]))
        .unwrap()
        .with_diagnostic_hook(move |diagnostic| {
            captured.lock().unwrap().push(diagnostic.code);
        });

    let outcome = localizer.resolve_parts_outcome(&annotated);
    assert!(outcome.used_source_fallback);
    assert_eq!(
        outcome.parts,
        vec![
            ResolvedLocalizedPart::Literal {
                value: "Gain {shown} ".into(),
            },
            ResolvedLocalizedPart::Placeholder {
                name: "card".into(),
                value: "Radiant Echo".into(),
                annotation: annotated.annotations().get("card"),
            },
            ResolvedLocalizedPart::Literal {
                value: " for ".into(),
            },
            ResolvedLocalizedPart::Placeholder {
                name: "count".into(),
                value: "2".into(),
                annotation: None,
            },
            ResolvedLocalizedPart::Literal {
                value: " turns (".into(),
            },
            ResolvedLocalizedPart::Placeholder {
                name: "plain".into(),
                value: "true".into(),
                annotation: None,
            },
            ResolvedLocalizedPart::Literal { value: ").".into() },
        ]
    );
    assert_eq!(join(&outcome.parts), localizer.resolve(&value));
    assert_eq!(diagnostics.lock().unwrap().len(), 2);
}

#[test]
fn target_order_repetition_and_omission_drive_the_parts() {
    let value = txa(
        "Offer {card} to {player} for {count} turns.",
        tx_args![card => "Echo", player => "Ada", count => 3_u32],
        "Offer summary.",
    );
    let annotated = value
        .clone()
        .annotate([(
            "card",
            Annotation {
                kind: "entity",
                id: "echo",
            },
        )])
        .unwrap();
    let localizer = Localizer::new(
        bundle("es", &[(&value, "{player}: {card} / {card}")], false),
        source_bundle(&[&value]),
    )
    .unwrap();

    let parts = localizer.resolve_parts_checked(&annotated).unwrap();
    let placeholders: Vec<_> = parts
        .iter()
        .filter_map(|part| match part {
            ResolvedLocalizedPart::Placeholder {
                name, annotation, ..
            } => Some((name.as_str(), *annotation)),
            ResolvedLocalizedPart::Literal { .. } => None,
        })
        .collect();
    assert_eq!(
        placeholders
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        ["player", "card", "card"]
    );
    assert!(placeholders[0].1.is_none());
    assert!(std::ptr::eq(
        placeholders[1].1.unwrap(),
        placeholders[2].1.unwrap()
    ));
    assert_eq!(join(&parts), localizer.resolve_checked(&value).unwrap());
}

#[test]
fn scalar_number_term_nested_opaque_and_bidi_match_string_resolution() {
    let nested = tx("Radiant Echo", "Card name.");
    let value = txa(
        "{text} / {number} / {noun} / {nested}",
        tx_args![
            text => "Ada",
            number => 12_345_u32,
            noun => counted(TermId::new("card"), 2_u32),
            nested => opaque(nested.clone()),
        ],
        "Mixed placeholder surfaces.",
    );
    let annotated = value
        .clone()
        .annotate([
            (
                "text",
                Annotation {
                    kind: "text",
                    id: "player",
                },
            ),
            (
                "nested",
                Annotation {
                    kind: "entity",
                    id: "card",
                },
            ),
        ])
        .unwrap();
    let mut source = source_bundle(&[&value, &nested]);
    source.terms.insert(
        "card".into(),
        BundleTerm {
            facets: BTreeMap::new(),
            forms: BTreeMap::from([
                ("$default".into(), scalar_term("en-US", "card")),
                (
                    "counted".into(),
                    BundleTermForm::Number {
                        values: BTreeMap::from([(
                            PluralCategory::Other,
                            BundleTermSurface {
                                origin_locale: "en-US".into(),
                                text: "cards".into(),
                            },
                        )]),
                    },
                ),
            ]),
        },
    );
    let mut target = bundle(
        "es",
        &[
            (&value, "{nested} · {noun} · {number} · {text}"),
            (&nested, "Eco radiante"),
        ],
        true,
    );
    target.terms.insert(
        "card".into(),
        BundleTerm {
            facets: BTreeMap::new(),
            forms: BTreeMap::from([(
                "counted".into(),
                BundleTermForm::Number {
                    values: BTreeMap::from([(
                        PluralCategory::Other,
                        BundleTermSurface {
                            origin_locale: "es".into(),
                            text: "cartas".into(),
                        },
                    )]),
                },
            )]),
        },
    );
    let localizer = Localizer::new(target, source).unwrap();

    let parts = localizer.resolve_parts_checked(&annotated).unwrap();
    let surfaces: Vec<_> = parts
        .iter()
        .filter_map(|part| match part {
            ResolvedLocalizedPart::Placeholder { name, value, .. } => {
                Some((name.as_str(), value.as_str()))
            }
            ResolvedLocalizedPart::Literal { .. } => None,
        })
        .collect();
    assert_eq!(
        surfaces,
        [
            ("nested", "\u{2068}Eco radiante\u{2069}"),
            ("noun", "\u{2068}cartas\u{2069}"),
            ("number", "\u{2068}12,345\u{2069}"),
            ("text", "\u{2068}Ada\u{2069}"),
        ]
    );
    assert_eq!(join(&parts), localizer.resolve_checked(&value).unwrap());
}

#[test]
fn recovery_diagnostics_and_validation_remain_unchanged() {
    let value = txa(
        "Create {noun}.",
        tx_args![noun => counted(TermId::new("missing"), 2_u32)],
        "Creation result.",
    );
    let annotated = value
        .clone()
        .annotate([(
            "noun",
            Annotation {
                kind: "term",
                id: "missing",
            },
        )])
        .unwrap();
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&diagnostics);
    let localizer = Localizer::new(
        bundle("es", &[(&value, "Crear {noun}.")], false),
        source_bundle(&[&value]),
    )
    .unwrap()
    .with_diagnostic_hook(move |diagnostic| {
        captured.lock().unwrap().push(diagnostic.code);
    });
    assert_eq!(
        join(&localizer.resolve_parts(&annotated)),
        localizer.resolve(&value)
    );
    assert_eq!(diagnostics.lock().unwrap().len(), 2);

    let error = value
        .clone()
        .annotate([(
            "missing",
            Annotation {
                kind: "entity",
                id: "x",
            },
        )])
        .unwrap_err();
    assert_eq!(error.code, "trox.unknown-annotation");

    let duplicate = value
        .clone()
        .annotate([
            (
                "noun",
                Annotation {
                    kind: "term",
                    id: "first",
                },
            ),
            (
                "noun",
                Annotation {
                    kind: "term",
                    id: "second",
                },
            ),
        ])
        .unwrap_err();
    assert_eq!(duplicate.code, "trox.duplicate-annotation");

    assert!(
        Localizer::new(
            bundle("es", &[(&value, "Crear {noun")], false),
            source_bundle(&[&value]),
        )
        .is_err()
    );
}

fn join<T>(parts: &[ResolvedLocalizedPart<'_, T>]) -> String {
    parts
        .iter()
        .map(|part| match part {
            ResolvedLocalizedPart::Literal { value }
            | ResolvedLocalizedPart::Placeholder { value, .. } => value.as_str(),
        })
        .collect()
}

fn plural_rules() -> PluralRules {
    PluralRules {
        cardinal: BTreeMap::from([
            (PluralCategory::One, "i = 1 and v = 0".into()),
            (PluralCategory::Other, String::new()),
        ]),
        ordinal: BTreeMap::from([(PluralCategory::Other, String::new())]),
    }
}

fn bundle(locale: &str, rows: &[(&LocalizedString, &str)], isolate: bool) -> Bundle {
    let mut bundle = Bundle {
        cldr_version: "48".into(),
        direction: TextDirection::Ltr,
        entries: BTreeMap::new(),
        fallback_chain: if locale == "en-US" {
            vec![]
        } else {
            vec!["en-US".into()]
        },
        fallbacks_flattened: true,
        format: "trox-bundle".into(),
        isolation: if isolate {
            IsolationPolicy::Isolate
        } else {
            IsolationPolicy::None
        },
        locale: locale.into(),
        message_facets: vec![],
        number_format: NumberFormat::default(),
        plural_rules: plural_rules(),
        source_catalog_fingerprint: "0".repeat(64),
        source_locale: "en-US".into(),
        terms: BTreeMap::new(),
        version: Version::V1,
    };
    for (value, translation) in rows {
        bundle.entries.insert(
            value.entry_id().into(),
            translated_entry(value, translation),
        );
    }
    bundle
}

fn source_bundle(values: &[&LocalizedString]) -> Bundle {
    let mut source = bundle("en-US", &[], false);
    for value in values {
        source.entries.insert(
            value.entry_id().into(),
            BundleEntry {
                arguments: Some(argument_schemas(value)),
                contract_signature: None,
                identity: Some(value.identity().clone()),
                rows: BTreeMap::new(),
                source_signature: value.source_signature().into(),
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
        entry_signature: value.source_signature().into(),
        path: vec![],
    };
    let row_id = expansion_row_id(&expansion).unwrap();
    BundleEntry {
        arguments: None,
        contract_signature: None,
        identity: None,
        rows: BTreeMap::from([(
            row_id,
            BundleRow {
                expansion,
                origin_locale: "es".into(),
                translation: translation.into(),
            },
        )]),
        source_signature: value.source_signature().into(),
    }
}

fn scalar_term(locale: &str, text: &str) -> BundleTermForm {
    BundleTermForm::Scalar {
        origin_locale: locale.into(),
        text: text.into(),
    }
}
