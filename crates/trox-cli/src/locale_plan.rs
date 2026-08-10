//! Checked locale selection and fallback-profile planning.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context, Result, bail};

use crate::config::{ProjectConfig, validate_locale_id};
use crate::diagnostic::DiagnosticResultExt;
use crate::extract::{FacetScope, LocaleProfile, load_profile};

/// Profiles needed to validate and process a selected set of target locales.
pub struct LocalePlan {
    selected: Vec<String>,
    profiles: BTreeMap<String, LocaleProfile>,
}

impl LocalePlan {
    pub fn load(config: &ProjectConfig, requested: &[String]) -> Result<Self> {
        Self::load_impl(config, requested).diagnostic(
            "trox.invalid-locale-plan",
            Some(config.path.clone()),
            "Correct locale selection and fallback chains, then rerun the command.",
        )
    }

    fn load_impl(config: &ProjectConfig, requested: &[String]) -> Result<Self> {
        let selected = select_locales(config, requested)?;
        let mut profiles = BTreeMap::new();
        let mut queued: BTreeSet<_> = selected.iter().cloned().collect();
        let mut pending: VecDeque<_> = selected.iter().cloned().collect();

        while let Some(locale) = pending.pop_front() {
            let profile = load_profile(config, &locale)?;
            for fallback in &profile.fallbacks {
                validate_locale_id(fallback)?;
                if fallback == &config.source_locale {
                    continue;
                }
                if !config.locales.contains_key(fallback) {
                    bail!("locale `{locale}` references unconfigured fallback locale `{fallback}`");
                }
                if queued.insert(fallback.clone()) {
                    pending.push_back(fallback.clone());
                }
            }
            profiles.insert(locale, profile);
        }

        check_fallback_cycles(&profiles, &config.source_locale)?;

        for profile in profiles.values() {
            for parent in profile
                .fallbacks
                .iter()
                .filter(|locale| *locale != &config.source_locale)
            {
                check_profile_fallback_compatibility(
                    profile,
                    profiles
                        .get(parent)
                        .with_context(|| format!("fallback profile `{parent}` was not loaded"))?,
                )?;
            }
        }

        Ok(Self { selected, profiles })
    }

    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    pub fn profiles(&self) -> impl Iterator<Item = (&String, &LocaleProfile)> {
        self.profiles.iter()
    }

    pub fn profile(&self, locale: &str) -> &LocaleProfile {
        &self.profiles[locale]
    }
}

fn check_fallback_cycles(
    profiles: &BTreeMap<String, LocaleProfile>,
    source_locale: &str,
) -> Result<()> {
    fn visit(
        locale: &str,
        profiles: &BTreeMap<String, LocaleProfile>,
        source_locale: &str,
        states: &mut BTreeMap<String, u8>,
        stack: &mut Vec<String>,
    ) -> Result<()> {
        states.insert(locale.to_owned(), 1);
        stack.push(locale.to_owned());
        for parent in profiles[locale]
            .fallbacks
            .iter()
            .filter(|parent| parent.as_str() != source_locale)
        {
            match states.get(parent).copied().unwrap_or(0) {
                0 => visit(parent, profiles, source_locale, states, stack)?,
                1 => {
                    let start = stack.iter().position(|item| item == parent).unwrap_or(0);
                    let mut cycle = stack[start..].to_vec();
                    cycle.push(parent.clone());
                    bail!("locale fallback cycle: {}", cycle.join(" -> "));
                }
                _ => {}
            }
        }
        stack.pop();
        states.insert(locale.to_owned(), 2);
        Ok(())
    }

    let mut states = BTreeMap::new();
    let mut stack = Vec::new();
    for locale in profiles.keys() {
        if states.get(locale).copied().unwrap_or(0) == 0 {
            visit(locale, profiles, source_locale, &mut states, &mut stack)?;
        }
    }
    Ok(())
}

pub fn select_locales(config: &ProjectConfig, requested: &[String]) -> Result<Vec<String>> {
    if requested.is_empty() {
        return Ok(config.locales.keys().cloned().collect());
    }
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for locale in requested {
        if !config.locales.contains_key(locale) {
            bail!("unknown configured locale `{locale}`");
        }
        if seen.insert(locale) {
            result.push(locale.clone());
        }
    }
    Ok(result)
}

fn check_profile_fallback_compatibility(
    child: &LocaleProfile,
    parent: &LocaleProfile,
) -> Result<()> {
    for (facet_id, child_facet) in child
        .facets
        .iter()
        .filter(|(_, facet)| facet.scope == FacetScope::Message)
    {
        let parent_facet = parent.facets.get(facet_id).with_context(|| {
            format!(
                "parent locale `{}` lacks message facet `{facet_id}` required by `{}`",
                parent.locale, child.locale
            )
        })?;
        if parent_facet.scope != FacetScope::Message || parent_facet.values != child_facet.values {
            bail!(
                "parent locale `{}` has incompatible message facet `{facet_id}`",
                parent.locale
            );
        }
        for (term_id, child_values) in &child.term_facets {
            if parent
                .term_facets
                .get(term_id)
                .and_then(|values| values.get(facet_id))
                != child_values.get(facet_id)
            {
                bail!(
                    "parent locale `{}` classifies term `{term_id}` incompatibly for `{facet_id}`",
                    parent.locale
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ProfileDirection, ProfileIsolation};

    fn profile(locale: &str, fallbacks: &[&str]) -> LocaleProfile {
        LocaleProfile {
            locale: locale.into(),
            direction: ProfileDirection::Ltr,
            isolation: ProfileIsolation::Isolate,
            fallbacks: fallbacks.iter().map(|value| (*value).into()).collect(),
            facets: Default::default(),
            term_facets: Default::default(),
            term_form_aliases: Default::default(),
        }
    }

    #[test]
    fn fallback_graph_rejects_cross_profile_cycles() {
        let profiles = BTreeMap::from([
            ("es".into(), profile("es", &["fr", "en-US"])),
            ("fr".into(), profile("fr", &["es", "en-US"])),
        ]);
        let error = check_fallback_cycles(&profiles, "en-US")
            .unwrap_err()
            .to_string();
        assert!(error.contains("es -> fr -> es"));
    }
}
