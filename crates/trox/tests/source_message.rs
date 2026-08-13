use std::collections::BTreeMap;

use trox::{
    ArgumentSchema, Bundle, BundleEntry, BundleTerm, BundleTermForm, IsolationPolicy,
    LocalizedString, NumberFormat, PluralCategory, PluralRules, SourceMessageRef, TermId,
    TextDirection, Version, counted, opaque, tx, tx_args,
};

fn source_bundle(values: &[&LocalizedString]) -> Bundle {
    let entries = values
        .iter()
        .map(|value| {
            let arguments = value.ron_template_arguments().cloned().unwrap_or_default();
            (
                value.entry_id().to_owned(),
                BundleEntry {
                    arguments: Some(arguments.clone()),
                    contract_signature: Some(
                        trox::contract_signature(value.identity(), &arguments).unwrap(),
                    ),
                    identity: Some(value.identity().clone()),
                    rows: BTreeMap::new(),
                    source_signature: value.source_signature().to_owned(),
                },
            )
        })
        .collect();
    Bundle {
        cldr_version: "48".into(),
        direction: TextDirection::Ltr,
        entries,
        fallback_chain: vec![],
        fallbacks_flattened: true,
        format: "trox-bundle".into(),
        isolation: IsolationPolicy::Isolate,
        locale: "en-US".into(),
        message_facets: vec![],
        number_format: NumberFormat::default(),
        plural_rules: PluralRules {
            cardinal: BTreeMap::from([(PluralCategory::Other, String::new())]),
            ordinal: BTreeMap::from([(PluralCategory::Other, String::new())]),
        },
        source_catalog_fingerprint: "0".repeat(64),
        source_locale: "en-US".into(),
        terms: BTreeMap::new(),
        version: Version::V1_1,
    }
}

#[test]
fn ron_templates_round_trip_as_authorized_source_messages_and_bind_exactly() {
    let template: LocalizedString = ron::from_str(
        r#"Tx(text:"Deck: {deck_name} ({count})",placeholders:{"count":Scalar,"deck_name":Opaque})"#,
    )
    .unwrap();
    let deck_name = tx("Night Garden", "Deck name.");
    let catalog = source_bundle(&[&template, &deck_name])
        .source_catalog()
        .unwrap();
    let reference = template.source_message_ref().unwrap();
    let canonical = reference.to_canonical_json().unwrap();
    let message = catalog.source_message_from_json(&canonical).unwrap();

    assert_eq!(
        message.argument_schemas(),
        &BTreeMap::from([
            ("count".into(), ArgumentSchema::Scalar),
            ("deck_name".into(), ArgumentSchema::Opaque),
        ])
    );
    let bound = message
        .bind(tx_args![count => 3_u32, deck_name => opaque(deck_name.clone())])
        .unwrap();
    assert_eq!(bound.entry_id(), template.entry_id());
    assert!(message.bind(tx_args![count => 3_u32]).is_err());

    let static_value: LocalizedString = ron::from_str(r#"Tx("Close")"#).unwrap();
    let static_catalog = source_bundle(&[&static_value]).source_catalog().unwrap();
    let static_message = static_catalog
        .source_message_from_value(
            serde_json::to_value(static_value.source_message_ref().unwrap()).unwrap(),
        )
        .unwrap();
    assert!(static_message.bind(BTreeMap::new()).unwrap().is_atomic());
}

#[test]
fn scalar_source_reference_matches_the_typescript_conformance_fixture() {
    let template: LocalizedString =
        ron::from_str(r#"Tx(text:"Hello {name}",placeholders:{"name":Scalar})"#).unwrap();
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/source-message-scalar.json"
    ))
    .unwrap();
    assert_eq!(
        format!(
            "{}\n",
            template
                .source_message_ref()
                .unwrap()
                .to_canonical_json()
                .unwrap()
        ),
        fixture
    );
}

#[test]
fn source_message_terms_are_catalog_authorized_and_tampering_is_rejected() {
    let template: LocalizedString = ron::from_str(
        r#"Tx(text:"Erode {amount}",placeholders:{"amount":Term(form:"counted",number:true)})"#,
    )
    .unwrap();
    let mut source = source_bundle(&[&template]);
    source.terms.insert(
        "unit.erode".into(),
        BundleTerm {
            facets: BTreeMap::new(),
            forms: BTreeMap::from([
                (
                    "$default".into(),
                    BundleTermForm::Scalar {
                        origin_locale: "en-US".into(),
                        text: "Erode".into(),
                    },
                ),
                (
                    "counted".into(),
                    BundleTermForm::Number {
                        values: BTreeMap::from([(
                            trox::PluralCategory::Other,
                            trox::BundleTermSurface {
                                origin_locale: "en-US".into(),
                                text: "{number}".into(),
                            },
                        )]),
                    },
                ),
            ]),
        },
    );
    let catalog = source.source_catalog().unwrap();
    let reference = template.source_message_ref().unwrap();
    let message = catalog
        .source_message_from_value(serde_json::to_value(&reference).unwrap())
        .unwrap();
    assert!(
        message
            .bind(tx_args![amount => counted(TermId::new("unit.erode"), 3_u32)])
            .is_ok()
    );

    let mut tampered = serde_json::to_value(&reference).unwrap();
    tampered["contract_signature"] = serde_json::Value::String("0".repeat(64));
    assert!(catalog.source_message_from_value(tampered).is_err());
    let wrong_form: SourceMessageRef = SourceMessageRef {
        source_signature: "0".repeat(64),
        ..reference
    };
    assert!(
        catalog
            .source_message_from_value(serde_json::to_value(wrong_form).unwrap())
            .is_err()
    );
}
