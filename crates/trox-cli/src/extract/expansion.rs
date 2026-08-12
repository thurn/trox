//! Locale-specific expansion of message and term rows.

use super::*;

fn translator_description(description: &str, meaning: Option<&str>, conditions: &str) -> String {
    let mut sections = Vec::new();
    if let Some(meaning) = meaning {
        sections.push(format!("Meaning: {meaning}"));
    }
    if !description.is_empty() {
        sections.push(description.to_owned());
    }
    if !conditions.is_empty() {
        sections.push(format!("Conditions: {conditions}"));
    }
    sections.join("\n\n")
}

fn ensure_structural_cap(config: &ProjectConfig, entry_id: &str, rows: usize) -> Result<()> {
    if rows > config.max_expanded_rows_per_entry {
        bail!(
            "entry `{entry_id}` expands to {rows} rows, over configured cap {}",
            config.max_expanded_rows_per_entry
        );
    }
    Ok(())
}

fn enforce_entry_row_limits(
    config: &ProjectConfig,
    entry_id: &str,
    rows: usize,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    ensure_structural_cap(config, entry_id, rows)?;
    if rows > 32 && config.lint.level("trox.human-row-expansion") != crate::config::LintLevel::Allow
    {
        bail!(
            "entry `{entry_id}` expands to {rows} human rows; a reasoned allow is required above 32"
        );
    }
    let warning = if rows > 16 {
        Some("trox.human-row-expansion-strong")
    } else if rows > 8 {
        Some("trox.human-row-expansion")
    } else {
        None
    };
    if let Some(rule) = warning {
        diagnostics.push(Diagnostic::warning(
            rule,
            format!("entry `{entry_id}` expands to {rows} rows"),
        ));
    }
    Ok(())
}

pub fn expand_rows(
    config: &ProjectConfig,
    model: &CatalogModel,
    locale: &str,
    profile: &LocaleProfile,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<ExpectedRow>> {
    let path = config
        .locales
        .get(locale)
        .map(|locale| config.resolve(&locale.profile))
        .or_else(|| Some(config.path.clone()));
    expand_rows_impl(config, model, locale, profile, diagnostics).diagnostic(
        "trox.invalid-row-expansion",
        path,
        "Correct the locale facets, term forms, or selector expansion and rerun the command.",
    )
}

fn expand_rows_impl(
    config: &ProjectConfig,
    model: &CatalogModel,
    locale: &str,
    profile: &LocaleProfile,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<ExpectedRow>> {
    validate_profile_terms(config, model, profile)?;
    let data = locale_data(locale);
    let mut rows = Vec::new();
    for entry in model.messages.values() {
        let mut message_rows = expand_message(config, entry, model, profile, &data)?;
        enforce_entry_row_limits(config, &entry.entry_id, message_rows.len(), diagnostics)?;
        rows.append(&mut message_rows);
    }
    rows.extend(expand_term_rows(
        config,
        model,
        profile,
        &data,
        diagnostics,
    )?);
    Ok(rows)
}

fn validate_profile_terms(
    config: &ProjectConfig,
    model: &CatalogModel,
    profile: &LocaleProfile,
) -> Result<()> {
    for (term_id, classifications) in &profile.term_facets {
        if !model.terms.contains_key(term_id) {
            bail!(
                "locale `{}` classifies unknown term `{term_id}`",
                profile.locale
            );
        }
        for (facet_id, value) in classifications {
            let facet = profile.facets.get(facet_id).with_context(|| {
                format!(
                    "locale `{}` classifies term `{term_id}` for unknown facet `{facet_id}`",
                    profile.locale
                )
            })?;
            if !facet.values.contains(value) {
                bail!("term `{term_id}` has unknown `{facet_id}` value `{value}`");
            }
        }
    }
    for term_id in profile.term_form_aliases.keys() {
        if !model.terms.contains_key(term_id) {
            bail!(
                "locale `{}` aliases forms for unknown term `{term_id}`",
                profile.locale
            );
        }
    }
    for term_id in model.terms.keys() {
        for (facet_id, facet) in &profile.facets {
            let value = profile
                .term_facets
                .get(term_id)
                .and_then(|values| values.get(facet_id))
                .with_context(|| {
                    format!(
                        "locale `{}` does not classify term `{term_id}` for facet `{facet_id}`",
                        profile.locale
                    )
                })?;
            if !facet.values.contains(value) {
                bail!("term `{term_id}` has unknown `{facet_id}` value `{value}`");
            }
        }
        if let Some(aliases) = profile.term_form_aliases.get(term_id) {
            for form in aliases.keys() {
                let declaration = config
                    .term_forms
                    .get(form)
                    .with_context(|| format!("locale alias references unknown form `{form}`"))?;
                let term = &model.terms[term_id];
                if resolve_term_surface(
                    config,
                    term,
                    Some(form),
                    declaration.number == NumberPolicy::Required,
                )
                .is_none()
                {
                    bail!("term `{term_id}` cannot alias incompatible form `{form}`");
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct ExpandedLeaf {
    pub(super) path: Vec<Value>,
    pub(super) conditions: Vec<String>,
    pub(super) english: String,
}

fn expand_message(
    config: &ProjectConfig,
    entry: &MessageEntry,
    model: &CatalogModel,
    profile: &LocaleProfile,
    data: &LocaleData,
) -> Result<Vec<ExpectedRow>> {
    let mut leaves = Vec::new();
    PatternExpander {
        config,
        entry,
        data,
        output: &mut leaves,
    }
    .expand(
        &entry.identity.pattern,
        &mut Vec::new(),
        Vec::new(),
        Vec::new(),
    )?;
    leaves = expand_facets(config, leaves, entry, model, profile)?;
    let locale_context = locale_revision_context(config, entry, model, profile);
    let placeholders = entry
        .arguments
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let authored_description = entry
        .descriptions
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let source_locations = entry
        .locations
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    leaves
        .into_iter()
        .map(|leaf| {
            let expansion = ExpansionDescriptor {
                entry_signature: entry.source_signature.clone(),
                path: leaf.path,
            };
            let row_id =
                expansion_row_id(&expansion).map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let revision = json!({
                "context_revision": entry.context_revision,
                "expansion": expansion,
                "locale_context": locale_context,
            });
            let source_revision =
                revision_id(&revision).map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let conditions = leaf.conditions.join("; ");
            let description = translator_description(
                &authored_description,
                entry.identity.meaning.as_deref(),
                &conditions,
            );
            Ok(ExpectedRow {
                conditions,
                english: leaf.english,
                description,
                placeholders: placeholders.clone(),
                entry_id: entry.entry_id.clone(),
                row_id,
                kind: "message".into(),
                source_locations: source_locations.clone(),
                source_revision,
                expansion,
                term: None,
            })
        })
        .collect()
}

pub(super) fn locale_revision_context(
    config: &ProjectConfig,
    entry: &MessageEntry,
    model: &CatalogModel,
    profile: &LocaleProfile,
) -> Value {
    let mut arguments = BTreeMap::new();
    for (argument, schema) in &entry.arguments {
        let ArgumentSchema::Term { form, number } = schema else {
            continue;
        };
        let reachable = model
            .terms
            .iter()
            .filter(|(term_id, term)| {
                entry
                    .term_reachability
                    .get(argument)
                    .and_then(Option::as_ref)
                    .is_none_or(|ids| ids.contains(*term_id))
                    && resolve_term_surface(config, term, form.as_deref(), *number).is_some()
            })
            .map(|(term_id, _)| term_id.clone())
            .collect::<BTreeSet<_>>();
        let mut facets = BTreeMap::new();
        for (facet_id, facet) in &profile.facets {
            if facet.scope != FacetScope::Message {
                continue;
            }
            let classifications = reachable
                .iter()
                .filter_map(|term_id| {
                    profile
                        .term_facets
                        .get(term_id)
                        .and_then(|values| values.get(facet_id))
                        .map(|value| (term_id.clone(), value.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            if !classifications.is_empty() {
                facets.insert(
                    facet_id.clone(),
                    json!({
                        "scope": facet.scope,
                        "values": facet.values.iter().collect::<BTreeSet<_>>(),
                        "classifications": classifications,
                    }),
                );
            }
        }
        let aliases = form
            .as_ref()
            .map(|form_id| {
                reachable
                    .iter()
                    .filter(|term_id| {
                        profile
                            .term_form_aliases
                            .get(*term_id)
                            .is_some_and(|forms| forms.contains_key(form_id))
                    })
                    .cloned()
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        arguments.insert(
            argument.clone(),
            json!({"facets": facets, "form_aliases_to_default": aliases}),
        );
    }
    json!(arguments)
}

struct PatternExpander<'a> {
    config: &'a ProjectConfig,
    entry: &'a MessageEntry,
    data: &'a LocaleData,
    output: &'a mut Vec<ExpandedLeaf>,
}

impl PatternExpander<'_> {
    fn expand(
        &mut self,
        pattern: &Pattern,
        structural: &mut Vec<usize>,
        path: Vec<Value>,
        conditions: Vec<String>,
    ) -> Result<()> {
        match pattern {
            Pattern::Text { text } => {
                ensure_structural_cap(self.config, &self.entry.entry_id, self.output.len() + 1)?;
                self.output.push(ExpandedLeaf {
                    path,
                    conditions,
                    english: text.clone(),
                });
            }
            Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
                let ordinal = matches!(pattern, Pattern::Ordinal { .. });
                let kind = if ordinal { "ordinal" } else { "plural" };
                let categories = if ordinal {
                    &self.data.ordinal_categories
                } else {
                    &self.data.cardinal_categories
                };
                let label = selector_label(self.entry, structural);
                let mut decisions: Vec<(usize, Value, String)> = Vec::new();
                if categories.contains(&PluralCategory::Other) {
                    let branch = branch_for_category(branches, PluralCategory::Other);
                    decisions.push((
                        branch,
                        json!({"branch":branch,"kind":kind,"match":{"category":"other"}}),
                        format!("{label}.{kind}=other"),
                    ));
                }
                for (index, branch) in branches.iter().enumerate().filter(|(_, branch)| {
                    matches!(branch.key, trox::NumericBranchKey::Exact { .. })
                }) {
                    if let trox::NumericBranchKey::Exact { exact } = branch.key {
                        decisions.push((
                            index,
                            json!({"branch":index,"kind":kind,"match":{"exact":exact}}),
                            format!("{label}={exact}"),
                        ));
                    }
                }
                for category in [
                    PluralCategory::Zero,
                    PluralCategory::One,
                    PluralCategory::Two,
                    PluralCategory::Few,
                    PluralCategory::Many,
                ] {
                    if categories.contains(&category) {
                        let branch = branch_for_category(branches, category);
                        decisions.push((
                        branch,
                        json!({"branch":branch,"kind":kind,"match":{"category":category.as_str()}}),
                        format!("{label}.{kind}={}", category.as_str()),
                    ));
                    }
                }
                for (branch, step, condition) in decisions {
                    let mut child_path = path.clone();
                    child_path.push(step);
                    let mut child_conditions = conditions.clone();
                    child_conditions.push(condition);
                    structural.push(branch);
                    self.expand(
                        &branches[branch].pattern,
                        structural,
                        child_path,
                        child_conditions,
                    )?;
                    structural.pop();
                }
            }
            Pattern::Select { branches } => {
                let label = self
                    .entry
                    .selector_labels
                    .get(structural)
                    .and_then(|labels| labels.first())
                    .cloned()
                    .unwrap_or_else(|| "selector".into());
                let predicates = self.entry.predicate_labels.get(structural);
                let order = std::iter::once(branches.len() - 1).chain(0..branches.len() - 1);
                for branch in order {
                    let mut child_path = path.clone();
                    child_path.push(json!({"branch":branch,"kind":"select"}));
                    let mut child_conditions = conditions.clone();
                    let predicate = predicates
                        .and_then(|by_branch| by_branch.get(&branch))
                        .map(|labels| labels.iter().cloned().collect::<Vec<_>>().join(" | "))
                        .unwrap_or_else(|| "when".into());
                    child_conditions.push(if branch == branches.len() - 1 {
                        format!("{label}.select[{branch}]=otherwise")
                    } else {
                        format!("{label}.select[{branch}]={predicate}")
                    });
                    structural.push(branch);
                    let pattern = match &branches[branch] {
                        SelectIdentityBranch::When { pattern }
                        | SelectIdentityBranch::Otherwise { pattern } => pattern,
                    };
                    self.expand(pattern, structural, child_path, child_conditions)?;
                    structural.pop();
                }
            }
        }
        Ok(())
    }
}

fn selector_label(entry: &MessageEntry, structural: &[usize]) -> String {
    entry
        .selector_labels
        .get(structural)
        .and_then(|labels| labels.first())
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "selector_{}",
                structural
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("_")
            )
        })
}

fn branch_for_category(branches: &[NumericBranch], category: PluralCategory) -> usize {
    branches
        .iter()
        .position(|branch| branch.key.category() == Some(category))
        .or_else(|| {
            branches
                .iter()
                .position(|branch| branch.key.category() == Some(PluralCategory::Other))
        })
        .expect("validated other")
}

pub(super) fn expand_facets(
    config: &ProjectConfig,
    mut leaves: Vec<ExpandedLeaf>,
    entry: &MessageEntry,
    model: &CatalogModel,
    profile: &LocaleProfile,
) -> Result<Vec<ExpandedLeaf>> {
    for (argument, schema) in &entry.arguments {
        let ArgumentSchema::Term { form, number } = schema else {
            continue;
        };
        let compatible = model
            .terms
            .iter()
            .filter(|(term_id, term)| {
                entry
                    .term_reachability
                    .get(argument)
                    .and_then(Option::as_ref)
                    .is_none_or(|ids| ids.contains(*term_id))
                    && resolve_term_surface(config, term, form.as_deref(), *number).is_some()
            })
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        for (facet_id, facet) in &profile.facets {
            if facet.scope != FacetScope::Message {
                continue;
            }
            let reachable = compatible
                .iter()
                .filter_map(|term_id| {
                    profile
                        .term_facets
                        .get(*term_id)
                        .and_then(|facets| facets.get(facet_id))
                })
                .collect::<BTreeSet<_>>();
            let ordered = facet
                .values
                .iter()
                .filter(|value| reachable.contains(value))
                .collect::<Vec<_>>();
            let expanded_len = leaves.len().checked_mul(ordered.len()).with_context(|| {
                format!("message `{}` row expansion overflowed", entry.entry_id)
            })?;
            ensure_structural_cap(config, &entry.entry_id, expanded_len)?;
            let mut expanded = Vec::new();
            for leaf in leaves {
                for value in &ordered {
                    let mut variant = leaf.clone();
                    variant.path.push(
                        json!({"argument":argument,"facet":facet_id,"kind":"facet","value":value}),
                    );
                    variant
                        .conditions
                        .push(format!("{argument}.{facet_id}={value}"));
                    expanded.push(variant);
                }
            }
            leaves = expanded;
        }
    }
    Ok(leaves)
}

pub(super) fn expand_term_rows(
    config: &ProjectConfig,
    model: &CatalogModel,
    profile: &LocaleProfile,
    data: &LocaleData,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<ExpectedRow>> {
    let mut rows = Vec::new();
    for (term_id, term) in &model.terms {
        let entry_id = format!("term:{term_id}");
        let entry_signature =
            trox::revision_id(&json!({"identity_version":1,"kind":"term","term_id":term_id}))
                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                .trim_start_matches("rev1_")
                .to_owned();
        let row_context = TermRowContext {
            term_id,
            entry_id: &entry_id,
            signature: &entry_signature,
            profile,
        };
        let mut term_rows = Vec::new();
        term_rows.push(row_context.row(
            "$default",
            None,
            &term.value,
            "term_value",
            term.description.as_deref().unwrap_or(""),
        )?);
        for (form_id, declaration) in &config.term_forms {
            if profile
                .term_form_aliases
                .get(term_id)
                .is_some_and(|aliases| aliases.contains_key(form_id))
            {
                continue;
            }
            let numbered = declaration.number == NumberPolicy::Required;
            match resolve_term_surface(config, term, Some(form_id), numbered) {
                Some(ResolvedTermSurface::Scalar(text)) => term_rows.push(row_context.row(
                    form_id,
                    None,
                    text,
                    "term_form",
                    &declaration.description,
                )?),
                Some(ResolvedTermSurface::Number(values)) => {
                    for category in human_categories(&data.cardinal_categories) {
                        let english = values
                            .iter()
                            .find(|value| value.parts().0 == category)
                            .or_else(|| {
                                values
                                    .iter()
                                    .find(|value| value.parts().0 == PluralCategory::Other)
                            })
                            .expect("validated numbered term surface has other")
                            .parts()
                            .1;
                        term_rows.push(row_context.row(
                            form_id,
                            Some(category),
                            english,
                            "term_form",
                            &declaration.description,
                        )?);
                    }
                }
                Some(ResolvedTermSurface::NumberFallback(text)) => {
                    for category in human_categories(&data.cardinal_categories) {
                        term_rows.push(row_context.row(
                            form_id,
                            Some(category),
                            text,
                            "term_form",
                            &declaration.description,
                        )?);
                    }
                }
                None => {}
            }
            ensure_structural_cap(config, &entry_id, term_rows.len())?;
        }
        enforce_entry_row_limits(config, &entry_id, term_rows.len(), diagnostics)?;
        rows.extend(term_rows);
    }
    Ok(rows)
}

struct TermRowContext<'a> {
    term_id: &'a str,
    entry_id: &'a str,
    signature: &'a str,
    profile: &'a LocaleProfile,
}

impl TermRowContext<'_> {
    fn row(
        &self,
        form: &str,
        category: Option<PluralCategory>,
        english: &str,
        kind: &str,
        description: &str,
    ) -> Result<ExpectedRow> {
        let step = if form == "$default" {
            json!({"kind":"term_value"})
        } else if let Some(category) = category {
            json!({"category":category.as_str(),"form":form,"kind":"term_number"})
        } else {
            json!({"form":form,"kind":"term_form"})
        };
        let expansion = ExpansionDescriptor {
            entry_signature: self.signature.into(),
            path: vec![step],
        };
        let row_id =
            expansion_row_id(&expansion).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let conditions = if form == "$default" {
            "form=default".into()
        } else if let Some(category) = category {
            format!("form={form}; number.plural={}", category.as_str())
        } else {
            format!("form={form}")
        };
        let relevant_aliases = (form == "$default")
            .then(|| self.profile.term_form_aliases.get(self.term_id))
            .flatten();
        let source_revision = revision_id(&json!({
            "term_id": self.term_id,
            "form": form,
            "category": category,
            "english": english,
            "description": description,
            "profile_facets": self.profile.term_facets.get(self.term_id),
            "form_aliases": relevant_aliases,
        }))
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let term = Some(if form == "$default" {
            super::TermRowDescriptor::Default
        } else if let Some(category) = category {
            super::TermRowDescriptor::Number {
                form: form.into(),
                category,
            }
        } else {
            super::TermRowDescriptor::Scalar { form: form.into() }
        });
        let description = translator_description(description, None, &conditions);
        Ok(ExpectedRow {
            conditions,
            english: english.into(),
            description,
            placeholders: String::new(),
            entry_id: self.entry_id.into(),
            row_id,
            kind: kind.into(),
            source_locations: String::new(),
            source_revision,
            expansion,
            term,
        })
    }
}

fn human_categories(categories: &[PluralCategory]) -> Vec<PluralCategory> {
    let mut result = Vec::new();
    if categories.contains(&PluralCategory::Other) {
        result.push(PluralCategory::Other);
    }
    for category in [
        PluralCategory::Zero,
        PluralCategory::One,
        PluralCategory::Two,
        PluralCategory::Few,
        PluralCategory::Many,
    ] {
        if categories.contains(&category) {
            result.push(category);
        }
    }
    result
}
