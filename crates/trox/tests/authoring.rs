use std::collections::BTreeMap;

use trox::prelude::*;

#[derive(Clone, Copy)]
enum Owner {
    Player,
    Opponent,
}

impl TroxSelector for Owner {
    fn trox_key(&self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Opponent => "opponent",
        }
    }
}

#[test]
fn static_and_interpolated_values_are_immutable_wire_values() {
    let static_value = tx("Close deck browser", "Accessible close button label.");
    assert!(static_value.entry_id().starts_with("tx1_"));
    assert_eq!(static_value.entry_id().len(), 30);
    assert!(static_value.is_atomic());
    assert_eq!(static_value.entry_id(), "tx1_5ij3qqw6xjvvxamd3nyt23flsm");

    let deck_name = String::from("Night Garden");
    let value = txa(
        "Deck: {deck_name}",
        tx_args![deck_name],
        "Label followed by a user-authored deck name.",
    );
    assert_eq!(deck_name, "Night Garden", "tx_args! borrows its input");
    assert!(!value.is_atomic());
    assert!(value.to_canonical_json().unwrap().contains("\"deck_name\""));
}

#[test]
fn dynamic_identity_matches_the_typescript_conformance_value() {
    let card_count = 3_u32;
    let value = txa(
        plural(
            card_count,
            [
                exact(0_u32, "No cards remain."),
                one("{card_count} card remains."),
                other("{card_count} cards remain."),
            ],
        ),
        tx_args![card_count],
        "Status text showing the remaining card count.",
    );
    assert_eq!(value.entry_id(), "tx1_s344kgctdm34ctyozrpgp4ksly");
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/localized-card-count.json"
    ))
    .unwrap();
    assert_eq!(format!("{}\n", value.to_canonical_json().unwrap()), fixture);
}

#[test]
fn counted_term_wire_matches_the_shared_catalog_contract() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/source-term-contract.json"
    ))
    .unwrap();
    let wire = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/localized-counted-term.json"
    ))
    .unwrap();
    let source = trox::Bundle::from_canonical_json(source.trim_end()).unwrap();
    let catalog = source.source_catalog().unwrap();
    let decoded = catalog.localized_string_from_json(wire.trim_end()).unwrap();
    assert_eq!(format!("{}\n", decoded.to_canonical_json().unwrap()), wire);

    let mut substituted: serde_json::Value = serde_json::from_str(wire.trim_end()).unwrap();
    substituted["arguments"]["noun"] = serde_json::json!({
        "kind": "text",
        "value": "cards",
    });
    let substituted = serde_json_canonicalizer::to_string(&substituted).unwrap();
    assert!(matches!(
        catalog.localized_string_from_json(&substituted),
        Err(trox::DeserializeError::Unauthorized(message))
            if message.contains("does not match its source entry schema")
    ));
}

#[test]
fn term_id_deserialization_enforces_the_stable_id_contract() {
    let error = serde_json::from_str::<TermId>(r#""Not.Valid""#).unwrap_err();
    assert!(error.to_string().contains("invalid term ID"), "{error}");
}

#[test]
#[should_panic(expected = "opaque value identity hash does not match its wire IDs")]
fn authored_arguments_reject_a_forged_atomic_wire() {
    let nested = tx("Card name", "An independently translated card name.");
    let mut wire: serde_json::Value =
        serde_json::from_str(&nested.to_canonical_json().unwrap()).unwrap();
    wire["entry_id"] = serde_json::Value::String("tx1_aaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    let argument: trox::Argument = serde_json::from_value(serde_json::json!({
        "kind": "opaque",
        "value": wire,
    }))
    .unwrap();

    txa(
        "Open {card_name}.",
        BTreeMap::from([("card_name".to_owned(), argument)]),
        "Instruction containing an independently translated card name.",
    );
}

#[test]
#[should_panic(expected = "source text must be NFC")]
fn authored_arguments_revalidate_an_atomic_wire_identity() {
    let nested = tx("Card name", "An independently translated card name.");
    let mut wire: serde_json::Value =
        serde_json::from_str(&nested.to_canonical_json().unwrap()).unwrap();
    wire["identity"]["pattern"]["text"] = serde_json::Value::String("e\u{301}".into());
    let argument: trox::Argument = serde_json::from_value(serde_json::json!({
        "kind": "opaque",
        "value": wire,
    }))
    .unwrap();

    txa(
        "Open {card_name}.",
        BTreeMap::from([("card_name".to_owned(), argument)]),
        "Instruction containing an independently translated card name.",
    );
}

#[test]
fn nested_patterns_have_canonical_selector_paths() {
    let owner = Owner::Player;
    let count = 3_u32;
    let value = txa(
        select(
            owner,
            [
                when(
                    Owner::Player,
                    plural(
                        count,
                        [
                            one("You have {count} card."),
                            other("You have {count} cards."),
                        ],
                    ),
                ),
                when(
                    Owner::Opponent,
                    plural(
                        count,
                        [
                            one("Your opponent has {count} card."),
                            other("Your opponent has {count} cards."),
                        ],
                    ),
                ),
                otherwise(plural(
                    count,
                    [
                        one("That player has {count} card."),
                        other("That player has {count} cards."),
                    ],
                )),
            ],
        ),
        tx_args![count],
        "Battle status showing cards owned by one participant.",
    );
    let paths: Vec<_> = value
        .selectors()
        .iter()
        .map(|selector| selector.path())
        .collect();
    assert_eq!(paths, vec![&[][..], &[0][..], &[1][..], &[2][..]]);
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/localized-nested-owner-count.json"
    ))
    .unwrap();
    assert_eq!(format!("{}\n", value.to_canonical_json().unwrap()), fixture);
    let source = trox::Bundle {
        cldr_version: "48".into(),
        direction: trox::TextDirection::Ltr,
        entries: BTreeMap::from([(
            value.entry_id().to_owned(),
            trox::BundleEntry {
                arguments: Some(BTreeMap::from([(
                    "count".to_owned(),
                    trox::ArgumentSchema::Scalar,
                )])),
                identity: Some(value.identity().clone()),
                rows: BTreeMap::new(),
                source_signature: value.source_signature().to_owned(),
            },
        )]),
        fallback_chain: vec![],
        fallbacks_flattened: true,
        format: "trox-bundle".into(),
        isolation: trox::IsolationPolicy::Isolate,
        locale: "en-US".into(),
        message_facets: vec![],
        number_format: trox::NumberFormat::default(),
        plural_rules: trox::PluralRules {
            cardinal: BTreeMap::from([(trox::PluralCategory::Other, String::new())]),
            ordinal: BTreeMap::from([(trox::PluralCategory::Other, String::new())]),
        },
        source_catalog_fingerprint: "0".repeat(64),
        source_locale: "en-US".into(),
        terms: BTreeMap::new(),
        version: trox::Version::V1,
    };
    let catalog = source.source_catalog().unwrap();
    let mut corrupted: serde_json::Value = serde_json::from_str(fixture.trim()).unwrap();
    corrupted["selectors"].as_array_mut().unwrap().reverse();
    let corrupted = serde_json_canonicalizer::to_string(&corrupted).unwrap();
    assert!(catalog.localized_string_from_json(&corrupted).is_err());
}

#[test]
fn meaning_changes_identity_but_description_does_not() {
    let action = tx(meaning("open-action", "Open"), "Button action.");
    let state = tx(meaning("open-state", "Open"), "Room state.");
    let action_again = tx(meaning("open-action", "Open"), "Different useful context.");
    assert_ne!(action.entry_id(), state.entry_id());
    assert_eq!(action.entry_id(), action_again.entry_id());
}

#[test]
fn term_and_opaque_arguments_follow_the_shallow_contract() {
    let card_name = tx("Radiant Echo", "Proper name of a game card.");
    let subtype = TermId::new("card-subtype.event");
    let count = 2_u32;
    let args = tx_args![
        card_name => opaque(card_name),
        subtype => indefinite(subtype.clone()),
        counted_subtype => counted(subtype, count),
    ];
    assert_eq!(args.len(), 3);
}

#[test]
#[should_panic(expected = "numeric selector branches are duplicate or noncanonical")]
fn noncanonical_plural_order_is_a_programmer_assertion() {
    let _ = plural(1_u32, [other("cards"), one("card")]);
}

#[test]
fn argument_map_empty_form_is_well_typed() {
    let args = tx_args![];
    let _: BTreeMap<String, trox::Argument> = args;
}

#[test]
fn rfc_8785_number_encoding_matches_javascript() {
    let n = trox::TroxNumber::new(1e20).unwrap();
    let value = txa("{n}", tx_args![n], "Number.");
    assert!(
        value
            .to_canonical_json()
            .unwrap()
            .contains("\"value\":100000000000000000000")
    );
    let n = trox::TroxNumber::new(1e-7).unwrap();
    let value = txa("{n}", tx_args![n], "Number.");
    assert!(
        value
            .to_canonical_json()
            .unwrap()
            .contains("\"value\":1e-7")
    );
}

#[test]
fn identity_validation_rejects_exact_values_above_the_safe_integer_limit() {
    let identity = trox::IdentityDescriptor {
        identity_version: 1,
        meaning: None,
        pattern: trox::Pattern::Plural {
            branches: vec![
                trox::NumericBranch {
                    key: trox::NumericBranchKey::Exact {
                        exact: 9_007_199_254_740_992,
                    },
                    pattern: trox::Pattern::Text {
                        text: "Impossible".into(),
                    },
                },
                trox::NumericBranch {
                    key: trox::NumericBranchKey::Plural {
                        plural: trox::PluralCategory::Other,
                    },
                    pattern: trox::Pattern::Text {
                        text: "Fallback".into(),
                    },
                },
            ],
        },
    };

    let error = trox::identity_ids(&identity).unwrap_err();
    assert_eq!(error.code, "trox.invalid-selector-number");
}

#[test]
#[should_panic(expected = "argument text must be NFC")]
fn authored_text_arguments_must_be_nfc() {
    let name = "e\u{301}".to_owned();
    let _ = txa("Hello {name}", tx_args![name], "Greeting with a name.");
}
