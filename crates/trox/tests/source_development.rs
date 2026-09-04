use std::sync::{Arc, Mutex};

use trox::prelude::*;
use trox::{DiagnosticCode, IsolationPolicy, ResolvedLocalizedPart, TextDirection};

#[derive(Clone, Copy)]
enum Audience {
    Player,
    Opponent,
}

impl TroxSelector for Audience {
    fn trox_key(&self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Opponent => "opponent",
        }
    }
}

fn project() -> &'static str {
    r#"(
        source_locale: "en-US",
        term_forms: {
            "counted": (
                description: "Term inflected for an explicit count.",
                number: Required,
            ),
            "indefinite": (
                description: "Nonspecific singular noun phrase.",
                source_fallback: Default,
            ),
        },
    )"#
}

fn terms() -> &'static str {
    r#"{
        "unit.card": (
            value: "card",
            forms: {
                "counted": Number([One("card"), Other("cards")]),
            },
            facets: { "noun-class": "object" },
        ),
    }"#
}

fn localizer() -> Localizer {
    Localizer::for_source(SourceLocale::from_project_ron(project(), terms()).unwrap()).unwrap()
}

#[test]
fn static_and_new_source_messages_render_without_a_bundle() {
    let localizer = localizer();
    let original = tx("Close deck browser", "Button label.");
    assert_eq!(
        localizer.resolve_checked(&original).unwrap(),
        "Close deck browser"
    );

    let newly_authored = tx(
        "This source edit is visible immediately.",
        "Message added after any prior extraction.",
    );
    assert_eq!(
        localizer.resolve(&newly_authored),
        "This source edit is visible immediately."
    );
    assert!(
        !localizer
            .resolve_outcome(&newly_authored)
            .used_source_fallback
    );
}

#[test]
fn interpolation_selectors_exact_branches_and_locale_rules_use_live_source_identity() {
    let localizer = localizer();

    let count = 0_u32;
    let exact_value = txa(
        plural(
            count,
            [
                exact(0_u32, "No cards remain."),
                one("{count} card remains."),
                other("{count} cards remain."),
            ],
        ),
        tx_args![count],
        "Remaining cards.",
    );
    assert_eq!(
        localizer.resolve_checked(&exact_value).unwrap(),
        "No cards remain."
    );

    let count = 2_u32;
    let plural_value = txa(
        plural(
            count,
            [one("{count} card remains."), other("{count} cards remain.")],
        ),
        tx_args![count],
        "Remaining cards.",
    );
    assert_eq!(
        localizer.resolve_checked(&plural_value).unwrap(),
        "\u{2068}2\u{2069} cards remain."
    );

    let position = 22_u32;
    let ordinal_value = txa(
        ordinal(
            position,
            [
                one("{position}st place"),
                two("{position}nd place"),
                few("{position}rd place"),
                other("{position}th place"),
            ],
        ),
        tx_args![position],
        "Placement.",
    );
    assert_eq!(
        localizer.resolve_checked(&ordinal_value).unwrap(),
        "\u{2068}22\u{2069}nd place"
    );

    let audience = Audience::Opponent;
    let selected = tx(
        select(
            audience,
            [
                when(Audience::Player, "You draw a card."),
                when(Audience::Opponent, "Your opponent draws a card."),
                otherwise("That player draws a card."),
            ],
        ),
        "Draw result.",
    );
    assert_eq!(
        localizer.resolve_checked(&selected).unwrap(),
        "Your opponent draws a card."
    );
}

#[test]
fn numbers_direction_terms_forms_counts_nested_values_and_ls_work_without_json() {
    let german = Localizer::for_source(SourceLocale::new("de").unwrap()).unwrap();
    let number = 12_345_u32;
    let formatted = txa("Score: {number}", tx_args![number], "Localized score.");
    assert_eq!(
        german.resolve_checked(&formatted).unwrap(),
        "Score: \u{2068}12.345\u{2069}"
    );
    assert_eq!(
        SourceLocale::new("ar").unwrap().direction(),
        TextDirection::Rtl
    );

    let localizer = localizer();
    let count = 2_u32;
    let term_value = txa(
        "Draw {count} {noun} with {article}.",
        tx_args![
            count,
            noun => counted(TermId::new("unit.card"), count),
            article => indefinite(TermId::new("unit.card")),
        ],
        "Term forms.",
    );
    assert_eq!(
        localizer.resolve_checked(&term_value).unwrap(),
        "Draw \u{2068}2\u{2069} \u{2068}cards\u{2069} with \u{2068}card\u{2069}."
    );

    let nested = tx("Radiant Echo", "Card name.");
    let opaque_value = txa(
        "Open {card_name}.",
        tx_args![card_name => opaque(nested)],
        "Open a named card.",
    );
    assert_eq!(
        localizer.resolve_checked(&opaque_value).unwrap(),
        "Open \u{2068}Radiant Echo\u{2069}."
    );
    assert_eq!(localizer.resolve(&ls("Debug value")), "Debug value");
}

#[test]
fn annotated_source_parts_preserve_boundaries_and_do_not_report_missing_rows() {
    let diagnostics = Arc::new(Mutex::new(Vec::<DiagnosticCode>::new()));
    let captured = Arc::clone(&diagnostics);
    let localizer = localizer().with_diagnostic_hook(move |diagnostic| {
        captured.lock().unwrap().push(diagnostic.code);
    });
    let count = 2_u32;
    let value = txa(
        "Draw {count} {noun}.",
        tx_args![
            count,
            noun => counted(TermId::new("unit.card"), count),
        ],
        "Annotated term message.",
    );
    let annotated = value.clone().annotate([("noun", "term-metadata")]).unwrap();

    let parts = localizer.resolve_parts_checked(&annotated).unwrap();
    assert_eq!(
        parts,
        vec![
            ResolvedLocalizedPart::Literal {
                value: "Draw ".into(),
            },
            ResolvedLocalizedPart::Placeholder {
                name: "count".into(),
                value: "\u{2068}2\u{2069}".into(),
                annotation: None,
            },
            ResolvedLocalizedPart::Literal { value: " ".into() },
            ResolvedLocalizedPart::Placeholder {
                name: "noun".into(),
                value: "\u{2068}cards\u{2069}".into(),
                annotation: annotated.annotations().get("noun"),
            },
            ResolvedLocalizedPart::Literal { value: ".".into() },
        ]
    );
    assert_eq!(
        localizer.resolve(&value),
        "Draw \u{2068}2\u{2069} \u{2068}cards\u{2069}."
    );
    assert!(diagnostics.lock().unwrap().is_empty());
}

#[test]
fn source_isolation_can_be_disabled_explicitly() {
    let localizer = Localizer::for_source(
        SourceLocale::new("en-US")
            .unwrap()
            .with_isolation(IsolationPolicy::None),
    )
    .unwrap();
    let name = "Ada";
    let value = txa("Hello, {name}.", tx_args![name], "Greeting.");
    assert_eq!(localizer.resolve_checked(&value).unwrap(), "Hello, Ada.");
}

#[test]
fn clean_fixture_application_builds_and_runs_without_generated_files_or_the_cli() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_fixture = manifest_dir.join("tests/fixtures/source-development-app");
    let temporary_fixture = (!repository_fixture.is_dir()).then(|| {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::create_dir(fixture.path().join("src")).unwrap();
        let dependency_path = manifest_dir
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        std::fs::write(
            fixture.path().join("Cargo.toml"),
            format!(
                r#"[package]
name = "trox-source-development-fixture"
version = "0.0.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
trox = {{ path = "{dependency_path}" }}
"#,
            ),
        )
        .unwrap();
        std::fs::write(
            fixture.path().join("src/main.rs"),
            r#"use trox::prelude::*;

fn main() {
    let localizer = Localizer::for_source(
        SourceLocale::from_project_ron(
            include_str!("../trox.ron"),
            include_str!("../terms.ron"),
        )
        .expect("valid source localization configuration"),
    )
    .expect("valid source localizer");
    let count = 2_u32;
    let message = txa(
        plural(
            count,
            [one("Draw {count} {noun}."), other("Draw {count} {noun}.")],
        ),
        tx_args![
            count,
            noun => counted(TermId::new("unit.card"), count),
        ],
        "Instruction using a counted source term.",
    );
    println!("{}", localizer.resolve_checked(&message).unwrap());
}
"#,
        )
        .unwrap();
        std::fs::write(
            fixture.path().join("terms.ron"),
            r#"{
    "unit.card": (
        value: "card",
        forms: {
            "counted": Number([One("card"), Other("cards")]),
        },
    ),
}
"#,
        )
        .unwrap();
        std::fs::write(
            fixture.path().join("trox.ron"),
            r#"(
    source_locale: "en-US",
    terms: "terms.ron",
    source_bundle: ".trox-output/en-US.trox.json",
    source_report: ".trox-output/en-US.csv",
    sources: [(language: Rust, include: ["src/**/*.rs"])],
    locales: {},
    term_forms: {
        "counted": (
            description: "Term inflected for an explicit count.",
            number: Required,
        ),
    },
)
"#,
        )
        .unwrap();
        fixture
    });
    let fixture = temporary_fixture
        .as_ref()
        .map_or(repository_fixture.as_path(), |fixture| fixture.path());
    let generated = std::fs::read_dir(fixture)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("csv" | "json")
            )
        })
        .collect::<Vec<_>>();
    assert!(generated.is_empty(), "fixture contains {generated:?}");

    let target = tempfile::tempdir().unwrap();
    let manifest = fixture.join("Cargo.toml");
    if temporary_fixture.is_some() {
        let output = std::process::Command::new(env!("CARGO"))
            .args(["generate-lockfile", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .env("CARGO_TARGET_DIR", target.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture cargo generate-lockfile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let run = |subcommand: &str| {
        let output = std::process::Command::new(env!("CARGO"))
            .args([subcommand, "--quiet", "--locked", "--manifest-path"])
            .arg(&manifest)
            .env("CARGO_TARGET_DIR", target.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture cargo {subcommand} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    run("check");
    run("test");
    run("clippy");
    let output = run("run");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Draw \u{2068}2\u{2069} \u{2068}cards\u{2069}.\n"
    );
}
