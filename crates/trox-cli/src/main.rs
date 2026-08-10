use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use trox_cli::bundle_build::{LocaleArtifacts, build_source_bundle, build_target_bundle};
use trox_cli::config::{LintLevel, ProjectConfig};
use trox_cli::csv_workflow::{prune, synchronize};
use trox_cli::diagnostic::{Diagnostic, DiagnosticResultExt, Diagnostics, classified_diagnostic};
use trox_cli::extract::{
    LocaleProfile, ProfileDirection, ProfileIsolation, build_catalog, expand_rows,
};
use trox_cli::locale_plan::LocalePlan;
use trox_cli::transaction::{atomic_replace_all, recover_pending_transaction};

#[derive(Debug, Parser)]
#[command(
    name = "trox",
    version,
    about = "Fast source-extraction localization tooling"
)]
struct Cli {
    /// Override the nearest parent trox.ron.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Emit diagnostics as JSON lines.
    #[arg(long, global = true)]
    json: bool,
    /// Override a named warning rule as RULE=warn or RULE=deny.
    #[arg(long = "lint", global = true, value_name = "RULE=LEVEL")]
    lint_overrides: Vec<String>,
    /// Suppress a named warning with the required reason: RULE=REASON.
    #[arg(long = "allow", global = true, value_name = "RULE=REASON")]
    lint_allows: Vec<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate source, terms, profiles, CSVs, and expansion without writes.
    Check {
        #[arg(long, value_name = "LOCALE")]
        locale: Vec<String>,
        #[arg(long, value_parser = ["warnings"])]
        deny: Option<String>,
    },
    /// Synchronize editable locale CSVs transactionally.
    Extract {
        #[arg(long, value_name = "LOCALE")]
        locale: Vec<String>,
        #[arg(long, value_parser = ["warnings"])]
        deny: Option<String>,
    },
    /// Build deterministic source and target JSON bundles.
    Bundle {
        #[arg(long, value_name = "LOCALE")]
        locale: Vec<String>,
        #[arg(long)]
        allow_missing: bool,
        #[arg(long, value_parser = ["warnings"])]
        deny: Option<String>,
    },
    /// Explicitly remove obsolete rows from locale CSVs.
    Prune {
        #[arg(long, value_name = "LOCALE")]
        locale: Vec<String>,
    },
    /// Locale profile scaffolding.
    Locale {
        #[command(subcommand)]
        command: LocaleCommand,
    },
    /// Measure scanner throughput over the configured corpus.
    Benchmark {
        #[arg(long, default_value_t = 5)]
        iterations: usize,
        /// Generate the normative mixed-language corpus at this total size.
        #[arg(long, default_value_t = 0)]
        synthetic_mib: usize,
        /// Fail when median throughput regresses by more than 15% from this value.
        #[arg(long)]
        baseline_mib_s: Option<f64>,
    },
}

#[derive(Debug, Subcommand)]
enum LocaleCommand {
    /// Create the configured locale profile and initial CSV.
    Init { locale: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(&cli) {
        let diagnostic = classified_diagnostic(&error).unwrap_or_else(|| {
            Diagnostic::error("trox.internal", format!("{error:#}")).with_guidance(
                "This failure was not classified; report it with the command and input paths.",
            )
        });
        if cli.json {
            eprintln!(
                "{}",
                serde_json::to_string(&diagnostic)
                    .expect("command diagnostic is always JSON serializable")
            );
        } else {
            eprintln!("{diagnostic}");
        }
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<()> {
    let config_path = cli.config.clone().map(Ok).unwrap_or_else(|| {
        ProjectConfig::discover(&env::current_dir()?).diagnostic(
            "trox.config-discovery",
            None,
            "Run inside a configured project or pass --config PATH.",
        )
    })?;
    let mut config = ProjectConfig::load(&config_path)?;
    recover_pending_transaction(&config.root, &configured_transaction_destinations(&config))?;
    apply_cli_lint_overrides(&mut config, &cli.lint_overrides, &cli.lint_allows).diagnostic(
        "trox.invalid-lint-option",
        Some(config_path.clone()),
        "Correct the named lint override or provide a nonempty suppression reason.",
    )?;
    let (rule_id, result) = match &cli.command {
        Command::Check { locale, deny } => (
            "trox.check",
            command_check(&config, locale, deny.is_some(), cli.json),
        ),
        Command::Extract { locale, deny } => (
            "trox.extract",
            command_extract(&config, locale, deny.is_some(), cli.json),
        ),
        Command::Bundle {
            locale,
            allow_missing,
            deny,
        } => (
            "trox.bundle",
            command_bundle(&config, locale, *allow_missing, deny.is_some(), cli.json),
        ),
        Command::Prune { locale } => ("trox.prune", command_prune(&config, locale, cli.json)),
        Command::Locale {
            command: LocaleCommand::Init { locale },
        } => (
            "trox.locale-init",
            command_locale_init(&config, locale, cli.json),
        ),
        Command::Benchmark {
            iterations,
            synthetic_mib,
            baseline_mib_s,
        } => (
            "trox.benchmark",
            trox_cli::benchmark::run(&config, *iterations, *synthetic_mib, *baseline_mib_s),
        ),
    };
    result.diagnostic(
        rule_id,
        Some(config.path.clone()),
        "Correct the reported validation failure and rerun the command.",
    )
}

fn source_profile(config: &ProjectConfig) -> LocaleProfile {
    LocaleProfile {
        locale: config.source_locale.clone(),
        direction: ProfileDirection::Ltr,
        isolation: ProfileIsolation::Isolate,
        fallbacks: vec![],
        facets: Default::default(),
        term_facets: BTreeMap::new(),
        term_form_aliases: BTreeMap::new(),
    }
}

fn configured_transaction_destinations(config: &ProjectConfig) -> Vec<PathBuf> {
    std::iter::once(&config.source_bundle)
        .chain(config.source_report.iter())
        .chain(
            config
                .locales
                .values()
                .flat_map(|locale| [&locale.profile, &locale.csv, &locale.bundle]),
        )
        .map(|path| config.resolve(path))
        .collect()
}

fn command_check(
    config: &ProjectConfig,
    requested: &[String],
    deny_warnings: bool,
    json: bool,
) -> Result<()> {
    let mut diagnostics = Diagnostics::default();
    let model = build_catalog(config, &mut diagnostics)?;
    let locale_plan = LocalePlan::load(config, requested)?;
    if let Some(report_path) = &config.source_report {
        let profile = source_profile(config);
        let rows = expand_rows(
            config,
            &model,
            &config.source_locale,
            &profile,
            &mut diagnostics,
        )?;
        let sync = synchronize(&config.resolve(report_path), &rows, true, &mut diagnostics)?;
        if sync.changed {
            diagnostics.push(Diagnostic::error(
                "trox.csv-out-of-date",
                format!(
                    "source report {} is out of date",
                    config.resolve(report_path).display()
                ),
            ));
        }
    }
    for (locale, profile) in locale_plan.profiles() {
        let rows = expand_rows(config, &model, locale, profile, &mut diagnostics)?;
        let path = config.resolve(&config.locales[locale].csv);
        let sync = synchronize(&path, &rows, false, &mut diagnostics)?;
        if sync.changed {
            diagnostics.push(Diagnostic::error(
                "trox.csv-out-of-date",
                format!(
                    "{} is out of date; run trox extract --locale {locale}",
                    path.display()
                ),
            ));
        }
    }
    apply_lint_policy(config, deny_warnings, &mut diagnostics);
    emit_diagnostics(&diagnostics, json)?;
    if diagnostics.has_errors() {
        bail!("validation failed with {} diagnostic(s)", diagnostics.len());
    }
    eprintln!(
        "checked {} files ({} bytes), {} messages, {} terms",
        model.files_scanned,
        model.bytes_scanned,
        model.messages.len(),
        model.terms.len()
    );
    Ok(())
}

fn command_extract(
    config: &ProjectConfig,
    requested: &[String],
    deny_warnings: bool,
    json: bool,
) -> Result<()> {
    let mut diagnostics = Diagnostics::default();
    let model = build_catalog(config, &mut diagnostics)?;
    let locale_plan = LocalePlan::load(config, requested)?;
    let mut replacements = Vec::new();
    if requested.is_empty()
        && let Some(report_path) = &config.source_report
    {
        let profile = source_profile(config);
        let rows = expand_rows(
            config,
            &model,
            &config.source_locale,
            &profile,
            &mut diagnostics,
        )?;
        let path = config.resolve(report_path);
        let sync = synchronize(&path, &rows, true, &mut diagnostics)?;
        replacements.push((path, sync.bytes));
    }
    for locale in locale_plan.selected() {
        let profile = locale_plan.profile(locale);
        let rows = expand_rows(config, &model, locale, profile, &mut diagnostics)?;
        let path = config.resolve(&config.locales[locale].csv);
        let sync = synchronize(&path, &rows, false, &mut diagnostics)?;
        replacements.push((path, sync.bytes));
    }
    apply_lint_policy(config, deny_warnings, &mut diagnostics);
    emit_diagnostics(&diagnostics, json)?;
    if diagnostics.has_errors() {
        bail!("extraction aborted before writing files");
    }
    atomic_replace_all(&config.root, &replacements)?;
    eprintln!(
        "extracted {} messages and {} terms from {} files",
        model.messages.len(),
        model.terms.len(),
        model.files_scanned
    );
    Ok(())
}

struct OwnedArtifacts {
    profile: LocaleProfile,
    rows: Vec<trox_cli::extract::ExpectedRow>,
    csv: trox_cli::csv_workflow::CsvDocument,
}

fn command_bundle(
    config: &ProjectConfig,
    requested: &[String],
    allow_missing: bool,
    deny_warnings: bool,
    json: bool,
) -> Result<()> {
    let mut diagnostics = Diagnostics::default();
    let model = build_catalog(config, &mut diagnostics)?;
    let locale_plan = LocalePlan::load(config, requested)?;
    let mut owned = BTreeMap::new();
    for (locale, profile) in locale_plan.profiles() {
        let rows = expand_rows(config, &model, locale, profile, &mut diagnostics)?;
        let path = config.resolve(&config.locales[locale].csv);
        let sync = synchronize(&path, &rows, false, &mut diagnostics)?;
        if sync.changed {
            diagnostics.push(Diagnostic::error(
                "trox.csv-out-of-date",
                format!("{} is out of date", path.display()),
            ));
        }
        owned.insert(
            locale.clone(),
            OwnedArtifacts {
                profile: profile.clone(),
                rows,
                csv: sync.document,
            },
        );
    }
    apply_lint_policy(config, deny_warnings, &mut diagnostics);
    emit_diagnostics(&diagnostics, json)?;
    if diagnostics.has_errors() {
        bail!("bundle aborted before writing files");
    }
    let source = build_source_bundle(config, &model)?;
    let mut replacements = vec![(
        config.resolve(&config.source_bundle),
        source.to_canonical_json()?.into_bytes(),
    )];
    for locale in locale_plan.selected() {
        let current = &owned[locale];
        let artifacts = LocaleArtifacts {
            profile: &current.profile,
            expected: &current.rows,
            csv: &current.csv,
        };
        let parents = current
            .profile
            .fallbacks
            .iter()
            .filter(|parent| *parent != &config.source_locale)
            .map(|parent| {
                let owned = owned
                    .get(parent)
                    .expect("validated fallback locale has loaded artifacts");
                (
                    parent.clone(),
                    LocaleArtifacts {
                        profile: &owned.profile,
                        expected: &owned.rows,
                        csv: &owned.csv,
                    },
                )
            })
            .collect();
        let bundle =
            build_target_bundle(config, &model, locale, &artifacts, &parents, allow_missing)?;
        replacements.push((
            config.resolve(&config.locales[locale].bundle),
            bundle.to_canonical_json()?.into_bytes(),
        ));
    }
    atomic_replace_all(&config.root, &replacements)?;
    eprintln!("built {} bundle artifact(s)", replacements.len());
    Ok(())
}

fn command_prune(config: &ProjectConfig, requested: &[String], json: bool) -> Result<()> {
    let mut diagnostics = Diagnostics::default();
    let model = build_catalog(config, &mut diagnostics)?;
    let locale_plan = LocalePlan::load(config, requested)?;
    let mut replacements = Vec::new();
    for locale in locale_plan.selected() {
        let path = config.resolve(&config.locales[locale].csv);
        let rows = expand_rows(
            config,
            &model,
            locale,
            locale_plan.profile(locale),
            &mut diagnostics,
        )?;
        replacements.push((path.clone(), prune(&path, &rows, &mut diagnostics)?));
    }
    apply_lint_policy(config, false, &mut diagnostics);
    emit_diagnostics(&diagnostics, json)?;
    if diagnostics.has_errors() {
        bail!("prune aborted before writing files");
    }
    atomic_replace_all(&config.root, &replacements)?;
    eprintln!("pruned obsolete rows from {} CSV(s)", replacements.len());
    Ok(())
}

fn command_locale_init(config: &ProjectConfig, locale: &str, json: bool) -> Result<()> {
    trox_cli::cldr::ensure_supported_locale(locale)?;
    let output = config
        .locales
        .get(locale)
        .with_context(|| format!("add locale `{locale}` to trox.ron before initializing it"))?;
    let path = config.resolve(&output.profile);
    if path.exists() {
        bail!("locale profile {} already exists", path.display());
    }
    let csv_path = config.resolve(&output.csv);
    if csv_path.exists() {
        bail!("locale CSV {} already exists", csv_path.display());
    }
    let data = trox_cli::cldr::locale_data(locale);
    let direction = if data.direction == trox::TextDirection::Rtl {
        "Rtl"
    } else {
        "Ltr"
    };
    let advisory = match locale.split('-').next().unwrap_or(locale) {
        "es" | "pt" | "fr" | "de" => {
            "    // Advisory: consider a checked gender facet if reusable terms drive agreement.\n"
        }
        "ru" | "pl" => {
            "    // Advisory: add explicit term forms or facets only for grammar used by complete messages.\n"
        }
        "ar" => "    // Advisory: RTL isolation is enabled for every placeholder substitution.\n",
        _ => "",
    };
    let profile_text = format!(
        "(\n    locale: \"{locale}\",\n    direction: {direction},\n    isolation: Isolate,\n    fallbacks: [\"{}\"],\n{advisory}    facets: {{}},\n    term_facets: {{}},\n    term_form_aliases: {{}},\n)\n",
        config.source_locale
    );
    let profile = LocaleProfile {
        locale: locale.into(),
        direction: if direction == "Rtl" {
            ProfileDirection::Rtl
        } else {
            ProfileDirection::Ltr
        },
        isolation: ProfileIsolation::Isolate,
        fallbacks: vec![config.source_locale.clone()],
        facets: Default::default(),
        term_facets: Default::default(),
        term_form_aliases: Default::default(),
    };
    let mut diagnostics = Diagnostics::default();
    let model = build_catalog(config, &mut diagnostics)?;
    let rows = expand_rows(config, &model, locale, &profile, &mut diagnostics)?;
    let sync = synchronize(&csv_path, &rows, false, &mut diagnostics)?;
    apply_lint_policy(config, false, &mut diagnostics);
    emit_diagnostics(&diagnostics, json)?;
    if diagnostics.has_errors() {
        bail!("locale initialization aborted before writing files");
    }
    atomic_replace_all(
        &config.root,
        &[
            (path.clone(), profile_text.into_bytes()),
            (csv_path.clone(), sync.bytes),
        ],
    )?;
    eprintln!("created {} and {}", path.display(), csv_path.display());
    Ok(())
}

fn apply_lint_policy(config: &ProjectConfig, cli_deny: bool, diagnostics: &mut Diagnostics) {
    diagnostics.apply_warning_policy(|rule| {
        if cli_deny {
            LintLevel::Deny
        } else {
            config.lint.level(rule)
        }
    });
}

fn apply_cli_lint_overrides(
    config: &mut ProjectConfig,
    overrides: &[String],
    allows: &[String],
) -> Result<()> {
    for value in overrides {
        let (rule, level) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--lint must be RULE=warn or RULE=deny"))?;
        let level = match level.to_ascii_lowercase().as_str() {
            "warn" => LintLevel::Warn,
            "deny" => LintLevel::Deny,
            "allow" => bail!("--lint cannot allow without a reason; use --allow RULE=REASON"),
            _ => bail!("invalid lint level `{level}`; expected warn or deny"),
        };
        config.lint.set_cli_level(rule.to_owned(), level, None)?;
    }
    for value in allows {
        let (rule, reason) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--allow must be RULE=REASON"))?;
        config
            .lint
            .set_cli_level(rule.to_owned(), LintLevel::Allow, Some(reason.to_owned()))?;
    }
    Ok(())
}

fn emit_diagnostics(diagnostics: &Diagnostics, json: bool) -> Result<()> {
    let mut stderr = io::stderr().lock();
    for diagnostic in diagnostics.iter() {
        if json {
            writeln!(stderr, "{}", serde_json::to_string(diagnostic)?)?;
        } else {
            writeln!(stderr, "{diagnostic}")?;
        }
    }
    Ok(())
}
