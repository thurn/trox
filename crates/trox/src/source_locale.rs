use std::collections::{BTreeMap, BTreeSet};

use icu_locale_core::Locale;
use serde::de::{EnumAccess, SeqAccess, VariantAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::TroxValueError;
use crate::bundle::{
    Bundle, BundleTerm, BundleTermForm, BundleTermSurface, IsolationPolicy, NumberFormat,
    PluralRules, TextDirection,
};
use crate::model::{PluralCategory, Version, placeholders, validate_nfc, validate_stable_id};

/// The CLDR release pinned into Trox's source and bundle runtimes.
pub const CLDR_VERSION: &str = "48";

const SUPPORTED_LOCALES: &[&str] = &[
    "ar", "de", "en-US", "es", "fr", "ja", "ko", "pl", "pt-BR", "pt-PT", "ru", "zh-Hans", "zh-Hant",
];

/// Human-authored configuration for bundle-free source-language resolution.
///
/// [`Self::new`] supplies Trox's pinned CLDR plural, ordinal, number-format, and
/// direction data. [`Self::from_project_ron`] additionally consumes the
/// source-locale and term-form declarations from `trox.ron` plus the ordinary
/// human-authored `terms.ron`; neither input is generated.
#[derive(Debug, Clone)]
pub struct SourceLocale {
    pub(crate) locale: String,
    pub(crate) direction: TextDirection,
    pub(crate) isolation: IsolationPolicy,
    pub(crate) number_format: NumberFormat,
    pub(crate) plural_rules: PluralRules,
    pub(crate) terms: BTreeMap<String, BundleTerm>,
}

impl SourceLocale {
    /// Constructs a source locale with pinned CLDR data and no terms.
    pub fn new(locale: impl Into<String>) -> Result<Self, TroxValueError> {
        let locale = locale.into();
        validate_locale(&locale)?;
        if !SUPPORTED_LOCALES.contains(&locale.as_str()) {
            return Err(source_config_error(format!(
                "locale `{locale}` is not supported by Trox's pinned CLDR {CLDR_VERSION} data; supported locales: {}",
                SUPPORTED_LOCALES.join(", ")
            )));
        }
        let (plural_rules, number_format, direction) = locale_data(&locale);
        Ok(Self {
            locale,
            direction,
            isolation: IsolationPolicy::Isolate,
            number_format,
            plural_rules,
            terms: BTreeMap::new(),
        })
    }

    /// Builds source-development configuration from human-authored project and term RON.
    ///
    /// Unknown project fields are ignored so the normal `trox.ron` file can be
    /// embedded directly with `include_str!`. Term forms that declare
    /// `source_fallback: Default` receive the same default surface behavior as
    /// generated source bundles.
    pub fn from_project_ron(project_ron: &str, terms_ron: &str) -> Result<Self, TroxValueError> {
        let project: SourceProject = ron::from_str(project_ron)
            .map_err(|error| source_config_error(format!("invalid project RON: {error}")))?;
        let records: BTreeMap<String, SourceTermRecord> = ron::from_str(terms_ron)
            .map_err(|error| source_config_error(format!("invalid term catalog RON: {error}")))?;
        let mut source = Self::new(project.source_locale)?;
        source.terms = compile_terms(
            &source.locale,
            &source.plural_rules,
            records,
            Some(&project.term_forms),
        )?;
        Ok(source)
    }

    /// Sets the source-development placeholder isolation policy.
    #[must_use]
    pub fn with_isolation(mut self, isolation: IsolationPolicy) -> Self {
        self.isolation = isolation;
        self
    }

    /// Returns the canonical BCP-47 locale identifier.
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Returns the locale's pinned text direction.
    pub fn direction(&self) -> TextDirection {
        self.direction
    }

    /// Returns the active placeholder isolation policy.
    pub fn isolation(&self) -> IsolationPolicy {
        self.isolation
    }

    /// Returns the pinned decimal-format data.
    pub fn number_format(&self) -> &NumberFormat {
        &self.number_format
    }

    /// Returns the pinned cardinal and ordinal rules.
    pub fn plural_rules(&self) -> &PluralRules {
        &self.plural_rules
    }

    pub(crate) fn into_runtime_bundle(self) -> Bundle {
        Bundle {
            cldr_version: CLDR_VERSION.into(),
            direction: self.direction,
            entries: BTreeMap::new(),
            fallback_chain: vec![],
            fallbacks_flattened: true,
            format: "trox-bundle".into(),
            isolation: self.isolation,
            locale: self.locale.clone(),
            message_facets: vec![],
            number_format: self.number_format,
            plural_rules: self.plural_rules,
            source_catalog_fingerprint: "0".repeat(64),
            source_locale: self.locale,
            terms: self.terms,
            version: Version::V1_1,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SourceProject {
    source_locale: String,
    #[serde(default)]
    term_forms: BTreeMap<String, SourceTermFormDeclaration>,
}

#[derive(Debug, Deserialize)]
struct SourceTermFormDeclaration {
    #[allow(dead_code)]
    description: String,
    #[serde(default)]
    number: SourceNumberPolicy,
    #[serde(default, deserialize_with = "plain_optional")]
    source_fallback: Option<SourceFallback>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
enum SourceNumberPolicy {
    #[default]
    Forbidden,
    Required,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
enum SourceFallback {
    Default,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceTermRecord {
    #[allow(dead_code)]
    #[serde(default, deserialize_with = "plain_optional")]
    description: Option<String>,
    value: String,
    #[serde(default)]
    forms: BTreeMap<String, SourceTermSurface>,
    #[serde(default)]
    facets: BTreeMap<String, String>,
}

#[derive(Debug)]
enum SourceTermSurface {
    Scalar(String),
    Number(Vec<SourceNumberSurface>),
}

impl<'de> Deserialize<'de> for SourceTermSurface {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SurfaceVisitor;
        impl<'de> Visitor<'de> for SurfaceVisitor {
            type Value = SourceTermSurface;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a string or Number([...])")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(SourceTermSurface::Scalar(value.into()))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(SourceTermSurface::Scalar(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let values = sequence
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("Number requires one category list"))?;
                if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::custom(
                        "Number accepts exactly one category list",
                    ));
                }
                Ok(SourceTermSurface::Number(values))
            }

            fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                let (variant, payload) = data.variant::<String>()?;
                if variant != "Number" {
                    return Err(serde::de::Error::unknown_variant(&variant, &["Number"]));
                }
                Ok(SourceTermSurface::Number(payload.newtype_variant()?))
            }
        }
        deserializer.deserialize_any(SurfaceVisitor)
    }
}

#[derive(Debug, Deserialize)]
enum SourceNumberSurface {
    Zero(String),
    One(String),
    Two(String),
    Few(String),
    Many(String),
    Other(String),
}

impl SourceNumberSurface {
    fn parts(&self) -> (PluralCategory, &str) {
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

fn compile_terms(
    locale: &str,
    plural_rules: &PluralRules,
    records: BTreeMap<String, SourceTermRecord>,
    declarations: Option<&BTreeMap<String, SourceTermFormDeclaration>>,
) -> Result<BTreeMap<String, BundleTerm>, TroxValueError> {
    if let Some(declarations) = declarations {
        for (form_id, declaration) in declarations {
            validate_stable_id(form_id, "term form ID")?;
            if declaration.description.trim().is_empty() {
                return Err(source_config_error(format!(
                    "term form `{form_id}` has an empty description"
                )));
            }
        }
    }
    let mut result = BTreeMap::new();
    for (term_id, record) in records {
        validate_stable_id(&term_id, "term ID")?;
        validate_term_text(&record.value, "term value")?;
        let mut facets = BTreeMap::new();
        for (facet_id, facet_value) in record.facets {
            validate_stable_id(&facet_id, "term facet ID")?;
            validate_stable_id(&facet_value, "term facet value")?;
            facets.insert(facet_id, facet_value);
        }
        let mut forms = BTreeMap::from([(
            "$default".into(),
            BundleTermForm::Scalar {
                origin_locale: locale.into(),
                text: record.value.clone(),
            },
        )]);
        for (form_id, surface) in record.forms {
            validate_stable_id(&form_id, "term form ID")?;
            if let Some(declarations) = declarations {
                let declaration = declarations.get(&form_id).ok_or_else(|| {
                    source_config_error(format!(
                        "term `{term_id}` uses undeclared form `{form_id}`"
                    ))
                })?;
                let matches = matches!(
                    (&surface, declaration.number),
                    (SourceTermSurface::Scalar(_), SourceNumberPolicy::Forbidden)
                        | (SourceTermSurface::Number(_), SourceNumberPolicy::Required)
                );
                if !matches {
                    return Err(source_config_error(format!(
                        "term `{term_id}` form `{form_id}` violates its number policy"
                    )));
                }
            }
            forms.insert(
                form_id,
                compile_term_form(locale, plural_rules, &term_id, surface)?,
            );
        }
        if let Some(declarations) = declarations {
            for (form_id, declaration) in declarations {
                if forms.contains_key(form_id)
                    || declaration.source_fallback != Some(SourceFallback::Default)
                {
                    continue;
                }
                let form = match declaration.number {
                    SourceNumberPolicy::Forbidden => BundleTermForm::Scalar {
                        origin_locale: locale.into(),
                        text: record.value.clone(),
                    },
                    SourceNumberPolicy::Required => BundleTermForm::Number {
                        values: BTreeMap::from([(
                            PluralCategory::Other,
                            BundleTermSurface {
                                origin_locale: locale.into(),
                                text: record.value.clone(),
                            },
                        )]),
                    },
                };
                forms.insert(form_id.clone(), form);
            }
        }
        result.insert(term_id, BundleTerm { facets, forms });
    }
    Ok(result)
}

fn compile_term_form(
    locale: &str,
    plural_rules: &PluralRules,
    term_id: &str,
    surface: SourceTermSurface,
) -> Result<BundleTermForm, TroxValueError> {
    match surface {
        SourceTermSurface::Scalar(text) => {
            validate_term_text(&text, "term form")?;
            Ok(BundleTermForm::Scalar {
                origin_locale: locale.into(),
                text,
            })
        }
        SourceTermSurface::Number(surfaces) => {
            let mut values = BTreeMap::new();
            let mut seen = BTreeSet::new();
            let mut previous = None;
            for surface in surfaces {
                let (category, text) = surface.parts();
                let order = PluralCategory::CANONICAL
                    .iter()
                    .position(|candidate| *candidate == category)
                    .expect("canonical category contains every category");
                if previous.is_some_and(|prior| prior >= order) || !seen.insert(category) {
                    return Err(source_config_error(format!(
                        "term `{term_id}` numbered form categories are duplicate or noncanonical"
                    )));
                }
                previous = Some(order);
                validate_term_text(text, "numbered term surface")?;
                values.insert(
                    category,
                    BundleTermSurface {
                        origin_locale: locale.into(),
                        text: text.into(),
                    },
                );
            }
            if !values.contains_key(&PluralCategory::Other) {
                return Err(source_config_error(format!(
                    "term `{term_id}` numbered form requires Other"
                )));
            }
            let missing = PluralCategory::CANONICAL
                .into_iter()
                .filter(|category| plural_rules.cardinal.contains_key(category))
                .filter(|category| !values.contains_key(category))
                .map(PluralCategory::as_str)
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(source_config_error(format!(
                    "term `{term_id}` numbered form lacks source-locale cardinal categories: {}",
                    missing.join(", ")
                )));
            }
            Ok(BundleTermForm::Number { values })
        }
    }
}

fn validate_term_text(value: &str, label: &str) -> Result<(), TroxValueError> {
    if value.is_empty() {
        return Err(source_config_error(format!("{label} must not be empty")));
    }
    validate_nfc(value, label)?;
    if !placeholders(value)?.is_empty() {
        return Err(source_config_error(format!(
            "{label} cannot contain placeholders"
        )));
    }
    Ok(())
}

fn validate_locale(locale: &str) -> Result<(), TroxValueError> {
    let parsed = locale
        .parse::<Locale>()
        .map_err(|_| source_config_error(format!("invalid locale `{locale}`")))?;
    if parsed.to_string() != locale {
        return Err(source_config_error(format!(
            "locale `{locale}` is not in canonical BCP-47 form"
        )));
    }
    Ok(())
}

fn source_config_error(message: impl Into<String>) -> TroxValueError {
    TroxValueError::new("trox.source-locale", message)
}

fn plain_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn locale_data(locale: &str) -> (PluralRules, NumberFormat, TextDirection) {
    let language = locale.split(['-', '_']).next().unwrap_or(locale);
    let mut cardinal = BTreeMap::new();
    let mut ordinal = BTreeMap::new();
    let mut number_format = NumberFormat::default();
    let mut direction = TextDirection::Ltr;
    match language {
        "ru" => {
            cardinal.insert(
                PluralCategory::One,
                "v = 0 and i % 10 = 1 and i % 100 != 11".into(),
            );
            cardinal.insert(
                PluralCategory::Few,
                "v = 0 and i % 10 = 2..4 and i % 100 != 12..14".into(),
            );
            cardinal.insert(
                PluralCategory::Many,
                "v = 0 and i % 10 = 0 or v = 0 and i % 10 = 5..9 or v = 0 and i % 100 = 11..14"
                    .into(),
            );
            number_format.group = " ".into();
            number_format.decimal = ",".into();
        }
        "pl" => {
            cardinal.insert(PluralCategory::One, "i = 1 and v = 0".into());
            cardinal.insert(
                PluralCategory::Few,
                "v = 0 and i % 10 = 2..4 and i % 100 != 12..14".into(),
            );
            cardinal.insert(PluralCategory::Many, "v = 0 and i != 1 and i % 10 = 0..1 or v = 0 and i % 10 = 5..9 or v = 0 and i % 100 = 12..14".into());
            number_format.group = " ".into();
            number_format.decimal = ",".into();
            number_format.minimum_grouping_digits = 2;
        }
        "ar" => {
            cardinal.insert(PluralCategory::Zero, "n = 0".into());
            cardinal.insert(PluralCategory::One, "n = 1".into());
            cardinal.insert(PluralCategory::Two, "n = 2".into());
            cardinal.insert(PluralCategory::Few, "n % 100 = 3..10".into());
            cardinal.insert(PluralCategory::Many, "n % 100 = 11..99".into());
            number_format.minus = "\u{200e}-".into();
            number_format.plus = "\u{200e}+".into();
            direction = TextDirection::Rtl;
        }
        "fr" => {
            cardinal.insert(PluralCategory::One, "i = 0,1".into());
            cardinal.insert(PluralCategory::Many, "i != 0 and i % 1000000 = 0".into());
            ordinal.insert(PluralCategory::One, "n = 1".into());
            number_format.group = " ".into();
            number_format.decimal = ",".into();
        }
        "pt" => {
            cardinal.insert(
                PluralCategory::One,
                if locale.eq_ignore_ascii_case("pt-PT") {
                    "i = 1 and v = 0"
                } else {
                    "i = 0..1"
                }
                .into(),
            );
            cardinal.insert(PluralCategory::Many, "i != 0 and i % 1000000 = 0".into());
            number_format.group = if locale.eq_ignore_ascii_case("pt-PT") {
                " "
            } else {
                "."
            }
            .into();
            number_format.decimal = ",".into();
            if locale.eq_ignore_ascii_case("pt-PT") {
                number_format.minimum_grouping_digits = 2;
            }
        }
        "es" | "de" | "en" => {
            cardinal.insert(PluralCategory::One, "i = 1 and v = 0".into());
            if language == "es" {
                cardinal.insert(PluralCategory::Many, "i != 0 and i % 1000000 = 0".into());
                number_format.minimum_grouping_digits = 2;
            }
            if language != "en" {
                number_format.group = ".".into();
                number_format.decimal = ",".into();
            }
        }
        _ => {}
    }
    if language == "en" {
        ordinal.insert(PluralCategory::One, "n % 10 = 1 and n % 100 != 11".into());
        ordinal.insert(PluralCategory::Two, "n % 10 = 2 and n % 100 != 12".into());
        ordinal.insert(PluralCategory::Few, "n % 10 = 3 and n % 100 != 13".into());
    }
    cardinal.insert(PluralCategory::Other, String::new());
    ordinal.insert(PluralCategory::Other, String::new());
    (PluralRules { cardinal, ordinal }, number_format, direction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_ron_applies_explicit_and_defaulted_term_forms() {
        let source = SourceLocale::from_project_ron(
            r#"(
                source_locale: "en-US",
                term_forms: {
                    "counted": (description: "Counted.", number: Required),
                    "indefinite": (description: "Indefinite.", source_fallback: Default),
                },
            )"#,
            r#"{
                "unit.card": (
                    value: "card",
                    forms: { "counted": Number([One("card"), Other("cards")]) },
                    facets: { "gender": "neutral" },
                ),
            }"#,
        )
        .unwrap();
        let term = &source.terms["unit.card"];
        assert!(term.forms.contains_key("counted"));
        assert!(term.forms.contains_key("indefinite"));
        assert_eq!(term.facets["gender"], "neutral");
    }

    #[test]
    fn project_ron_rejects_noncanonical_or_incomplete_source_term_categories() {
        let project = r#"(
            source_locale: "en-US",
            term_forms: {
                "counted": (description: "Counted.", number: Required),
            },
        )"#;
        let noncanonical = SourceLocale::from_project_ron(
            project,
            r#"{
                "unit.card": (
                    value: "card",
                    forms: { "counted": Number([Other("cards"), One("card")]) },
                ),
            }"#,
        )
        .unwrap_err();
        assert!(noncanonical.message.contains("noncanonical"));

        let incomplete = SourceLocale::from_project_ron(
            project,
            r#"{
                "unit.card": (
                    value: "card",
                    forms: { "counted": Number([Other("cards")]) },
                ),
            }"#,
        )
        .unwrap_err();
        assert!(incomplete.message.contains("cardinal categories: one"));
    }
}
