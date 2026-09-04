//! Regression tests for catalog aggregation and row expansion.

use super::*;
use trox::NumericBranchKey;

const NUMBER_FALLBACK_CONFIG: &str = r#"(
    source_locale: "en-US",
    terms: "terms.ron",
    source_bundle: "bundle.json",
    sources: [(language: Rust, include: ["**/*.rs"])],
    locales: {},
    term_forms: {
        "counted": (
            description: "A counted term.",
            number: Required,
            source_fallback: Default,
        ),
    },
)"#;

fn config_with_number_fallback() -> ProjectConfig {
    ron::from_str(NUMBER_FALLBACK_CONFIG).unwrap()
}

fn fallback_term() -> TermRecord {
    TermRecord {
        description: None,
        value: "Card".into(),
        forms: BTreeMap::new(),
        facets: BTreeMap::new(),
    }
}

fn profile() -> LocaleProfile {
    LocaleProfile {
        locale: "en-US".into(),
        direction: ProfileDirection::Ltr,
        isolation: ProfileIsolation::Isolate,
        fallbacks: vec![],
        facets: IndexMap::new(),
        term_facets: BTreeMap::new(),
        term_form_aliases: BTreeMap::new(),
    }
}

#[test]
fn message_rows_follow_the_earliest_source_location() {
    let config = config_with_number_fallback();
    let message = |entry_id: &str, english: &str, locations: Vec<(&str, usize)>| MessageEntry {
        entry_id: entry_id.into(),
        source_signature: format!("signature-{entry_id}"),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern: Pattern::Text {
                text: english.into(),
            },
        },
        descriptions: BTreeSet::new(),
        ron_paths: BTreeSet::new(),
        arguments: BTreeMap::new(),
        term_reachability: BTreeMap::new(),
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
        locations: locations
            .into_iter()
            .map(|(path, line)| SourceLocation {
                path: path.into(),
                line,
                column: 1,
            })
            .collect(),
        context_revision: "revision".into(),
    };
    let messages = [
        message("tx1_a", "Latest", vec![("src/z.rs", 1), ("src/a.rs", 30)]),
        message("tx1_b", "Middle", vec![("src/a.rs", 20)]),
        message("tx1_z", "Earliest", vec![("src/a.rs", 3)]),
    ]
    .into_iter()
    .map(|entry| (entry.entry_id.clone(), entry))
    .collect();
    let model = CatalogModel {
        messages,
        terms: BTreeMap::new(),
        bytes_scanned: 0,
        files_scanned: 2,
    };

    let rows = expand_rows(
        &config,
        &model,
        "en-US",
        &profile(),
        &mut Diagnostics::default(),
    )
    .unwrap();

    assert_eq!(
        rows.iter()
            .map(|row| row.english.as_str())
            .collect::<Vec<_>>(),
        ["Earliest", "Middle", "Latest"]
    );
}

#[test]
fn predicate_labels_merge_with_their_branch_ordinals() {
    let mut merged = BTreeMap::new();
    merge_predicate_labels(&mut merged, &[2, 0], &["zebra".into(), "alpha".into()]);
    merge_predicate_labels(&mut merged, &[2, 0], &["bravo".into(), "yankee".into()]);

    let branches = &merged[&vec![2, 0]];
    assert_eq!(
        branches[&0].iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["bravo", "zebra"]
    );
    assert_eq!(
        branches[&1].iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["alpha", "yankee"]
    );
}

#[test]
fn select_conditions_display_only_the_labels_for_the_selected_branch() {
    let config = config_with_number_fallback();
    let pattern = Pattern::Select {
        branches: vec![
            SelectIdentityBranch::When {
                pattern: Pattern::Text {
                    text: "First".into(),
                },
            },
            SelectIdentityBranch::When {
                pattern: Pattern::Text {
                    text: "Second".into(),
                },
            },
            SelectIdentityBranch::Otherwise {
                pattern: Pattern::Text {
                    text: "Other".into(),
                },
            },
        ],
    };
    let entry = MessageEntry {
        entry_id: "tx1_test".into(),
        source_signature: "signature".into(),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern,
        },
        descriptions: BTreeSet::new(),
        ron_paths: BTreeSet::new(),
        arguments: BTreeMap::new(),
        term_reachability: BTreeMap::new(),
        selector_labels: BTreeMap::from([(vec![], BTreeSet::from(["choice".into()]))]),
        predicate_labels: BTreeMap::from([(
            vec![],
            BTreeMap::from([
                (0, BTreeSet::from(["first-rust".into(), "first-ts".into()])),
                (1, BTreeSet::from(["second".into()])),
            ]),
        )]),
        locations: BTreeSet::new(),
        context_revision: "revision".into(),
    };
    let model = CatalogModel {
        messages: BTreeMap::from([("tx1_test".into(), entry)]),
        terms: BTreeMap::new(),
        bytes_scanned: 0,
        files_scanned: 0,
    };
    let rows = expand_rows(
        &config,
        &model,
        "en-US",
        &profile(),
        &mut Diagnostics::default(),
    )
    .unwrap();

    assert_eq!(rows[1].conditions, "choice.select[0]=first-rust | first-ts");
    assert_eq!(rows[2].conditions, "choice.select[1]=second");
    assert_eq!(
        rows[1].description,
        "Conditions: choice.select[0]=first-rust | first-ts"
    );
    assert_eq!(
        rows[0].description,
        "Conditions: choice.select[2]=otherwise"
    );
}

#[test]
fn message_meaning_is_included_in_the_translator_description() {
    let config = config_with_number_fallback();
    let entry = MessageEntry {
        entry_id: "tx1_test".into(),
        source_signature: "signature".into(),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: Some("open-action".into()),
            pattern: Pattern::Text {
                text: "Open".into(),
            },
        },
        descriptions: BTreeSet::from([
            "Button label that opens a deck.".into(),
            "Shown in the deck browser.".into(),
        ]),
        ron_paths: BTreeSet::new(),
        arguments: BTreeMap::new(),
        term_reachability: BTreeMap::new(),
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
        locations: BTreeSet::new(),
        context_revision: "revision".into(),
    };
    let model = CatalogModel {
        messages: BTreeMap::from([("tx1_test".into(), entry)]),
        terms: BTreeMap::new(),
        bytes_scanned: 0,
        files_scanned: 0,
    };

    let rows = expand_rows(
        &config,
        &model,
        "en-US",
        &profile(),
        &mut Diagnostics::default(),
    )
    .unwrap();

    assert_eq!(
        rows[0].description,
        "Meaning: open-action\n\nButton label that opens a deck.\n\nShown in the deck browser."
    );
}

#[test]
fn ron_paths_are_included_in_the_translator_description() {
    let config = config_with_number_fallback();
    let entry = MessageEntry {
        entry_id: "tx1_test".into(),
        source_signature: "signature".into(),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern: Pattern::Text {
                text: "Shared card text".into(),
            },
        },
        descriptions: BTreeSet::from(["Authored guidance.".into()]),
        ron_paths: BTreeSet::from([
            "CardDefinition.name".into(),
            "CardDefinition.ability_text".into(),
        ]),
        arguments: BTreeMap::new(),
        term_reachability: BTreeMap::new(),
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
        locations: BTreeSet::new(),
        context_revision: "revision".into(),
    };
    let model = CatalogModel {
        messages: BTreeMap::from([("tx1_test".into(), entry)]),
        terms: BTreeMap::new(),
        bytes_scanned: 0,
        files_scanned: 0,
    };

    let rows = expand_rows(
        &config,
        &model,
        "en-US",
        &profile(),
        &mut Diagnostics::default(),
    )
    .unwrap();

    assert_eq!(
        rows[0].description,
        "Authored guidance.\n\nPaths: CardDefinition.ability_text; CardDefinition.name"
    );
}

#[test]
fn numbered_default_fallback_is_compatible_and_expands_per_category() {
    let config = config_with_number_fallback();
    let term = fallback_term();
    assert!(matches!(
        resolve_term_surface(&config, &term, Some("counted"), true),
        Some(ResolvedTermSurface::NumberFallback("Card"))
    ));

    let model = CatalogModel {
        messages: BTreeMap::new(),
        terms: BTreeMap::from([("card".into(), term)]),
        bytes_scanned: 0,
        files_scanned: 0,
    };
    let rows = expand_term_rows(
        &config,
        &model,
        &profile(),
        &locale_data("en-US"),
        &mut Diagnostics::default(),
    )
    .unwrap();
    let counted = rows
        .iter()
        .filter(|row| row.conditions.starts_with("form=counted"))
        .collect::<Vec<_>>();
    assert_eq!(counted.len(), 2);
    assert!(
        counted
            .iter()
            .all(|row| row.english == "Card" && row.conditions.contains("number.plural="))
    );
}

#[test]
fn numbered_default_fallback_keeps_message_facets_reachable() {
    let config = config_with_number_fallback();
    let mut locale_profile = profile();
    locale_profile.facets.insert(
        "gender".into(),
        FacetDefinition {
            scope: FacetScope::Message,
            values: vec!["neutral".into()],
        },
    );
    locale_profile.term_facets.insert(
        "card".into(),
        BTreeMap::from([("gender".into(), "neutral".into())]),
    );
    let entry = MessageEntry {
        entry_id: "tx1_test".into(),
        source_signature: "signature".into(),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern: Pattern::Text {
                text: "{item}".into(),
            },
        },
        descriptions: BTreeSet::new(),
        ron_paths: BTreeSet::new(),
        arguments: BTreeMap::from([(
            "item".into(),
            ArgumentSchema::Term {
                form: Some("counted".into()),
                number: true,
            },
        )]),
        term_reachability: BTreeMap::from([("item".into(), None)]),
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
        locations: BTreeSet::new(),
        context_revision: "revision".into(),
    };
    let model = CatalogModel {
        messages: BTreeMap::new(),
        terms: BTreeMap::from([("card".into(), fallback_term())]),
        bytes_scanned: 0,
        files_scanned: 0,
    };
    let leaves = vec![ExpandedLeaf {
        path: vec![],
        conditions: vec![],
        english: "{item}".into(),
    }];

    let expanded = expand_facets(&config, leaves, &entry, &model, &locale_profile).unwrap();
    assert_eq!(expanded.len(), 1);
    assert_eq!(expanded[0].conditions, vec!["item.gender=neutral"]);
}

#[test]
fn source_locale_categories_are_required_for_messages_and_terms() {
    let other_only = Pattern::Plural {
        branches: vec![NumericBranch {
            key: NumericBranchKey::Plural {
                plural: PluralCategory::Other,
            },
            pattern: Pattern::Text {
                text: "Cards".into(),
            },
        }],
    };
    let message_error =
        validate_pattern_category_completeness("tx1_test", &other_only, &locale_data("en-US"))
            .unwrap_err()
            .to_string();
    assert!(message_error.contains("cardinal categories: one"));

    let terms = BTreeMap::from([(
        "card".into(),
        TermRecord {
            description: None,
            value: "Card".into(),
            forms: BTreeMap::from([(
                "counted".into(),
                TermSurface::Number(vec![NumberSurface::Other("Cards".into())]),
            )]),
            facets: BTreeMap::new(),
        },
    )]);
    let term_error = validate_term_category_completeness(&terms, &locale_data("en-US"))
        .unwrap_err()
        .to_string();
    assert!(term_error.contains("term `card` form `counted`"));
    assert!(term_error.contains("cardinal categories: one"));
}

#[test]
fn term_reachability_unions_static_ids_and_dynamic_widens_it() {
    let mut merged = TermReachability::new();
    merge_term_reachability(
        &mut merged,
        &BTreeMap::from([("noun".into(), Some("card".into()))]),
    );
    merge_term_reachability(
        &mut merged,
        &BTreeMap::from([("noun".into(), Some("deck".into()))]),
    );
    assert_eq!(
        merged["noun"],
        Some(BTreeSet::from(["card".into(), "deck".into()]))
    );
    merge_term_reachability(&mut merged, &BTreeMap::from([("noun".into(), None)]));
    assert_eq!(merged["noun"], None);
}

#[test]
fn catalog_merges_compatible_calls_with_different_static_terms() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("terms.ron"),
        r#"{
            "card": (value: "Card"),
            "deck": (value: "Deck"),
        }"#,
    )
    .unwrap();
    fs::create_dir(project.path().join("src")).unwrap();
    fs::write(
        project.path().join("src/messages.rs"),
        r#"
txa("{noun}", tx_args![noun => counted(TermId::new("card"), count)], "Term label.");
txa("{noun}", tx_args![noun => counted(TermId::new("deck"), count)], "Term label.");
"#,
    )
    .unwrap();
    let mut config = config_with_number_fallback();
    config.root = project.path().to_path_buf();
    config.path = project.path().join("trox.ron");
    fs::write(&config.path, NUMBER_FALLBACK_CONFIG).unwrap();
    let model = build_catalog(&config, &mut Diagnostics::default()).unwrap();
    assert_eq!(model.messages.len(), 1);
    assert_eq!(
        model.messages.values().next().unwrap().term_reachability["noun"],
        Some(BTreeSet::from(["card".into(), "deck".into()]))
    );
}

#[test]
fn empty_catalog_still_validates_term_form_declarations() {
    let mut config = config_with_number_fallback();
    config.term_forms.insert(
        "Invalid_Form".into(),
        crate::config::TermFormDeclaration {
            description: String::new(),
            number: NumberPolicy::Forbidden,
            source_fallback: None,
        },
    );
    let error = validate_terms(&config, &BTreeMap::new())
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid term form ID"), "{error}");
}

#[test]
fn facet_expansion_stops_at_the_configured_cap() {
    let mut config = config_with_number_fallback();
    config.max_expanded_rows_per_entry = 4;
    let arguments: BTreeMap<String, ArgumentSchema> = ["first", "second", "third"]
        .into_iter()
        .map(|name| {
            (
                name.into(),
                ArgumentSchema::Term {
                    form: Some("counted".into()),
                    number: true,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let term_reachability: TermReachability =
        arguments.keys().map(|name| (name.clone(), None)).collect();
    let entry = MessageEntry {
        entry_id: "tx1_cap".into(),
        source_signature: "signature".into(),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern: Pattern::Text { text: "x".into() },
        },
        descriptions: BTreeSet::new(),
        ron_paths: BTreeSet::new(),
        arguments,
        term_reachability,
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
        locations: BTreeSet::new(),
        context_revision: "revision".into(),
    };
    let model = CatalogModel {
        messages: BTreeMap::new(),
        terms: BTreeMap::from([
            ("card".into(), fallback_term()),
            ("deck".into(), fallback_term()),
        ]),
        bytes_scanned: 0,
        files_scanned: 0,
    };
    let mut locale_profile = profile();
    locale_profile.facets.insert(
        "gender".into(),
        FacetDefinition {
            scope: FacetScope::Message,
            values: vec!["a".into(), "b".into()],
        },
    );
    locale_profile.term_facets = BTreeMap::from([
        (
            "card".into(),
            BTreeMap::from([("gender".into(), "a".into())]),
        ),
        (
            "deck".into(),
            BTreeMap::from([("gender".into(), "b".into())]),
        ),
    ]);
    let error = expand_facets(
        &config,
        vec![ExpandedLeaf {
            path: vec![],
            conditions: vec![],
            english: "x".into(),
        }],
        &entry,
        &model,
        &locale_profile,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("over configured cap 4"), "{error}");
}

#[test]
fn selector_expansion_stops_before_materializing_over_cap_leaves() {
    let mut config = config_with_number_fallback();
    config.max_expanded_rows_per_entry = 4;
    let mut pattern = Pattern::Text {
        text: "Nested fallback".into(),
    };
    for _ in 0..16 {
        pattern = Pattern::Plural {
            branches: vec![
                NumericBranch {
                    key: NumericBranchKey::Plural {
                        plural: PluralCategory::One,
                    },
                    pattern: Pattern::Text { text: "One".into() },
                },
                NumericBranch {
                    key: NumericBranchKey::Plural {
                        plural: PluralCategory::Other,
                    },
                    pattern,
                },
            ],
        };
    }
    let entry = MessageEntry {
        entry_id: "tx1_selector_cap".into(),
        source_signature: "signature".into(),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern,
        },
        descriptions: BTreeSet::new(),
        ron_paths: BTreeSet::new(),
        arguments: BTreeMap::new(),
        term_reachability: BTreeMap::new(),
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
        locations: BTreeSet::new(),
        context_revision: "revision".into(),
    };
    let model = CatalogModel {
        messages: BTreeMap::from([(entry.entry_id.clone(), entry)]),
        terms: BTreeMap::new(),
        bytes_scanned: 0,
        files_scanned: 0,
    };
    let mut arabic = profile();
    arabic.locale = "ar".into();
    let error = expand_rows(&config, &model, "ar", &arabic, &mut Diagnostics::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("over configured cap 4"), "{error}");
}

#[test]
fn term_entries_obey_the_configured_row_cap() {
    let mut config = config_with_number_fallback();
    config.max_expanded_rows_per_entry = 1;
    let model = CatalogModel {
        messages: BTreeMap::new(),
        terms: BTreeMap::from([("card".into(), fallback_term())]),
        bytes_scanned: 0,
        files_scanned: 0,
    };
    let error = expand_term_rows(
        &config,
        &model,
        &profile(),
        &locale_data("en-US"),
        &mut Diagnostics::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("term:card"), "{error}");
    assert!(error.contains("over configured cap 1"), "{error}");
}

#[test]
fn locale_profiles_reject_unknown_catalog_and_facet_keys() {
    let config = config_with_number_fallback();
    let model = CatalogModel {
        messages: BTreeMap::new(),
        terms: BTreeMap::from([("card".into(), fallback_term())]),
        bytes_scanned: 0,
        files_scanned: 0,
    };

    let mut unknown_term = profile();
    unknown_term.term_form_aliases.insert(
        "typo".into(),
        BTreeMap::from([("counted".into(), AliasTarget::Default)]),
    );
    let error = expand_rows(
        &config,
        &model,
        "en-US",
        &unknown_term,
        &mut Diagnostics::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unknown term `typo`"), "{error}");

    let mut unknown_facet = profile();
    unknown_facet.term_facets.insert(
        "card".into(),
        BTreeMap::from([("gendre".into(), "x".into())]),
    );
    let error = expand_rows(
        &config,
        &model,
        "en-US",
        &unknown_facet,
        &mut Diagnostics::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unknown facet `gendre`"), "{error}");
}

#[test]
fn catalog_text_validation_rejects_empty_term_surfaces() {
    assert!(
        validate_text("", "term value")
            .unwrap_err()
            .to_string()
            .contains("must not be empty")
    );
}

#[test]
fn revision_context_tracks_only_relevant_forms_and_locale_term_metadata() {
    let mut config = config_with_number_fallback();
    let arguments = BTreeMap::from([(
        "noun".into(),
        ArgumentSchema::Term {
            form: Some("counted".into()),
            number: true,
        },
    )]);
    let before_forms = term_form_context(&config, &arguments);
    config.term_forms.get_mut("counted").unwrap().description = "Changed semantics.".into();
    assert_ne!(before_forms, term_form_context(&config, &arguments));

    let entry = MessageEntry {
        entry_id: "tx1_revision".into(),
        source_signature: "signature".into(),
        identity: IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern: Pattern::Text {
                text: "{noun}".into(),
            },
        },
        descriptions: BTreeSet::new(),
        ron_paths: BTreeSet::new(),
        arguments,
        term_reachability: BTreeMap::from([("noun".into(), Some(BTreeSet::from(["card".into()])))]),
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
        locations: BTreeSet::new(),
        context_revision: "revision".into(),
    };
    let model = CatalogModel {
        messages: BTreeMap::new(),
        terms: BTreeMap::from([("card".into(), fallback_term())]),
        bytes_scanned: 0,
        files_scanned: 0,
    };
    let mut locale_profile = profile();
    locale_profile.term_form_aliases.insert(
        "card".into(),
        BTreeMap::from([("counted".into(), AliasTarget::Default)]),
    );
    let aliased = locale_revision_context(&config, &entry, &model, &locale_profile);
    locale_profile.term_form_aliases.clear();
    let unaliased = locale_revision_context(&config, &entry, &model, &locale_profile);
    assert_ne!(aliased, unaliased);
}
