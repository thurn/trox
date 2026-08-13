use super::*;

fn scan(source: &str, language: Language) -> ScanResult {
    scan_with_ron_default(source, language, Some("Default description."))
}

fn scan_with_ron_default(
    source: &str,
    language: Language,
    ron_default_description: Option<&str>,
) -> ScanResult {
    let mut scanner = Scanner {
        source,
        path: Path::new("fixture"),
        language,
        tsx: false,
        ron_default_description,
        line_starts: line_starts(source),
        messages: vec![],
        diagnostics: vec![],
    };
    scanner.scan();
    ScanResult {
        messages: scanner.messages,
        diagnostics: scanner.diagnostics,
        bytes_scanned: source.len() as u64,
    }
}

#[test]
fn captures_ron_field_paths_separately_from_authored_descriptions() {
    let result = scan_with_ron_default(
        r#"[
            CardDefinition(
                name: Tx("Windcutter"),
                ability_text: [
                    Tx("First ability."),
                    Tx(text: "Second ability.", description: "Authored guidance."),
                ],
            ),
        ]"#,
        Language::Ron,
        Some("Default guidance."),
    );

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.messages[0].ron_path.as_deref(),
        Some("CardDefinition.name")
    );
    assert_eq!(
        result.messages[1].ron_path.as_deref(),
        Some("CardDefinition.ability_text")
    );
    assert_eq!(
        result.messages[2].ron_path.as_deref(),
        Some("CardDefinition.ability_text")
    );
    assert_eq!(
        result.messages[0].description.as_deref(),
        Some("Default guidance.")
    );
    assert_eq!(
        result.messages[2].description.as_deref(),
        Some("Authored guidance.")
    );
}

#[test]
fn captures_a_ron_field_path_without_a_configured_default() {
    let result = scan_with_ron_default(
        r#"CardDefinition(name: Tx("Windcutter"))"#,
        Language::Ron,
        None,
    );

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.messages[0].ron_path.as_deref(),
        Some("CardDefinition.name")
    );
    assert_eq!(result.messages[0].description, None);
}

#[test]
fn ron_paths_retain_nested_constructor_and_field_context() {
    let result = scan_with_ron_default(
        r#"Outer(details: [Inner(label: Tx("Nested"))])"#,
        Language::Ron,
        None,
    );

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.messages[0].ron_path.as_deref(),
        Some("Outer.details.Inner.label")
    );
}

fn scan_tsx(source: &str) -> ScanResult {
    let mut scanner = Scanner {
        source,
        path: Path::new("fixture.tsx"),
        language: Language::TypeScript,
        tsx: true,
        ron_default_description: None,
        line_starts: line_starts(source),
        messages: vec![],
        diagnostics: vec![],
    };
    scanner.scan();
    ScanResult {
        messages: scanner.messages,
        diagnostics: scanner.diagnostics,
        bytes_scanned: source.len() as u64,
    }
}

#[test]
fn extracts_equivalent_static_languages() {
    let rust = scan(r#"tx("Close", "Button label.")"#, Language::Rust);
    let ts = scan(r#"tx("Close", "Button label.")"#, Language::TypeScript);
    let ron = scan(r#"label: Tx("Close")"#, Language::Ron);
    assert!(rust.diagnostics.is_empty());
    assert!(ts.diagnostics.is_empty());
    assert!(ron.diagnostics.is_empty());
    assert_eq!(rust.messages[0].entry_id, ts.messages[0].entry_id);
    assert_eq!(rust.messages[0].entry_id, ron.messages[0].entry_id);
}

#[test]
fn extracts_short_and_named_ron_tx_forms_equivalently() {
    let short = scan(r#"label: Tx("Hello")"#, Language::Ron);
    let short_with_comma = scan(r##"label: Tx(r#"Hello"#,)"##, Language::Ron);
    let named = scan(r#"label: Tx(text: "Hello")"#, Language::Ron);

    for result in [&short, &short_with_comma, &named] {
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.messages.len(), 1);
    }
    assert_eq!(short.messages[0].entry_id, named.messages[0].entry_id);
    assert_eq!(
        short_with_comma.messages[0].entry_id,
        named.messages[0].entry_id
    );
}

#[test]
fn extracts_nested_typescript() {
    let result = scan(
        r#"txa(select(owner, [when("player", plural(count, [one("{count} card"), other("{count} cards")])), otherwise("None")]), { count }, "Status.")"#,
        Language::TypeScript,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.messages[0].selector_labels.len(), 2);
}

#[test]
fn ignores_comments_and_strings() {
    let result = scan(
        r#"// tx("No", "No")
const x = "tx(\\\"No\\\", \\\"No\\\")";
tx("Yes", "Actual label.");"#,
        Language::TypeScript,
    );
    assert_eq!(result.messages.len(), 1);
}

#[test]
fn ignores_assert_localized_calls() {
    for (source, language) in [
        (
            r#"assert_localized("Not extracted {raw} text")"#,
            Language::Rust,
        ),
        (
            r#"assertLocalized("Not extracted {raw} text")"#,
            Language::TypeScript,
        ),
    ] {
        let result = scan(source, language);
        assert!(result.messages.is_empty());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }
}

#[test]
fn reports_sound_source_contract_violations() {
    for (source, rule) in [
        (r#"tx(dynamic, "Description.")"#, "trox.dynamic-pattern"),
        (
            r#"tx(select(1, [when(1, "One"), otherwise("Other")]), "Description.")"#,
            "trox.numeric-select",
        ),
        (
            r#"txa("Deck: {deck_name}", { other_name: deck_name }, "Description.")"#,
            "trox.argument-mismatch",
        ),
        (r#"Tx(text: "Deck: {deck_name}")"#, "trox.ron-placeholder"),
    ] {
        let language = if source.starts_with("Tx") {
            Language::Ron
        } else {
            Language::TypeScript
        };
        let result = scan(source, language);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.rule_id == rule),
            "{source}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn scans_unicode_code_and_block_comments_without_panicking() {
    let rust = scan(
        "let café = 1; /* Καλημέρα */ tx(\"Close\", \"Button label.\");",
        Language::Rust,
    );
    assert!(rust.diagnostics.is_empty(), "{:?}", rust.diagnostics);
    assert_eq!(rust.messages.len(), 1);

    let typescript = scan(
        "const café = 1; /* Καλημέρα */ tx(\"Open\", \"Button label.\");",
        Language::TypeScript,
    );
    assert!(
        typescript.diagnostics.is_empty(),
        "{:?}",
        typescript.diagnostics
    );
    assert_eq!(typescript.messages.len(), 1);
}

#[test]
fn decodes_host_string_escapes_to_runtime_values() {
    let rust_escaped = scan(r#"tx("\x43lose", "Button label.")"#, Language::Rust);
    let rust_plain = scan(r#"tx("Close", "Button label.")"#, Language::Rust);
    assert_eq!(
        rust_escaped.messages[0].entry_id,
        rust_plain.messages[0].entry_id
    );

    let ts_escaped = scan(
        r#"tx("A\bB\fC\vD \uD83D\uDE00", "Escaped text.")"#,
        Language::TypeScript,
    );
    assert!(
        ts_escaped.diagnostics.is_empty(),
        "{:?}",
        ts_escaped.diagnostics
    );
    assert_eq!(
        ts_escaped.messages[0].identity.pattern,
        Pattern::Text {
            text: "A\u{0008}B\u{000c}C\u{000b}D 😀".into()
        }
    );

    let ts_identity_escape = scan(
        r#"tx("\é and \😀", "Identity escapes.")"#,
        Language::TypeScript,
    );
    assert!(
        ts_identity_escape.diagnostics.is_empty(),
        "{:?}",
        ts_identity_escape.diagnostics
    );
    assert!(matches!(
        &ts_identity_escape.messages[0].identity.pattern,
        Pattern::Text { text } if text == "é and 😀"
    ));
}

#[test]
fn tsx_scans_only_executable_expression_ranges() {
    let result = scan_tsx(
        r#"
const view = <div>
  tx("Raw child text", "Not executable")
  {tx("Expression", "Executable child expression.")}
  <span title={tx("Attribute", "Executable attribute expression.")}>
    txa("Still raw text", {}, "Not executable")
  </span>
</div>;
"#,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let texts = result
        .messages
        .iter()
        .map(|message| match &message.identity.pattern {
            Pattern::Text { text } => text.as_str(),
            _ => panic!("expected text"),
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["Expression", "Attribute"]);
}

#[test]
fn scan_file_enables_tsx_lexing_from_the_source_extension() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("view.tsx");
    std::fs::write(
        &path,
        r#"const view = <div>tx("Raw", "Not executable"){tx("Real", "Expression.")}</div>;"#,
    )
    .unwrap();

    let result = scan_file(&path, Language::TypeScript, None);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.messages.len(), 1);
    assert!(matches!(
        &result.messages[0].identity.pattern,
        Pattern::Text { text } if text == "Real"
    ));
}

#[test]
fn extracts_direct_atomic_calls_nested_inside_opaque_arguments() {
    let result = scan(
        r#"txa("Card: {card_name}", { card_name: opaque(tx("Ace", "Atomic card name.")) }, "Card label.")"#,
        Language::TypeScript,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let texts = result
        .messages
        .iter()
        .map(|message| match &message.identity.pattern {
            Pattern::Text { text } => text.as_str(),
            _ => panic!("expected text"),
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["Card: {card_name}", "Ace"]);
}

#[test]
fn rejects_host_numeric_literal_spellings_in_select() {
    for (source, language) in [
        (
            r#"tx(select(0x10, [otherwise("Other")]), "Hex selector.")"#,
            Language::TypeScript,
        ),
        (
            r#"tx(select(0b10n, [otherwise("Other")]), "BigInt selector.")"#,
            Language::TypeScript,
        ),
        (
            r#"tx(select(1_000u32, [otherwise("Other")]), "Rust selector.")"#,
            Language::Rust,
        ),
    ] {
        let result = scan(source, language);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.rule_id == "trox.numeric-select"),
            "{source}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn top_level_argument_iteration_is_lazy() {
    let input = format!("first,{}", "later,".repeat(10_000));
    let mut items = lexer::top_level_items(&input, ',', Language::TypeScript);
    assert_eq!(items.next(), Some("first"));
}

#[test]
fn distinguishes_typescript_regex_template_chunks_and_comment_rules() {
    let result = scan(
        r#"
const lookalike = /tx("Regex fake", "Not a call")/;
const rendered = `raw tx("Template fake", "Not a call") ${tx("Inside", "Template expression.")}`;
/* JavaScript comments do not nest: /* */ tx("After", "After comment.");
"#,
        Language::TypeScript,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let texts: Vec<_> = result
        .messages
        .iter()
        .map(|message| match &message.identity.pattern {
            Pattern::Text { text } => text.as_str(),
            _ => panic!("expected text"),
        })
        .collect();
    assert_eq!(texts, ["Inside", "After"]);
}

#[test]
fn parses_the_restricted_argument_helper_grammar_exactly() {
    let valid = classify_argument(
        r#"term(TermId::new("card-subtype.warrior")).form("indefinite").finish()"#,
        Language::Rust,
    )
    .unwrap();
    assert_eq!(
        valid,
        ArgumentSchema::Term {
            form: Some("indefinite".into()),
            number: false,
        }
    );

    let dynamic_form =
        classify_argument("term(term_id).form(form_id).finish()", Language::TypeScript)
            .unwrap_err();
    assert_eq!(dynamic_form.rule, "trox.literal-required");

    let repeated_form = classify_argument(
        r#"term(term_id).form("first").form("second")"#,
        Language::TypeScript,
    )
    .unwrap_err();
    assert_eq!(repeated_form.rule, "trox.argument-map");

    assert_eq!(
        classify_argument(
            r#"wrapper(term(termId("card-subtype.warrior")))"#,
            Language::TypeScript
        )
        .unwrap(),
        ArgumentSchema::Scalar
    );
    assert_eq!(
        classify_argument(r##"opaque(r#"a,b"#)"##, Language::Rust).unwrap(),
        ArgumentSchema::Opaque
    );
}

#[test]
fn rejects_more_than_256_visible_arguments() {
    let placeholders = (0..257)
        .map(|index| format!("{{arg_{index}}}"))
        .collect::<String>();
    let arguments = (0..257)
        .map(|index| format!("arg_{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("txa(\"{placeholders}\", {{ {arguments} }}, \"Argument limit fixture.\")");
    let result = scan(&source, Language::TypeScript);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.rule_id == "trox.argument-limit"),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn file_read_diagnostics_include_the_source_path() {
    let path = Path::new("definitely-missing-scanner-fixture.rs");
    let result = scan_file(path, Language::Rust, None);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].path.as_deref(), Some(path));
}

#[test]
fn keeps_term_reachability_separate_from_the_argument_schema() {
    let result = scan(
        r#"txa("{noun}", { noun: counted(termId("card"), count) }, "Term use.")"#,
        Language::TypeScript,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.messages[0].arguments["noun"],
        ArgumentSchema::Term {
            form: Some("counted".into()),
            number: true,
        }
    );
    assert_eq!(result.messages[0].term_ids["noun"], Some("card".into()));
}

#[test]
fn predicate_normalization_preserves_whitespace_inside_literals() {
    let result = scan(
        r#"tx(select(choice, [when("a  b", "Wide"), when("a b", "Narrow"), otherwise("Other")]), "Choice.")"#,
        Language::TypeScript,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.messages[0].predicate_labels[&Vec::new()],
        vec![r#""a  b""#.to_owned(), r#""a b""#.to_owned()]
    );
}

#[test]
fn typescript_regex_goal_handles_postfix_division_and_blocks() {
    let result = scan(
        r#"
let value = 1;
const ratio = value++ / Number(tx("Actual", "Real call.")) / total;
if (ready) {} /tx("Fake", "Regex lookalike.")/.test(input);
"#,
        Language::TypeScript,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let texts = result
        .messages
        .iter()
        .map(|message| match &message.identity.pattern {
            Pattern::Text { text } => text.as_str(),
            _ => panic!("expected flat text"),
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["Actual"]);
}

#[test]
fn ignores_noncanonical_identifier_suffixes() {
    let result = scan(
        r#"
$tx("Dollar", "Not canonical.");
object.#tx("Private", "Not canonical.");
λtx("Unicode", "Not canonical.");
tx("Actual", "Canonical call.");
"#,
        Language::TypeScript,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.messages.len(), 1);
    assert!(matches!(
        &result.messages[0].identity.pattern,
        Pattern::Text { text } if text == "Actual"
    ));
}

#[test]
fn rejects_noncanonical_ron_descriptions() {
    let result = scan(
        "Tx(text: \"Close\", description: \"Cafe\\u{301}\")",
        Language::Ron,
    );
    assert!(result.messages.is_empty());
    assert_eq!(result.diagnostics[0].rule_id, "trox.description");
}

#[test]
fn rejects_selector_depth_during_parsing() {
    let mut pattern = "\"Leaf\"".to_owned();
    for _ in 0..17 {
        pattern = format!("select(choice, [otherwise({pattern})])");
    }
    let result = scan(
        &format!("tx({pattern}, \"Too deep.\")"),
        Language::TypeScript,
    );
    assert!(result.messages.is_empty());
    assert_eq!(result.diagnostics[0].rule_id, "trox.pattern-depth");
}

#[test]
fn argument_helper_errors_are_rebased_to_the_file() {
    let result = scan(
        "\n\n\ntxa(\"{noun}\", {\n  noun: term(term_id).form(dynamic_form).finish()\n}, \"Term use.\")",
        Language::TypeScript,
    );
    let diagnostic = result
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.rule_id == "trox.literal-required")
        .unwrap();
    assert!(
        diagnostic.span.as_ref().unwrap().line >= 4,
        "{diagnostic:?}"
    );
}
