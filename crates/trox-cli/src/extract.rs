//! Source aggregation and catalog validation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use rayon::prelude::*;
use serde::de::{EnumAccess, SeqAccess, VariantAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use trox::{
    ExpansionDescriptor, IdentityDescriptor, NumericBranch, Pattern, PluralCategory,
    SelectIdentityBranch, expansion_row_id, revision_id,
};
use unicode_normalization::UnicodeNormalization;

use crate::cldr::{LocaleData, locale_data};
use crate::config::{NumberPolicy, ProjectConfig, SourceFallback};
use crate::diagnostic::{Diagnostic, DiagnosticResultExt, Diagnostics, Span};
use crate::scanner::{ArgumentSchema, ExtractedMessage, SourceLocation, scan_file};

mod discovery;
mod expansion;

use discovery::discover_source_files;
pub use expansion::expand_rows;
#[cfg(test)]
use expansion::{ExpandedLeaf, expand_facets, expand_term_rows, locale_revision_context};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TermRecord {
    #[serde(default, deserialize_with = "crate::config::plain_optional")]
    pub description: Option<String>,
    pub value: String,
    #[serde(default)]
    pub forms: BTreeMap<String, TermSurface>,
    #[serde(default)]
    pub facets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum TermSurface {
    Scalar(String),
    Number(Vec<NumberSurface>),
}

impl<'de> Deserialize<'de> for TermSurface {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct SurfaceVisitor;
        impl<'de> Visitor<'de> for SurfaceVisitor {
            type Value = TermSurface;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a string or Number([...])")
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(TermSurface::Scalar(value.into()))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                value: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(TermSurface::Scalar(value))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let values: Vec<NumberSurface> = sequence
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("Number requires one category list"))?;
                if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::custom(
                        "Number accepts exactly one category list",
                    ));
                }
                Ok(TermSurface::Number(values))
            }
            fn visit_enum<A: EnumAccess<'de>>(
                self,
                data: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let (variant, payload) = data.variant::<String>()?;
                if variant != "Number" {
                    return Err(serde::de::Error::unknown_variant(&variant, &["Number"]));
                }
                Ok(TermSurface::Number(payload.newtype_variant()?))
            }
        }
        deserializer.deserialize_any(SurfaceVisitor)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum NumberSurface {
    Zero(String),
    One(String),
    Two(String),
    Few(String),
    Many(String),
    Other(String),
}

impl NumberSurface {
    pub fn parts(&self) -> (PluralCategory, &str) {
        match self {
            Self::Zero(value) => (PluralCategory::Zero, value),
            Self::One(value) => (PluralCategory::One, value),
            Self::Two(value) => (PluralCategory::Two, value),
            Self::Few(value) => (PluralCategory::Few, value),
            Self::Many(value) => (PluralCategory::Many, value),
            Self::Other(value) => (PluralCategory::Other, value),
        }
    }
}

pub type TermCatalog = BTreeMap<String, TermRecord>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ProfileDirection {
    Ltr,
    Rtl,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum ProfileIsolation {
    #[default]
    Isolate,
    None,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum FacetScope {
    Term,
    Message,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FacetDefinition {
    pub scope: FacetScope,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum AliasTarget {
    Default,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocaleProfile {
    pub locale: String,
    pub direction: ProfileDirection,
    #[serde(default)]
    pub isolation: ProfileIsolation,
    pub fallbacks: Vec<String>,
    #[serde(default)]
    pub facets: IndexMap<String, FacetDefinition>,
    #[serde(default)]
    pub term_facets: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub term_form_aliases: BTreeMap<String, BTreeMap<String, AliasTarget>>,
}

/// Normalized predicate labels grouped by selector path and source branch ordinal.
pub type SelectorPredicateLabels = BTreeMap<Vec<usize>, BTreeMap<usize, BTreeSet<String>>>;
/// Literal term IDs reachable per argument. `None` means any compatible term is reachable.
pub type TermReachability = BTreeMap<String, Option<BTreeSet<String>>>;

#[derive(Debug, Clone)]
pub struct MessageEntry {
    pub entry_id: String,
    pub source_signature: String,
    pub identity: IdentityDescriptor,
    pub descriptions: BTreeSet<String>,
    pub ron_paths: BTreeSet<String>,
    pub arguments: BTreeMap<String, ArgumentSchema>,
    pub term_reachability: TermReachability,
    pub selector_labels: BTreeMap<Vec<usize>, BTreeSet<String>>,
    pub predicate_labels: SelectorPredicateLabels,
    pub locations: BTreeSet<SourceLocation>,
    pub context_revision: String,
}

#[derive(Debug, Clone)]
pub struct CatalogModel {
    pub messages: BTreeMap<String, MessageEntry>,
    pub terms: TermCatalog,
    pub bytes_scanned: u64,
    pub files_scanned: usize,
}

#[derive(Debug, Clone)]
pub struct ExpectedRow {
    #[cfg_attr(not(test), allow(dead_code))]
    pub conditions: String,
    pub english: String,
    pub description: String,
    pub placeholders: String,
    pub entry_id: String,
    pub row_id: String,
    pub kind: String,
    pub source_locations: String,
    pub source_revision: String,
    pub expansion: ExpansionDescriptor,
    pub term: Option<TermRowDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermRowDescriptor {
    Default,
    Scalar {
        form: String,
    },
    Number {
        form: String,
        category: PluralCategory,
    },
}

pub fn load_terms(config: &ProjectConfig) -> Result<TermCatalog> {
    load_terms_impl(config).diagnostic(
        "trox.invalid-term-catalog",
        Some(config.resolve(&config.terms)),
        "Correct the term catalog schema and contracts, then rerun the command.",
    )
}

fn load_terms_impl(config: &ProjectConfig) -> Result<TermCatalog> {
    let path = config.resolve(&config.terms);
    let input = fs::read_to_string(&path)
        .with_context(|| format!("failed to read term catalog {}", path.display()))?;
    let catalog: TermCatalog = ron::from_str(&input)
        .with_context(|| format!("invalid term catalog {}", path.display()))?;
    validate_terms(config, &catalog)?;
    let project = fs::read_to_string(&config.path)
        .with_context(|| format!("failed to read project config {}", config.path.display()))?;
    trox::SourceLocale::from_project_ron(&project, &input).map_err(|error| {
        anyhow::anyhow!(
            "source-development term configuration disagrees with the CLI contract: {}",
            error.message
        )
    })?;
    Ok(catalog)
}

fn validate_terms(config: &ProjectConfig, catalog: &TermCatalog) -> Result<()> {
    for (form_id, declaration) in &config.term_forms {
        validate_stable_id(form_id, "term form ID")?;
        validate_description_text(&declaration.description, "term form description")?;
    }
    for (term_id, term) in catalog {
        validate_stable_id(term_id, "term ID")?;
        validate_text(&term.value, "term value")?;
        if let Some(description) = &term.description {
            validate_description_text(description, "term description")?;
        }
        for (facet_id, value) in &term.facets {
            validate_stable_id(facet_id, "source term facet ID")?;
            validate_stable_id(value, "source term facet value")?;
        }
        for (form_id, surface) in &term.forms {
            let declaration = config
                .term_forms
                .get(form_id)
                .with_context(|| format!("term `{term_id}` uses undeclared form `{form_id}`"))?;
            match (surface, declaration.number) {
                (TermSurface::Scalar(value), NumberPolicy::Forbidden) => {
                    validate_text(value, "term form")?
                }
                (TermSurface::Number(values), NumberPolicy::Required) => {
                    validate_number_surfaces(term_id, form_id, values)?
                }
                _ => bail!("term `{term_id}` form `{form_id}` violates its number policy"),
            }
        }
        for (form_id, declaration) in &config.term_forms {
            if !term.forms.contains_key(form_id)
                && declaration.source_fallback != Some(SourceFallback::Default)
            {
                // Sparse catalogs may omit forms not used by the term. Reachability validation catches uses.
            }
        }
    }
    Ok(())
}

fn validate_number_surfaces(term_id: &str, form_id: &str, values: &[NumberSurface]) -> Result<()> {
    let mut previous = None;
    let mut has_other = false;
    for value in values {
        let (category, text) = value.parts();
        validate_text(text, "numbered term surface")?;
        let index = category_order(category);
        if previous.is_some_and(|prior| prior >= index) {
            bail!("term `{term_id}` form `{form_id}` categories are duplicate or noncanonical");
        }
        previous = Some(index);
        has_other |= category == PluralCategory::Other;
    }
    if !has_other {
        bail!("term `{term_id}` form `{form_id}` requires Other");
    }
    Ok(())
}

pub fn load_profile(config: &ProjectConfig, locale: &str) -> Result<LocaleProfile> {
    let path = config
        .locales
        .get(locale)
        .map(|locale| config.resolve(&locale.profile));
    load_profile_impl(config, locale).diagnostic(
        "trox.invalid-locale-profile",
        path,
        "Correct the locale profile and fallback metadata, then rerun the command.",
    )
}

fn load_profile_impl(config: &ProjectConfig, locale: &str) -> Result<LocaleProfile> {
    let locale_config = config
        .locales
        .get(locale)
        .with_context(|| format!("unknown configured locale `{locale}`"))?;
    let path = config.resolve(&locale_config.profile);
    let input = fs::read_to_string(&path)
        .with_context(|| format!("failed to read locale profile {}", path.display()))?;
    let profile: LocaleProfile = ron::from_str(&input)
        .with_context(|| format!("invalid locale profile {}", path.display()))?;
    if profile.locale != locale {
        bail!(
            "locale profile {} declares `{}`, expected `{locale}`",
            path.display(),
            profile.locale
        );
    }
    crate::config::validate_locale_id(&profile.locale)?;
    if profile.fallbacks.last().map(String::as_str) != Some(config.source_locale.as_str()) {
        bail!(
            "locale `{locale}` fallback chain must end in `{}`",
            config.source_locale
        );
    }
    let mut seen = BTreeSet::new();
    if profile
        .fallbacks
        .iter()
        .any(|fallback| fallback == locale || !seen.insert(fallback))
    {
        bail!("locale `{locale}` fallback chain is cyclic or duplicate");
    }
    for (facet_id, facet) in &profile.facets {
        validate_stable_id(facet_id, "facet ID")?;
        if facet.values.is_empty() {
            bail!("facet `{facet_id}` has no values");
        }
        let mut values = BTreeSet::new();
        for value in &facet.values {
            validate_stable_id(value, "facet value")?;
            if !values.insert(value) {
                bail!("facet `{facet_id}` repeats value `{value}`");
            }
        }
    }
    let expected_direction = locale_data(locale).direction;
    if matches!(
        (profile.direction, expected_direction),
        (ProfileDirection::Ltr, trox::TextDirection::Rtl)
            | (ProfileDirection::Rtl, trox::TextDirection::Ltr)
    ) {
        bail!("locale `{locale}` direction disagrees with pinned CLDR data");
    }
    Ok(profile)
}

pub fn build_catalog(
    config: &ProjectConfig,
    diagnostics: &mut Diagnostics,
) -> Result<CatalogModel> {
    build_catalog_impl(config, diagnostics).diagnostic(
        "trox.invalid-source-catalog",
        Some(config.path.clone()),
        "Correct the source or catalog contract violation and rerun the command.",
    )
}

fn build_catalog_impl(
    config: &ProjectConfig,
    diagnostics: &mut Diagnostics,
) -> Result<CatalogModel> {
    let terms = load_terms(config)?;
    let source_locale_data = locale_data(&config.source_locale);
    validate_term_category_completeness(&terms, &source_locale_data)?;
    let files = discover_source_files(config)?;
    let results: Vec<_> = files
        .par_iter()
        .map(|(path, language, default)| scan_file(path, *language, default.as_deref()))
        .collect();
    let bytes_scanned = results.iter().map(|result| result.bytes_scanned).sum();
    for result in &results {
        diagnostics.extend(result.diagnostics.clone());
    }
    let mut grouped: BTreeMap<String, Vec<ExtractedMessage>> = BTreeMap::new();
    for result in results {
        for message in result.messages {
            grouped
                .entry(message.entry_id.clone())
                .or_default()
                .push(message);
        }
    }
    let mut messages = BTreeMap::new();
    for (entry_id, calls) in grouped {
        let first = &calls[0];
        let mut descriptions = BTreeSet::new();
        let mut ron_paths = BTreeSet::new();
        let mut locations = BTreeSet::new();
        let mut selector_labels: BTreeMap<Vec<usize>, BTreeSet<String>> = BTreeMap::new();
        let mut predicate_labels = SelectorPredicateLabels::new();
        let mut term_reachability = TermReachability::new();
        for call in &calls {
            if call.source_signature != first.source_signature {
                bail!("short ID collision for `{entry_id}`");
            }
            if call.arguments != first.arguments {
                bail!("incompatible argument schemas share message `{entry_id}`; add meaning");
            }
            merge_term_reachability(&mut term_reachability, &call.term_ids);
            if let Some(description) = &call.description {
                descriptions.insert(description.clone());
            }
            if let Some(ron_path) = &call.ron_path {
                ron_paths.insert(ron_path.clone());
            }
            locations.insert(call.location.clone());
            for (path, label) in &call.selector_labels {
                selector_labels
                    .entry(path.clone())
                    .or_default()
                    .insert(label.clone());
            }
            for (path, labels) in &call.predicate_labels {
                merge_predicate_labels(&mut predicate_labels, path, labels);
            }
        }
        if descriptions.is_empty() && ron_paths.is_empty() {
            diagnostics.push(Diagnostic::warning(
                "trox.missing-ron-description",
                format!("RON message `{entry_id}` has no description"),
            ));
        }
        if descriptions.len() > 1 {
            diagnostics.push(Diagnostic::warning(
                "trox.multiple-descriptions",
                format!("message `{entry_id}` has multiple distinct descriptions"),
            ));
        }
        lint_source_message(first, &descriptions, diagnostics);
        validate_pattern_category_completeness(
            &entry_id,
            &first.identity.pattern,
            &source_locale_data,
        )?;
        validate_argument_schemas(config, &terms, &first.arguments, &term_reachability)?;
        let context = json!({
            "arguments": first.arguments,
            "descriptions": descriptions,
            "ron_paths": ron_paths,
            "predicate_labels": predicate_labels.iter().map(|(path, values)| json!({"path":path,"values":values})).collect::<Vec<_>>(),
            "selector_labels": selector_labels.iter().map(|(path, values)| json!({"path":path,"values":values})).collect::<Vec<_>>(),
            "term_forms": term_form_context(config, &first.arguments),
            "term_reachability": term_reachability,
            "terms": term_context(config, &terms, &first.arguments, &term_reachability),
        });
        let context_revision =
            revision_id(&context).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        messages.insert(
            entry_id.clone(),
            MessageEntry {
                entry_id,
                source_signature: first.source_signature.clone(),
                identity: first.identity.clone(),
                descriptions,
                ron_paths,
                arguments: first.arguments.clone(),
                term_reachability,
                selector_labels,
                predicate_labels,
                locations,
                context_revision,
            },
        );
    }
    Ok(CatalogModel {
        messages,
        terms,
        bytes_scanned,
        files_scanned: files.len(),
    })
}

fn merge_term_reachability(merged: &mut TermReachability, call: &BTreeMap<String, Option<String>>) {
    for (argument, static_id) in call {
        let entry = merged
            .entry(argument.clone())
            .or_insert_with(|| Some(BTreeSet::new()));
        match (entry.as_mut(), static_id) {
            (Some(ids), Some(id)) => {
                ids.insert(id.clone());
            }
            (_, None) => *entry = None,
            (None, Some(_)) => {}
        }
    }
}

fn merge_predicate_labels(merged: &mut SelectorPredicateLabels, path: &[usize], labels: &[String]) {
    let by_branch = merged.entry(path.to_vec()).or_default();
    for (branch, label) in labels.iter().enumerate() {
        by_branch.entry(branch).or_default().insert(label.clone());
    }
}

fn lint_source_message(
    call: &ExtractedMessage,
    descriptions: &BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) {
    let mut texts = Vec::new();
    collect_pattern_texts(&call.identity.pattern, &mut texts);
    let mut warn = |rule: &str, message: String| {
        diagnostics.push(Diagnostic::warning(rule, message).at(
            &call.location.path,
            Span {
                line: call.location.line,
                column: call.location.column,
                end_line: call.location.line,
                end_column: call.location.column + 1,
            },
        ));
    };
    if descriptions
        .iter()
        .any(|description| texts.iter().any(|text| description.trim() == text.trim()))
    {
        warn(
            "trox.description-repeats-english",
            format!(
                "message `{}` has a description that only repeats its English text",
                call.entry_id
            ),
        );
    }
    for (name, schema) in &call.arguments {
        if ["data", "item", "name", "text", "thing", "value"].contains(&name.as_str()) {
            warn(
                "trox.vague-placeholder",
                format!(
                    "message `{}` uses vague placeholder `{name}`",
                    call.entry_id
                ),
            );
        }
        if matches!(schema, ArgumentSchema::Opaque)
            && texts.iter().any(|text| {
                let lower = text.to_ascii_lowercase();
                lower.contains(&format!("a {{{name}}}"))
                    || lower.contains(&format!("an {{{name}}}"))
            })
        {
            warn(
                "trox.article-before-opaque",
                format!(
                    "message `{}` places an English article before opaque `{name}`",
                    call.entry_id
                ),
            );
        }
        if let ArgumentSchema::Term { form: None, .. } = schema {
            if texts
                .iter()
                .any(|text| text.trim() != format!("{{{name}}}"))
            {
                warn(
                    "trox.base-term-in-sentence",
                    format!(
                        "message `{}` uses the default surface of term placeholder `{name}` inside a sentence",
                        call.entry_id
                    ),
                );
            }
            if call
                .term_ids
                .get(name)
                .is_some_and(|term_id| term_id.is_some())
            {
                warn(
                    "trox.fixed-default-term",
                    format!(
                        "message `{}` uses a statically fixed term only for its default surface",
                        call.entry_id
                    ),
                );
            }
        }
    }
}

fn collect_pattern_texts<'a>(pattern: &'a Pattern, output: &mut Vec<&'a str>) {
    match pattern {
        Pattern::Text { text } => output.push(text),
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            for branch in branches {
                collect_pattern_texts(&branch.pattern, output);
            }
        }
        Pattern::Select { branches } => {
            for branch in branches {
                let pattern = match branch {
                    SelectIdentityBranch::When { pattern }
                    | SelectIdentityBranch::Otherwise { pattern } => pattern,
                };
                collect_pattern_texts(pattern, output);
            }
        }
    }
}

fn validate_term_category_completeness(terms: &TermCatalog, source: &LocaleData) -> Result<()> {
    for (term_id, term) in terms {
        for (form_id, surface) in &term.forms {
            let TermSurface::Number(values) = surface else {
                continue;
            };
            let present = values
                .iter()
                .map(|value| value.parts().0)
                .collect::<BTreeSet<_>>();
            ensure_categories_present(
                &format!("term `{term_id}` form `{form_id}`"),
                &present,
                &source.cardinal_categories,
                "cardinal",
            )?;
        }
    }
    Ok(())
}

fn validate_pattern_category_completeness(
    entry_id: &str,
    pattern: &Pattern,
    source: &LocaleData,
) -> Result<()> {
    match pattern {
        Pattern::Text { .. } => Ok(()),
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            let (expected, kind) = if matches!(pattern, Pattern::Ordinal { .. }) {
                (&source.ordinal_categories, "ordinal")
            } else {
                (&source.cardinal_categories, "cardinal")
            };
            let present = branches
                .iter()
                .filter_map(|branch| branch.key.category())
                .collect::<BTreeSet<_>>();
            ensure_categories_present(&format!("message `{entry_id}`"), &present, expected, kind)?;
            for branch in branches {
                validate_pattern_category_completeness(entry_id, &branch.pattern, source)?;
            }
            Ok(())
        }
        Pattern::Select { branches } => {
            for branch in branches {
                let child = match branch {
                    SelectIdentityBranch::When { pattern }
                    | SelectIdentityBranch::Otherwise { pattern } => pattern,
                };
                validate_pattern_category_completeness(entry_id, child, source)?;
            }
            Ok(())
        }
    }
}

fn ensure_categories_present(
    owner: &str,
    present: &BTreeSet<PluralCategory>,
    expected: &[PluralCategory],
    kind: &str,
) -> Result<()> {
    let missing = expected
        .iter()
        .filter(|category| !present.contains(category))
        .map(|category| category.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "{owner} is missing source-locale {kind} categories: {}",
            missing.join(", ")
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ResolvedTermSurface<'a> {
    Scalar(&'a str),
    Number(&'a [NumberSurface]),
    NumberFallback(&'a str),
}

fn resolve_term_surface<'a>(
    config: &ProjectConfig,
    term: &'a TermRecord,
    form: Option<&str>,
    number: bool,
) -> Option<ResolvedTermSurface<'a>> {
    let Some(form) = form else {
        return (!number).then_some(ResolvedTermSurface::Scalar(&term.value));
    };
    let declaration = config.term_forms.get(form)?;
    if (declaration.number == NumberPolicy::Required) != number {
        return None;
    }
    match term.forms.get(form) {
        Some(TermSurface::Scalar(value)) if !number => Some(ResolvedTermSurface::Scalar(value)),
        Some(TermSurface::Number(values)) if number => Some(ResolvedTermSurface::Number(values)),
        None if declaration.source_fallback == Some(SourceFallback::Default) && number => {
            Some(ResolvedTermSurface::NumberFallback(&term.value))
        }
        None if declaration.source_fallback == Some(SourceFallback::Default) => {
            Some(ResolvedTermSurface::Scalar(&term.value))
        }
        _ => None,
    }
}

fn validate_argument_schemas(
    config: &ProjectConfig,
    terms: &TermCatalog,
    arguments: &BTreeMap<String, ArgumentSchema>,
    term_reachability: &TermReachability,
) -> Result<()> {
    for (argument, schema) in arguments {
        if let ArgumentSchema::Term { form, number } = schema {
            if let Some(form) = form {
                let declaration = config.term_forms.get(form).with_context(|| {
                    format!("argument `{argument}` requests unknown term form `{form}`")
                })?;
                if (declaration.number == NumberPolicy::Required) != *number {
                    bail!("argument `{argument}` violates number policy for form `{form}`");
                }
            } else if *number {
                bail!("argument `{argument}` numbers a default term form");
            }
            if let Some(Some(static_ids)) = term_reachability.get(argument) {
                for term_id in static_ids {
                    let term = terms.get(term_id).with_context(|| {
                        format!("argument `{argument}` references unknown term `{term_id}`")
                    })?;
                    if resolve_term_surface(config, term, form.as_deref(), *number).is_none() {
                        let requested = form.as_deref().unwrap_or("default");
                        bail!("term `{term_id}` lacks compatible requested form `{requested}`");
                    }
                }
            }
        }
    }
    Ok(())
}

fn term_context(
    config: &ProjectConfig,
    terms: &TermCatalog,
    arguments: &BTreeMap<String, ArgumentSchema>,
    reachability: &TermReachability,
) -> Value {
    let mut result = BTreeMap::new();
    for (argument, schema) in arguments {
        let ArgumentSchema::Term { form, number } = schema else {
            continue;
        };
        let static_ids = reachability.get(argument).and_then(Option::as_ref);
        let term_values = terms
            .iter()
            .filter(|(term_id, term)| {
                static_ids.is_none_or(|ids| ids.contains(*term_id))
                    && resolve_term_surface(config, term, form.as_deref(), *number).is_some()
            })
            .map(|(term_id, term)| {
                let surface = match form {
                    None => json!({"kind":"default","value":term.value}),
                    Some(form_id) => match term.forms.get(form_id) {
                        Some(surface) => json!({"kind":"declared","value":surface}),
                        None => json!({"kind":"default_fallback","value":term.value}),
                    },
                };
                (
                    term_id.clone(),
                    json!({"description":term.description,"surface":surface}),
                )
            })
            .collect::<BTreeMap<_, _>>();
        result.insert(argument.clone(), term_values);
    }
    json!(result)
}

fn term_form_context(
    config: &ProjectConfig,
    arguments: &BTreeMap<String, ArgumentSchema>,
) -> Value {
    let forms = arguments
        .values()
        .filter_map(|schema| match schema {
            ArgumentSchema::Term {
                form: Some(form), ..
            } => Some(form),
            _ => None,
        })
        .filter_map(|form| {
            config
                .term_forms
                .get(form)
                .map(|declaration| (form.clone(), declaration))
        })
        .collect::<BTreeMap<_, _>>();
    json!(forms)
}

fn validate_stable_id(value: &str, label: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 96
        && value.is_ascii()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.split(['.', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if !valid {
        bail!("invalid {label} `{value}`");
    }
    Ok(())
}
fn validate_text(value: &str, label: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.nfc().collect::<String>() != value {
        bail!("{label} must be NFC");
    }
    if !crate::scanner::parse_placeholders(value)
        .map_err(anyhow::Error::msg)?
        .is_empty()
    {
        bail!("{label} cannot contain placeholders");
    }
    Ok(())
}
fn validate_description_text(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} must not be empty");
    }
    if value.nfc().collect::<String>() != value {
        bail!("{label} must be NFC");
    }
    if value.contains('\r') {
        bail!("{label} must use LF line endings");
    }
    Ok(())
}
fn category_order(category: PluralCategory) -> usize {
    [
        PluralCategory::Zero,
        PluralCategory::One,
        PluralCategory::Two,
        PluralCategory::Few,
        PluralCategory::Many,
        PluralCategory::Other,
    ]
    .iter()
    .position(|item| *item == category)
    .unwrap()
}

trait BranchCategory {
    fn category(&self) -> Option<PluralCategory>;
}
impl BranchCategory for trox::NumericBranchKey {
    fn category(&self) -> Option<PluralCategory> {
        match self {
            Self::Plural { plural } => Some(*plural),
            Self::Ordinal { ordinal } => Some(*ordinal),
            Self::Exact { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
