//! Deterministic source and target bundle construction.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use trox::{
    Bundle, BundleEntry, BundleRow, BundleTerm, BundleTermForm, BundleTermSurface, IsolationPolicy,
    TextDirection, Version,
};

use crate::cldr::{CLDR_VERSION, locale_data};
use crate::config::{NumberPolicy, ProjectConfig, SourceFallback};
use crate::csv_workflow::CsvDocument;
use crate::diagnostic::DiagnosticResultExt;
use crate::extract::{
    CatalogModel, ExpectedRow, FacetScope, LocaleProfile, ProfileDirection, ProfileIsolation,
    TermRowDescriptor, TermSurface,
};

pub struct LocaleArtifacts<'a> {
    pub profile: &'a LocaleProfile,
    pub expected: &'a [ExpectedRow],
    pub csv: &'a CsvDocument,
}

pub fn source_fingerprint(config: &ProjectConfig, model: &CatalogModel) -> Result<String> {
    let descriptor = serde_json::json!({
        "source_locale": config.source_locale,
        "identity_version": 1,
        "terms": model.terms,
        "term_forms": config.term_forms.iter().map(|(id, form)| (id, serde_json::json!({"description":form.description,"number":format!("{:?}",form.number),"source_fallback":format!("{:?}",form.source_fallback)}))).collect::<BTreeMap<_,_>>(),
        "entries": model.messages.iter().map(|(id, entry)| (id, serde_json::json!({"signature":entry.source_signature,"context_revision":entry.context_revision}))).collect::<BTreeMap<_,_>>(),
    });
    Ok(trox::revision_id(&descriptor)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?
        .trim_start_matches("rev1_")
        .to_owned())
}

pub fn build_source_bundle(config: &ProjectConfig, model: &CatalogModel) -> Result<Bundle> {
    build_source_bundle_impl(config, model).diagnostic(
        "trox.source-bundle-build",
        Some(config.resolve(&config.source_bundle)),
        "Correct the source catalog before rebuilding the source bundle.",
    )
}

fn build_source_bundle_impl(config: &ProjectConfig, model: &CatalogModel) -> Result<Bundle> {
    let data = locale_data(&config.source_locale);
    let fingerprint = source_fingerprint(config, model)?;
    let entries = model
        .messages
        .iter()
        .map(|(id, entry)| {
            Ok((
                id.clone(),
                BundleEntry {
                    arguments: Some(entry.arguments.clone()),
                    contract_signature: Some(trox::contract_signature(
                        &entry.identity,
                        &entry.arguments,
                    )?),
                    identity: Some(entry.identity.clone()),
                    rows: BTreeMap::new(),
                    source_signature: entry.source_signature.clone(),
                },
            ))
        })
        .collect::<Result<_>>()?;
    let terms = source_terms(config, model)?;
    let bundle = Bundle {
        cldr_version: CLDR_VERSION.into(),
        direction: data.direction,
        entries,
        fallback_chain: vec![],
        fallbacks_flattened: true,
        format: "trox-bundle".into(),
        isolation: IsolationPolicy::Isolate,
        locale: config.source_locale.clone(),
        message_facets: vec![],
        number_format: data.number_format,
        plural_rules: data.rules,
        source_catalog_fingerprint: fingerprint,
        source_locale: config.source_locale.clone(),
        terms,
        version: Version::V1_1,
    };
    bundle.validate()?;
    Ok(bundle)
}

fn source_terms(
    config: &ProjectConfig,
    model: &CatalogModel,
) -> Result<BTreeMap<String, BundleTerm>> {
    let mut terms = BTreeMap::new();
    for (term_id, term) in &model.terms {
        let mut forms = BTreeMap::new();
        forms.insert(
            "$default".into(),
            BundleTermForm::Scalar {
                origin_locale: config.source_locale.clone(),
                text: term.value.clone(),
            },
        );
        for (form_id, declaration) in &config.term_forms {
            match term.forms.get(form_id) {
                Some(TermSurface::Scalar(text)) => {
                    forms.insert(
                        form_id.clone(),
                        BundleTermForm::Scalar {
                            origin_locale: config.source_locale.clone(),
                            text: text.clone(),
                        },
                    );
                }
                Some(TermSurface::Number(values)) => {
                    let values = values
                        .iter()
                        .map(|value| {
                            let (category, text) = value.parts();
                            (
                                category,
                                BundleTermSurface {
                                    origin_locale: config.source_locale.clone(),
                                    text: text.into(),
                                },
                            )
                        })
                        .collect();
                    forms.insert(form_id.clone(), BundleTermForm::Number { values });
                }
                None if declaration.source_fallback == Some(SourceFallback::Default)
                    && declaration.number == NumberPolicy::Forbidden =>
                {
                    forms.insert(
                        form_id.clone(),
                        BundleTermForm::Scalar {
                            origin_locale: config.source_locale.clone(),
                            text: term.value.clone(),
                        },
                    );
                }
                None if declaration.source_fallback == Some(SourceFallback::Default)
                    && declaration.number == NumberPolicy::Required =>
                {
                    forms.insert(
                        form_id.clone(),
                        BundleTermForm::Number {
                            values: BTreeMap::from([(
                                trox::PluralCategory::Other,
                                BundleTermSurface {
                                    origin_locale: config.source_locale.clone(),
                                    text: term.value.clone(),
                                },
                            )]),
                        },
                    );
                }
                None => {}
            }
        }
        terms.insert(
            term_id.clone(),
            BundleTerm {
                facets: BTreeMap::new(),
                forms,
            },
        );
    }
    Ok(terms)
}

pub fn build_target_bundle(
    config: &ProjectConfig,
    model: &CatalogModel,
    locale: &str,
    artifacts: &LocaleArtifacts<'_>,
    parents: &BTreeMap<String, LocaleArtifacts<'_>>,
    allow_missing: bool,
) -> Result<Bundle> {
    build_target_bundle_impl(config, model, locale, artifacts, parents, allow_missing).diagnostic(
        "trox.target-bundle-build",
        config
            .locales
            .get(locale)
            .map(|target| config.resolve(&target.bundle)),
        "Complete or correct the locale translations and rebuild the target bundle.",
    )
}

fn build_target_bundle_impl(
    config: &ProjectConfig,
    model: &CatalogModel,
    locale: &str,
    artifacts: &LocaleArtifacts<'_>,
    parents: &BTreeMap<String, LocaleArtifacts<'_>>,
    allow_missing: bool,
) -> Result<Bundle> {
    let fingerprint = source_fingerprint(config, model)?;
    let data = locale_data(locale);
    let own = approved_translations(artifacts.csv)?;
    let parent_translations: BTreeMap<_, _> = parents
        .iter()
        .map(|(locale, artifacts)| Ok((locale.clone(), approved_translations(artifacts.csv)?)))
        .collect::<Result<_>>()?;
    let own_expected: BTreeMap<_, _> = artifacts
        .expected
        .iter()
        .map(|row| (row.row_id.as_str(), row))
        .collect();
    let mut entries: BTreeMap<String, BundleEntry> = model
        .messages
        .iter()
        .map(|(id, entry)| {
            Ok((
                id.clone(),
                BundleEntry {
                    arguments: None,
                    contract_signature: Some(trox::contract_signature(
                        &entry.identity,
                        &entry.arguments,
                    )?),
                    identity: None,
                    rows: BTreeMap::new(),
                    source_signature: entry.source_signature.clone(),
                },
            ))
        })
        .collect::<Result<_>>()?;
    let mut missing = Vec::new();
    for row in artifacts
        .expected
        .iter()
        .filter(|row| row.kind == "message")
    {
        let resolved = own
            .get(&row.row_id)
            .map(|text| (locale.to_owned(), text.clone()))
            .or_else(|| resolve_parent(row, artifacts.profile, parents, &parent_translations));
        if let Some((origin_locale, translation)) = resolved {
            entries
                .get_mut(&row.entry_id)
                .expect("known message entry")
                .rows
                .insert(
                    row.row_id.clone(),
                    BundleRow {
                        expansion: row.expansion.clone(),
                        origin_locale,
                        translation,
                    },
                );
        } else if !allow_missing {
            missing.push(row.row_id.clone());
        }
    }
    if !missing.is_empty() {
        bail!(
            "locale `{locale}` has {} missing or stale message translations (first: {})",
            missing.len(),
            missing[0]
        );
    }
    // Empty entries are retained: they carry compatibility signatures and fall back entry-wise.
    let terms = target_terms(TargetTermBuild {
        config,
        model,
        locale,
        artifacts,
        parents,
        own: &own,
        parent_translations: &parent_translations,
        allow_missing,
    })?;
    let direction = match artifacts.profile.direction {
        ProfileDirection::Ltr => TextDirection::Ltr,
        ProfileDirection::Rtl => TextDirection::Rtl,
    };
    let isolation = match artifacts.profile.isolation {
        ProfileIsolation::Isolate => IsolationPolicy::Isolate,
        ProfileIsolation::None => IsolationPolicy::None,
    };
    let message_facets = artifacts
        .profile
        .facets
        .iter()
        .filter(|(_, facet)| facet.scope == FacetScope::Message)
        .map(|(id, _)| id.clone())
        .collect();
    let bundle = Bundle {
        cldr_version: CLDR_VERSION.into(),
        direction,
        entries,
        fallback_chain: artifacts.profile.fallbacks.clone(),
        fallbacks_flattened: true,
        format: "trox-bundle".into(),
        isolation,
        locale: locale.into(),
        message_facets,
        number_format: data.number_format,
        plural_rules: data.rules,
        source_catalog_fingerprint: fingerprint,
        source_locale: config.source_locale.clone(),
        terms,
        version: Version::V1_1,
    };
    bundle.validate()?;
    let _ = own_expected; // Kept to make the target row authority explicit during review.
    Ok(bundle)
}

fn approved_translations(csv: &CsvDocument) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let mut previous: Option<(&str, String)> = None;
    for row in csv.rows.iter().filter(|row| row.status != "obsolete") {
        let resolved = if row.status != "translated" {
            None
        } else if row.translation == "^" {
            previous
                .as_ref()
                .filter(|(entry, _)| *entry == row.entry_id)
                .map(|(_, text)| text.clone())
        } else if row.translation.is_empty() {
            None
        } else {
            Some(row.translation.clone())
        };
        if let Some(text) = resolved {
            result.insert(row.row_id.clone(), text.clone());
            previous = Some((&row.entry_id, text));
        } else {
            previous = Some((&row.entry_id, String::new()));
        }
    }
    Ok(result)
}

fn resolve_parent(
    target: &ExpectedRow,
    target_profile: &LocaleProfile,
    parents: &BTreeMap<String, LocaleArtifacts<'_>>,
    translations: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<(String, String)> {
    for parent_locale in target_profile
        .fallbacks
        .iter()
        .filter(|locale| parents.contains_key(*locale))
    {
        let artifacts = &parents[parent_locale];
        let approved = &translations[parent_locale];
        let mut candidates: Vec<_> = artifacts
            .expected
            .iter()
            .filter(|row| row.entry_id == target.entry_id && row.kind == target.kind)
            .filter_map(|row| {
                expansion_compatibility(&target.expansion.path, &row.expansion.path)
                    .map(|rank| (rank, row))
            })
            .collect();
        candidates.sort_by_key(|(rank, row)| (*rank, &row.row_id));
        if let Some((_, row)) = candidates
            .into_iter()
            .find(|(_, row)| approved.contains_key(&row.row_id))
        {
            return Some((parent_locale.clone(), approved[&row.row_id].clone()));
        }
    }
    None
}

fn expansion_compatibility(target: &[Value], parent: &[Value]) -> Option<usize> {
    if target.len() != parent.len() {
        return None;
    }
    let mut rank = 0;
    for (target, parent) in target.iter().zip(parent) {
        let target_obj = target.as_object()?;
        let parent_obj = parent.as_object()?;
        if target_obj.get("kind") != parent_obj.get("kind") {
            return None;
        }
        let kind = target_obj.get("kind")?.as_str()?;
        match kind {
            "plural" | "ordinal" => {
                let target_match = target_obj.get("match")?.as_object()?;
                let parent_match = parent_obj.get("match")?.as_object()?;
                if let Some(exact) = target_match.get("exact") {
                    if parent_match.get("exact") != Some(exact) {
                        return None;
                    }
                } else {
                    let target_category = target_match.get("category")?;
                    let parent_category = parent_match.get("category")?;
                    if parent_category != target_category {
                        if parent_category.as_str() == Some("other") {
                            rank += 1;
                        } else {
                            return None;
                        }
                    }
                }
            }
            "term_number" => {
                if target_obj.get("form") != parent_obj.get("form") {
                    return None;
                }
                let target_category = target_obj.get("category")?;
                let parent_category = parent_obj.get("category")?;
                if parent_category != target_category {
                    if parent_category.as_str() == Some("other") {
                        rank += 1;
                    } else {
                        return None;
                    }
                }
            }
            _ if target != parent => return None,
            _ => {}
        }
    }
    Some(rank)
}

struct TargetTermBuild<'a> {
    config: &'a ProjectConfig,
    model: &'a CatalogModel,
    locale: &'a str,
    artifacts: &'a LocaleArtifacts<'a>,
    parents: &'a BTreeMap<String, LocaleArtifacts<'a>>,
    own: &'a BTreeMap<String, String>,
    parent_translations: &'a BTreeMap<String, BTreeMap<String, String>>,
    allow_missing: bool,
}

fn target_terms(build: TargetTermBuild<'_>) -> Result<BTreeMap<String, BundleTerm>> {
    let TargetTermBuild {
        config,
        model,
        locale,
        artifacts,
        parents,
        own,
        parent_translations,
        allow_missing,
    } = build;
    let mut result = BTreeMap::new();
    for term_id in model.terms.keys() {
        let entry_id = format!("term:{term_id}");
        let expected: Vec<_> = artifacts
            .expected
            .iter()
            .filter(|row| row.entry_id == entry_id)
            .collect();
        let mut forms = BTreeMap::new();
        for row in expected {
            let resolved = own
                .get(&row.row_id)
                .map(|text| (locale.to_owned(), text.clone()))
                .or_else(|| resolve_parent(row, artifacts.profile, parents, parent_translations));
            let Some((origin_locale, text)) = resolved else {
                if allow_missing {
                    continue;
                }
                bail!("locale `{locale}` is missing term row `{}`", row.row_id);
            };
            insert_term_row(&mut forms, row, origin_locale, text)?;
        }
        // Locale-wide aliases materialize the already resolved default record.
        if let Some(aliases) = artifacts.profile.term_form_aliases.get(term_id) {
            for form_id in aliases.keys() {
                if let Some(default) = forms.get("$default").cloned() {
                    let declaration = &config.term_forms[form_id];
                    forms.insert(
                        form_id.clone(),
                        materialize_default_alias(
                            &default,
                            declaration.number,
                            &locale_data(locale).cardinal_categories,
                        )?,
                    );
                }
            }
        }
        let facets = artifacts
            .profile
            .term_facets
            .get(term_id)
            .cloned()
            .unwrap_or_default();
        result.insert(term_id.clone(), BundleTerm { facets, forms });
    }
    Ok(result)
}

fn materialize_default_alias(
    default: &BundleTermForm,
    number: NumberPolicy,
    categories: &[trox::PluralCategory],
) -> Result<BundleTermForm> {
    let BundleTermForm::Scalar {
        origin_locale,
        text,
    } = default
    else {
        bail!("term default alias source must be scalar");
    };
    Ok(match number {
        NumberPolicy::Forbidden => default.clone(),
        NumberPolicy::Required => BundleTermForm::Number {
            values: categories
                .iter()
                .map(|category| {
                    (
                        *category,
                        BundleTermSurface {
                            origin_locale: origin_locale.clone(),
                            text: text.clone(),
                        },
                    )
                })
                .collect(),
        },
    })
}

fn insert_term_row(
    forms: &mut BTreeMap<String, BundleTermForm>,
    row: &ExpectedRow,
    origin_locale: String,
    text: String,
) -> Result<()> {
    let descriptor = row
        .term
        .as_ref()
        .with_context(|| format!("term row `{}` lacks typed metadata", row.row_id))?;
    match descriptor {
        TermRowDescriptor::Default => {
            forms.insert(
                "$default".into(),
                BundleTermForm::Scalar {
                    origin_locale,
                    text,
                },
            );
        }
        TermRowDescriptor::Number { form, category } => {
            let form = forms
                .entry(form.clone())
                .or_insert_with(|| BundleTermForm::Number {
                    values: BTreeMap::new(),
                });
            let BundleTermForm::Number { values } = form else {
                bail!("term row `{}` changes form kind", row.row_id);
            };
            values.insert(
                *category,
                BundleTermSurface {
                    origin_locale,
                    text,
                },
            );
        }
        TermRowDescriptor::Scalar { form } => {
            forms.insert(
                form.clone(),
                BundleTermForm::Scalar {
                    origin_locale,
                    text,
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LintConfig, TermFormDeclaration};
    use crate::extract::{MessageEntry, TermRecord};
    use std::collections::BTreeSet;
    use trox::{ArgumentSchema, IdentityDescriptor, Pattern, PluralCategory, identity_ids};

    #[test]
    fn source_bundle_emits_entry_argument_schemas() {
        let identity = IdentityDescriptor {
            identity_version: 1,
            meaning: None,
            pattern: Pattern::Text {
                text: "Hello {name}".into(),
            },
        };
        let (entry_id, source_signature) = identity_ids(&identity).unwrap();
        let arguments = BTreeMap::from([("name".into(), ArgumentSchema::Scalar)]);
        let model = CatalogModel {
            messages: BTreeMap::from([(
                entry_id.clone(),
                MessageEntry {
                    entry_id: entry_id.clone(),
                    source_signature,
                    identity,
                    descriptions: BTreeSet::from(["Greeting with a name.".into()]),
                    ron_paths: BTreeSet::new(),
                    arguments: arguments.clone(),
                    term_reachability: BTreeMap::new(),
                    selector_labels: BTreeMap::new(),
                    predicate_labels: BTreeMap::new(),
                    locations: BTreeSet::new(),
                    context_revision: "rev1_test".into(),
                },
            )]),
            terms: BTreeMap::new(),
            source_locale_data: locale_data("en-US"),
            bytes_scanned: 0,
            files_scanned: 0,
        };
        let config = ProjectConfig {
            source_locale: "en-US".into(),
            terms: "terms.ron".into(),
            source_bundle: "source.json".into(),
            source_report: None,
            sources: vec![],
            locales: BTreeMap::new(),
            max_expanded_rows_per_entry: 256,
            lint: LintConfig::default(),
            term_forms: BTreeMap::new(),
            ron_description_defaults: BTreeMap::new(),
            root: Default::default(),
            path: Default::default(),
        };

        let bundle = build_source_bundle_impl(&config, &model).unwrap();
        assert_eq!(
            bundle.entries[&entry_id].arguments.as_ref(),
            Some(&arguments)
        );
        assert_eq!(bundle.version, Version::V1_1);
        assert_eq!(
            bundle.entries[&entry_id].contract_signature.as_deref(),
            Some(
                trox::contract_signature(
                    bundle.entries[&entry_id].identity.as_ref().unwrap(),
                    &arguments,
                )
                .unwrap()
                .as_str()
            ),
        );
    }

    #[test]
    fn parent_plural_rows_use_same_category_then_other_without_crossing_exact_values() {
        let target_few = vec![serde_json::json!({
            "branch": 1,
            "kind": "plural",
            "match": { "category": "few" }
        })];
        let parent_few = target_few.clone();
        let parent_other = vec![serde_json::json!({
            "branch": 1,
            "kind": "plural",
            "match": { "category": "other" }
        })];
        assert_eq!(expansion_compatibility(&target_few, &parent_few), Some(0));
        assert_eq!(expansion_compatibility(&target_few, &parent_other), Some(1));

        let target_exact = vec![serde_json::json!({
            "branch": 0,
            "kind": "plural",
            "match": { "exact": 0 }
        })];
        assert_eq!(expansion_compatibility(&target_exact, &parent_other), None);
    }

    #[test]
    fn numbered_term_rows_use_same_category_then_other() {
        let target = vec![serde_json::json!({
            "category": "few",
            "form": "counted",
            "kind": "term_number"
        })];
        let same = target.clone();
        let other = vec![serde_json::json!({
            "category": "other",
            "form": "counted",
            "kind": "term_number"
        })];
        let wrong_form = vec![serde_json::json!({
            "category": "other",
            "form": "indefinite",
            "kind": "term_number"
        })];
        assert_eq!(expansion_compatibility(&target, &same), Some(0));
        assert_eq!(expansion_compatibility(&target, &other), Some(1));
        assert_eq!(expansion_compatibility(&target, &wrong_form), None);
    }

    #[test]
    fn term_bundle_semantics_do_not_depend_on_display_conditions() {
        let row = ExpectedRow {
            conditions: "this display label may change freely".into(),
            english: String::new(),
            description: String::new(),
            placeholders: String::new(),
            entry_id: "term:card".into(),
            row_id: "row1_test".into(),
            kind: "term_form".into(),
            source_locations: String::new(),
            source_revision: String::new(),
            expansion: trox::ExpansionDescriptor {
                entry_signature: String::new(),
                path: vec![],
            },
            term: Some(TermRowDescriptor::Number {
                form: "counted".into(),
                category: PluralCategory::Few,
            }),
        };
        let mut forms = BTreeMap::new();
        insert_term_row(&mut forms, &row, "ru".into(), "воина".into()).unwrap();
        let BundleTermForm::Number { values } = &forms["counted"] else {
            panic!("expected numbered form");
        };
        assert_eq!(values[&PluralCategory::Few].text, "воина");
    }

    #[test]
    fn numbered_source_fallback_materializes_other_from_default_text() {
        let config = ProjectConfig {
            source_locale: "en-US".into(),
            terms: "terms.ron".into(),
            source_bundle: "source.json".into(),
            source_report: None,
            sources: vec![],
            locales: BTreeMap::new(),
            max_expanded_rows_per_entry: 256,
            lint: LintConfig::default(),
            term_forms: BTreeMap::from([(
                "counted".into(),
                TermFormDeclaration {
                    description: "Counted form.".into(),
                    number: NumberPolicy::Required,
                    source_fallback: Some(SourceFallback::Default),
                },
            )]),
            ron_description_defaults: BTreeMap::new(),
            root: Default::default(),
            path: Default::default(),
        };
        let model = CatalogModel {
            messages: BTreeMap::new(),
            terms: BTreeMap::from([(
                "unit.card".into(),
                TermRecord {
                    description: None,
                    value: "card".into(),
                    forms: BTreeMap::new(),
                },
            )]),
            source_locale_data: locale_data("en-US"),
            bytes_scanned: 0,
            files_scanned: 0,
        };

        let terms = source_terms(&config, &model).unwrap();
        let BundleTermForm::Number { values } = &terms["unit.card"].forms["counted"] else {
            panic!("expected numbered source fallback");
        };
        assert_eq!(values[&PluralCategory::Other].text, "card");
    }

    #[test]
    fn numbered_locale_alias_materializes_numbered_target_surfaces() {
        let default = BundleTermForm::Scalar {
            origin_locale: "ja".into(),
            text: "カード".into(),
        };
        let aliased =
            materialize_default_alias(&default, NumberPolicy::Required, &[PluralCategory::Other])
                .unwrap();
        let BundleTermForm::Number { values } = aliased else {
            panic!("numbered aliases must preserve the number contract");
        };
        assert_eq!(values[&PluralCategory::Other].text, "カード");
        assert_eq!(values[&PluralCategory::Other].origin_locale, "ja");
    }
}
