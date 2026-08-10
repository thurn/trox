use data_encoding::BASE32_NOPAD;
use icu_locale_core::Locale;

use super::*;
use crate::model::{
    MAX_EXPANDED_ROWS, identity_ids, validate_nfc, validate_placeholder_name, validate_stable_id,
};
use crate::runtime::validate_plural_rules;

pub(super) fn validate_bundle(bundle: &Bundle) -> Result<(), DeserializeError> {
    if bundle.format != "trox-bundle" || bundle.version.major != 1 || bundle.version.minor != 0 {
        return Err(DeserializeError::UnsupportedVersion {
            format: "bundle",
            major: bundle.version.major,
            minor: bundle.version.minor,
        });
    }
    if !bundle.fallbacks_flattened {
        return Err(DeserializeError::InvalidBundle(
            "fallbacks_flattened must be true".into(),
        ));
    }
    if bundle.cldr_version != "48" {
        return Err(DeserializeError::InvalidBundle(format!(
            "unsupported CLDR version `{}`",
            bundle.cldr_version
        )));
    }
    validate_bundle_locale(&bundle.locale)?;
    validate_bundle_locale(&bundle.source_locale)?;
    let source_bundle = bundle.locale == bundle.source_locale;
    let mut fallback_locales = BTreeSet::new();
    for locale in &bundle.fallback_chain {
        validate_bundle_locale(locale)?;
        if locale == &bundle.locale || !fallback_locales.insert(locale.clone()) {
            return Err(DeserializeError::InvalidBundle(
                "fallback_chain is cyclic or contains duplicates".into(),
            ));
        }
    }
    if source_bundle {
        if !bundle.fallback_chain.is_empty() {
            return Err(DeserializeError::InvalidBundle(
                "source bundle fallback_chain must be empty".into(),
            ));
        }
    } else if bundle.fallback_chain.last() != Some(&bundle.source_locale) {
        return Err(DeserializeError::InvalidBundle(
            "target fallback_chain must end in source_locale".into(),
        ));
    }
    if !is_hex_digest(&bundle.source_catalog_fingerprint) {
        return Err(DeserializeError::InvalidBundle(
            "source_catalog_fingerprint must be 64 lowercase hex characters".into(),
        ));
    }
    let digits: BTreeSet<_> = bundle.number_format.digits.chars().collect();
    if digits.len() != 10 {
        return Err(DeserializeError::InvalidBundle(
            "number_format.digits must contain ten distinct scalars".into(),
        ));
    }
    if bundle.number_format.grouping.contains(&0)
        || bundle.number_format.minimum_grouping_digits == 0
        || bundle.number_format.decimal.is_empty()
        || bundle.number_format.group.is_empty()
        || bundle.number_format.exponent.is_empty()
        || bundle.number_format.minus.is_empty()
        || bundle.number_format.plus.is_empty()
    {
        return Err(DeserializeError::InvalidBundle(
            "number_format contains an invalid empty symbol or grouping width".into(),
        ));
    }
    for (label, value) in [
        (
            "number_format.decimal",
            bundle.number_format.decimal.as_str(),
        ),
        ("number_format.digits", bundle.number_format.digits.as_str()),
        (
            "number_format.exponent",
            bundle.number_format.exponent.as_str(),
        ),
        ("number_format.group", bundle.number_format.group.as_str()),
        ("number_format.minus", bundle.number_format.minus.as_str()),
        ("number_format.plus", bundle.number_format.plus.as_str()),
    ] {
        validate_nfc(value, label)
            .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
    }
    validate_plural_rules(&bundle.plural_rules)?;
    let mut facet_ids = BTreeSet::new();
    for facet in &bundle.message_facets {
        validate_stable_id(facet, "message facet ID")
            .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
        if !facet_ids.insert(facet) {
            return Err(DeserializeError::InvalidBundle(
                "duplicate message facet ID".into(),
            ));
        }
    }
    for (entry_id, entry) in &bundle.entries {
        if !is_canonical_short_id(entry_id, "tx1_") {
            return Err(DeserializeError::InvalidBundle(format!(
                "entry ID `{entry_id}` is malformed"
            )));
        }
        if !is_hex_digest(&entry.source_signature) {
            return Err(DeserializeError::InvalidBundle(format!(
                "entry `{entry_id}` has malformed source signature"
            )));
        }
        if source_bundle
            && (entry.arguments.is_none() || entry.identity.is_none() || !entry.rows.is_empty())
        {
            return Err(DeserializeError::InvalidBundle(format!(
                "source entry `{entry_id}` requires arguments, identity, and empty rows"
            )));
        }
        if !source_bundle && (entry.arguments.is_some() || entry.identity.is_some()) {
            return Err(DeserializeError::InvalidBundle(format!(
                "target entry `{entry_id}` must not contain arguments or identity"
            )));
        }
        if let Some(identity) = &entry.identity {
            let (computed, signature) = identity_ids(identity)
                .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
            if computed != *entry_id || signature != entry.source_signature {
                return Err(DeserializeError::InvalidBundle(format!(
                    "entry `{entry_id}` identity mismatch"
                )));
            }
            let declared = super::declared_placeholders(&identity.pattern)
                .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
            let arguments = entry
                .arguments
                .as_ref()
                .expect("validated source arguments");
            let actual: BTreeSet<_> = arguments.keys().cloned().collect();
            if declared != actual {
                return Err(DeserializeError::InvalidBundle(format!(
                    "source entry `{entry_id}` argument schemas differ from its placeholders"
                )));
            }
            for schema in arguments.values() {
                if let ArgumentSchema::Term {
                    form: Some(form), ..
                } = schema
                {
                    validate_stable_id(form, "term form")
                        .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
                }
            }
        }
        if entry.rows.len() > MAX_EXPANDED_ROWS {
            return Err(DeserializeError::InvalidBundle(format!(
                "entry `{entry_id}` exceeds the 4,096-row runtime limit"
            )));
        }
        for (row_id, row) in &entry.rows {
            if row.translation.is_empty() {
                return Err(DeserializeError::InvalidBundle(format!(
                    "row `{row_id}` has an empty translation"
                )));
            }
            validate_nfc(&row.translation, "bundle row translation")
                .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
            validate_bundle_locale(&row.origin_locale)?;
            if row.origin_locale != bundle.locale
                && row.origin_locale != bundle.source_locale
                && !fallback_locales.contains(&row.origin_locale)
            {
                return Err(DeserializeError::InvalidBundle(format!(
                    "row `{row_id}` has origin outside fallback_chain"
                )));
            }
            if row.expansion.entry_signature != entry.source_signature {
                return Err(DeserializeError::InvalidBundle(format!(
                    "row `{row_id}` has wrong entry signature"
                )));
            }
            expansion_key_from_json(&row.expansion.path)?;
            let signature = signature(&row.expansion)
                .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
            if short_id("row1_", &signature) != *row_id {
                return Err(DeserializeError::InvalidBundle(format!(
                    "row `{row_id}` hash mismatch"
                )));
            }
        }
    }
    for (term_id, term) in &bundle.terms {
        validate_stable_id(term_id, "term ID")
            .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
        if source_bundle && !term.forms.contains_key("$default") {
            return Err(DeserializeError::InvalidBundle(format!(
                "source term `{term_id}` lacks $default"
            )));
        }
        for (facet, value) in &term.facets {
            validate_stable_id(facet, "term facet ID")
                .and_then(|()| validate_stable_id(value, "term facet value"))
                .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
        }
        for (form_id, form) in &term.forms {
            if form_id != "$default" {
                validate_stable_id(form_id, "term form ID")
                    .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
            }
            match form {
                BundleTermForm::Scalar {
                    origin_locale,
                    text,
                } => {
                    validate_surface_text(text, term_id, form_id)?;
                    validate_term_origin(origin_locale, bundle, &fallback_locales)?;
                }
                BundleTermForm::Number { values } => {
                    if form_id == "$default" {
                        return Err(DeserializeError::InvalidBundle(format!(
                            "term `{term_id}` has a numbered $default form"
                        )));
                    }
                    if values.is_empty() {
                        return Err(DeserializeError::InvalidBundle(format!(
                            "numbered term form `{term_id}.{form_id}` is empty"
                        )));
                    }
                    if source_bundle && !values.contains_key(&PluralCategory::Other) {
                        return Err(DeserializeError::InvalidBundle(format!(
                            "source numbered term form `{term_id}.{form_id}` lacks other"
                        )));
                    }
                    for (category, surface) in values {
                        validate_surface_text(&surface.text, term_id, form_id)?;
                        if !bundle.plural_rules.cardinal.contains_key(category) {
                            return Err(DeserializeError::InvalidBundle(format!(
                                "term form `{term_id}.{form_id}` uses unsupported category `{}`",
                                category.as_str()
                            )));
                        }
                        validate_term_origin(&surface.origin_locale, bundle, &fallback_locales)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_surface_text(text: &str, term_id: &str, form_id: &str) -> Result<(), DeserializeError> {
    if text.is_empty() {
        return Err(DeserializeError::InvalidBundle(format!(
            "term surface `{term_id}.{form_id}` is empty"
        )));
    }
    validate_nfc(text, "bundle term surface")
        .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_canonical_short_id(value: &str, prefix: &str) -> bool {
    let Some(encoded) = value.strip_prefix(prefix) else {
        return false;
    };
    let upper = encoded.to_ascii_uppercase();
    BASE32_NOPAD.decode(upper.as_bytes()).is_ok_and(|decoded| {
        decoded.len() == 16 && BASE32_NOPAD.encode(&decoded).to_ascii_lowercase() == encoded
    })
}

fn validate_bundle_locale(locale: &str) -> Result<(), DeserializeError> {
    let parsed = locale
        .parse::<Locale>()
        .map_err(|_| DeserializeError::InvalidBundle(format!("invalid locale `{locale}`")))?;
    if parsed.to_string() != locale {
        return Err(DeserializeError::InvalidBundle(format!(
            "locale `{locale}` is not in canonical BCP-47 form"
        )));
    }
    Ok(())
}

fn validate_term_origin(
    origin: &str,
    bundle: &Bundle,
    fallbacks: &BTreeSet<String>,
) -> Result<(), DeserializeError> {
    validate_bundle_locale(origin)?;
    if origin != bundle.locale && origin != bundle.source_locale && !fallbacks.contains(origin) {
        return Err(DeserializeError::InvalidBundle(format!(
            "term surface has origin `{origin}` outside fallback_chain"
        )));
    }
    Ok(())
}

pub(super) fn validate_message_expansion(
    pattern: &Pattern,
    path: &[Value],
    bundle: &Bundle,
) -> Result<(), DeserializeError> {
    let declared = super::declared_placeholders(pattern)
        .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
    let mut pattern = pattern;
    let mut index = 0;
    loop {
        match pattern {
            Pattern::Text { .. } => break,
            Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
                let step = path.get(index).and_then(Value::as_object).ok_or_else(|| {
                    DeserializeError::InvalidBundle(
                        "message expansion omits a selector step".into(),
                    )
                })?;
                let expected_kind = if matches!(pattern, Pattern::Ordinal { .. }) {
                    "ordinal"
                } else {
                    "plural"
                };
                if step.len() != 3
                    || step.get("kind").and_then(Value::as_str) != Some(expected_kind)
                {
                    return Err(DeserializeError::InvalidBundle(
                        "numeric expansion step has unknown fields or wrong kind".into(),
                    ));
                }
                let branch = step
                    .get("branch")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|branch| *branch < branches.len())
                    .ok_or_else(|| {
                        DeserializeError::InvalidBundle(
                            "numeric expansion branch is invalid".into(),
                        )
                    })?;
                let matcher = step
                    .get("match")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        DeserializeError::InvalidBundle("numeric expansion lacks match".into())
                    })?;
                if let Some(exact) = matcher.get("exact").and_then(Value::as_u64) {
                    if matcher.len() != 1 || branches[branch].key.exact() != Some(exact) {
                        return Err(DeserializeError::InvalidBundle(
                            "exact expansion does not match its source branch".into(),
                        ));
                    }
                } else {
                    let category = matcher
                        .get("category")
                        .and_then(Value::as_str)
                        .and_then(parse_plural_category)
                        .ok_or_else(|| {
                            DeserializeError::InvalidBundle(
                                "numeric expansion has invalid category".into(),
                            )
                        })?;
                    if matcher.len() != 1 {
                        return Err(DeserializeError::InvalidBundle(
                            "numeric match has unknown fields".into(),
                        ));
                    }
                    let rules = if expected_kind == "ordinal" {
                        &bundle.plural_rules.ordinal
                    } else {
                        &bundle.plural_rules.cardinal
                    };
                    if !rules.contains_key(&category) {
                        return Err(DeserializeError::InvalidBundle(format!(
                            "bundle has a row for unsupported {expected_kind} category `{}`",
                            category.as_str()
                        )));
                    }
                    let expected_branch = branches
                        .iter()
                        .position(|candidate| candidate.key.category() == Some(category))
                        .or_else(|| {
                            branches.iter().position(|candidate| {
                                candidate.key.category() == Some(PluralCategory::Other)
                            })
                        })
                        .expect("validated other");
                    if branch != expected_branch {
                        return Err(DeserializeError::InvalidBundle(
                            "category expansion points at the wrong source branch".into(),
                        ));
                    }
                }
                pattern = &branches[branch].pattern;
                index += 1;
            }
            Pattern::Select { branches } => {
                let step = path.get(index).and_then(Value::as_object).ok_or_else(|| {
                    DeserializeError::InvalidBundle("message expansion omits select step".into())
                })?;
                if step.len() != 2 || step.get("kind").and_then(Value::as_str) != Some("select") {
                    return Err(DeserializeError::InvalidBundle(
                        "select expansion step has unknown fields".into(),
                    ));
                }
                let branch = step
                    .get("branch")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|branch| *branch < branches.len())
                    .ok_or_else(|| {
                        DeserializeError::InvalidBundle("select expansion branch is invalid".into())
                    })?;
                pattern = branches[branch].pattern();
                index += 1;
            }
        }
    }
    let facet_order: BTreeMap<_, _> = bundle
        .message_facets
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let mut prior: Option<(String, usize)> = None;
    let mut seen = BTreeSet::new();
    for step in &path[index..] {
        let step = step.as_object().ok_or_else(|| {
            DeserializeError::InvalidBundle("facet expansion step is not an object".into())
        })?;
        if step.len() != 4 || step.get("kind").and_then(Value::as_str) != Some("facet") {
            return Err(DeserializeError::InvalidBundle(
                "expansion has trailing non-facet or unknown fields".into(),
            ));
        }
        let argument = step
            .get("argument")
            .and_then(Value::as_str)
            .ok_or_else(|| DeserializeError::InvalidBundle("facet step lacks argument".into()))?;
        if !declared.contains(argument) {
            return Err(DeserializeError::InvalidBundle(format!(
                "facet expansion references undeclared argument `{argument}`"
            )));
        }
        let facet = step
            .get("facet")
            .and_then(Value::as_str)
            .ok_or_else(|| DeserializeError::InvalidBundle("facet step lacks facet ID".into()))?;
        let value = step
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| DeserializeError::InvalidBundle("facet step lacks value".into()))?;
        let order = *facet_order.get(facet).ok_or_else(|| {
            DeserializeError::InvalidBundle(format!("facet `{facet}` is not message-scoped"))
        })?;
        let current = (argument.to_owned(), order);
        if prior.as_ref().is_some_and(|prior| prior >= &current) || !seen.insert((argument, facet))
        {
            return Err(DeserializeError::InvalidBundle(
                "facet expansion steps are duplicate or noncanonical".into(),
            ));
        }
        if !bundle
            .terms
            .values()
            .any(|term| term.facets.get(facet).is_some_and(|known| known == value))
        {
            return Err(DeserializeError::InvalidBundle(
                "facet expansion value is not reachable from a bundled term".into(),
            ));
        }
        prior = Some(current);
    }
    Ok(())
}

fn parse_plural_category(value: &str) -> Option<PluralCategory> {
    Some(match value {
        "zero" => PluralCategory::Zero,
        "one" => PluralCategory::One,
        "two" => PluralCategory::Two,
        "few" => PluralCategory::Few,
        "many" => PluralCategory::Many,
        "other" => PluralCategory::Other,
        _ => return None,
    })
}

pub(super) fn expansion_key_from_json(path: &[Value]) -> Result<ExpansionKey, DeserializeError> {
    path.iter()
        .map(|raw| {
            let step = raw.as_object().ok_or_else(|| {
                DeserializeError::InvalidBundle("expansion step is not an object".into())
            })?;
            let kind = step.get("kind").and_then(Value::as_str).ok_or_else(|| {
                DeserializeError::InvalidBundle("expansion step lacks kind".into())
            })?;
            match kind {
                "plural" | "ordinal" => {
                    if step.len() != 3 {
                        return Err(DeserializeError::InvalidBundle(
                            "numeric expansion step has unknown fields".into(),
                        ));
                    }
                    let branch = expansion_branch(step)?;
                    let matcher =
                        step.get("match")
                            .and_then(Value::as_object)
                            .ok_or_else(|| {
                                DeserializeError::InvalidBundle(
                                    "numeric expansion lacks match".into(),
                                )
                            })?;
                    let ordinal = kind == "ordinal";
                    if let Some(exact) = matcher.get("exact").and_then(Value::as_u64) {
                        if matcher.len() != 1 {
                            return Err(DeserializeError::InvalidBundle(
                                "exact expansion has unknown fields".into(),
                            ));
                        }
                        Ok(ExpansionStep::Exact {
                            branch,
                            exact,
                            ordinal,
                        })
                    } else {
                        let category = matcher
                            .get("category")
                            .and_then(Value::as_str)
                            .and_then(parse_plural_category)
                            .ok_or_else(|| {
                                DeserializeError::InvalidBundle(
                                    "numeric expansion has invalid category".into(),
                                )
                            })?;
                        if matcher.len() != 1 {
                            return Err(DeserializeError::InvalidBundle(
                                "category expansion has unknown fields".into(),
                            ));
                        }
                        Ok(ExpansionStep::Category {
                            branch,
                            category,
                            ordinal,
                        })
                    }
                }
                "select" => {
                    if step.len() != 2 {
                        return Err(DeserializeError::InvalidBundle(
                            "select expansion step has unknown fields".into(),
                        ));
                    }
                    Ok(ExpansionStep::Select {
                        branch: expansion_branch(step)?,
                    })
                }
                "facet" => {
                    if step.len() != 4 {
                        return Err(DeserializeError::InvalidBundle(
                            "facet expansion step has unknown fields".into(),
                        ));
                    }
                    let argument = step
                        .get("argument")
                        .and_then(Value::as_str)
                        .filter(|value| validate_placeholder_name(value))
                        .ok_or_else(|| {
                            DeserializeError::InvalidBundle(
                                "facet expansion has invalid argument name".into(),
                            )
                        })?;
                    let facet = expansion_stable_id(step, "facet", "facet ID")?;
                    let value = expansion_stable_id(step, "value", "facet value")?;
                    Ok(ExpansionStep::Facet {
                        argument: argument.to_owned(),
                        facet: facet.to_owned(),
                        value: value.to_owned(),
                    })
                }
                _ => Err(DeserializeError::InvalidBundle(format!(
                    "unknown expansion step kind `{kind}`"
                ))),
            }
        })
        .collect()
}

fn expansion_branch(step: &serde_json::Map<String, Value>) -> Result<usize, DeserializeError> {
    step.get("branch")
        .and_then(Value::as_u64)
        .and_then(|branch| usize::try_from(branch).ok())
        .ok_or_else(|| DeserializeError::InvalidBundle("invalid expansion branch".into()))
}

fn expansion_stable_id<'a>(
    step: &'a serde_json::Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<&'a str, DeserializeError> {
    let value = step
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| DeserializeError::InvalidBundle(format!("facet expansion lacks {label}")))?;
    validate_stable_id(value, label)
        .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
    Ok(value)
}
