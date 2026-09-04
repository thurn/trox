use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Deref;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AnnotatedLocalizedString;
use crate::canonical::{canonical_json, short_id, signature};
pub use crate::catalog::SourceCatalog;
use crate::error::{DeserializeError, ResolveError};
use crate::model::{
    Argument, ArgumentSchema, IdentityDescriptor, Pattern, PluralCategory, SelectIdentityBranch,
    SelectorRecord, Version, placeholders,
};
use crate::runtime::{CompiledPluralRules, format_number};
use crate::source_locale::SourceLocale;
use crate::value::LocalizedString;
use validation::{expansion_key_from_json, validate_message_expansion};

#[cfg(test)]
mod selection_tests;
mod validation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Base direction of a locale's surrounding text.
pub enum TextDirection {
    /// Left-to-right text.
    Ltr,
    /// Right-to-left text.
    Rtl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Controls bidirectional isolation around interpolated placeholder surfaces.
pub enum IsolationPolicy {
    /// Wrap every interpolation in FSI and PDI controls.
    Isolate,
    /// Insert interpolation text without directional controls.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Pinned locale data used by Trox's deterministic decimal formatter.
pub struct NumberFormat {
    /// Decimal separator.
    pub decimal: String,
    /// Ten output scalars ordered from zero through nine.
    pub digits: String,
    /// Exponent separator.
    pub exponent: String,
    /// Integer grouping separator.
    pub group: String,
    /// Primary and secondary grouping widths.
    pub grouping: [usize; 2],
    /// CLDR threshold controlling when grouping first appears.
    pub minimum_grouping_digits: usize,
    /// Minus-sign surface.
    pub minus: String,
    /// Plus-sign surface.
    pub plus: String,
}

impl Default for NumberFormat {
    fn default() -> Self {
        Self {
            decimal: ".".into(),
            digits: "0123456789".into(),
            exponent: "E".into(),
            group: ",".into(),
            grouping: [3, 3],
            minimum_grouping_digits: 1,
            minus: "-".into(),
            plus: "+".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// CLDR cardinal and ordinal conditions keyed by resulting category.
pub struct PluralRules {
    /// Cardinal conditions; `other` must map to an empty condition.
    pub cardinal: BTreeMap<PluralCategory, String>,
    /// Ordinal conditions; `other` must map to an empty condition.
    pub ordinal: BTreeMap<PluralCategory, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Validated runtime artifact for one locale.
pub struct Bundle {
    /// Pinned CLDR data version required by this artifact.
    pub cldr_version: String,
    /// Locale text direction.
    pub direction: TextDirection,
    /// Message entries keyed by content-derived entry ID.
    pub entries: BTreeMap<String, BundleEntry>,
    /// Checked parent locales ending in the source locale for target bundles.
    pub fallback_chain: Vec<String>,
    /// Confirms that parent-locale rows were materialized at build time.
    pub fallbacks_flattened: bool,
    /// Wire-format discriminator, currently `trox-bundle`.
    pub format: String,
    /// Placeholder isolation policy.
    pub isolation: IsolationPolicy,
    /// Canonical BCP-47 locale ID represented by the bundle.
    pub locale: String,
    /// Message-scoped facet IDs, in checked profile declaration order.
    pub message_facets: Vec<String>,
    /// Deterministic decimal-format data.
    pub number_format: NumberFormat,
    /// Pinned plural-selection conditions.
    pub plural_rules: PluralRules,
    /// Full fingerprint tying target and source catalogs together.
    pub source_catalog_fingerprint: String,
    /// Canonical BCP-47 source locale ID.
    pub source_locale: String,
    /// Runtime term surfaces keyed by stable term ID.
    pub terms: BTreeMap<String, BundleTerm>,
    /// Bundle wire-format version.
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One message family in a bundle.
pub struct BundleEntry {
    /// Source-authored placeholder contracts, present only in source bundles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<BTreeMap<String, ArgumentSchema>>,
    /// Signature of the source identity and placeholder contract (bundle 1.1+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_signature: Option<String>,
    /// Canonical source identity, present only in source bundles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityDescriptor>,
    /// Expanded target rows keyed by deterministic row ID.
    pub rows: BTreeMap<String, BundleRow>,
    /// Full source identity signature used for compatibility checks.
    pub source_signature: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One translated expansion row.
pub struct BundleRow {
    /// Canonical selector and facet decisions identifying this row.
    pub expansion: ExpansionDescriptor,
    /// Locale from which this flattened row was inherited.
    pub origin_locale: String,
    /// Validated target pattern containing text and named placeholders.
    pub translation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical data hashed to produce a row ID.
pub struct ExpansionDescriptor {
    /// Full source entry signature.
    pub entry_signature: String,
    /// Ordered selector decisions followed by ordered message-facet decisions.
    pub path: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ExpansionStep {
    Exact {
        branch: usize,
        exact: u64,
        ordinal: bool,
    },
    Category {
        branch: usize,
        category: PluralCategory,
        ordinal: bool,
    },
    Select {
        branch: usize,
    },
    Facet {
        argument: String,
        facet: String,
        value: String,
    },
}

type ExpansionKey = Vec<ExpansionStep>;

impl ExpansionStep {
    fn to_json(&self) -> Value {
        match self {
            Self::Exact {
                branch,
                exact,
                ordinal,
            } => json!({
                "branch": branch,
                "kind": if *ordinal { "ordinal" } else { "plural" },
                "match": { "exact": exact }
            }),
            Self::Category {
                branch,
                category,
                ordinal,
            } => json!({
                "branch": branch,
                "kind": if *ordinal { "ordinal" } else { "plural" },
                "match": { "category": category.as_str() }
            }),
            Self::Select { branch } => json!({ "branch": branch, "kind": "select" }),
            Self::Facet {
                argument,
                facet,
                value,
            } => json!({
                "argument": argument,
                "facet": facet,
                "kind": "facet",
                "value": value
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Runtime facets and forms for one stable term ID.
pub struct BundleTerm {
    /// Locale-owned intrinsic facet classifications.
    pub facets: BTreeMap<String, String>,
    /// Default and named surfaces keyed by form ID.
    pub forms: BTreeMap<String, BundleTermForm>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
/// A scalar or cardinally inflected term form.
pub enum BundleTermForm {
    /// A form that forbids a number argument.
    Scalar {
        /// Locale from which this surface was inherited.
        origin_locale: String,
        /// Surface text.
        text: String,
    },
    /// A form requiring a cardinal number argument.
    Number {
        /// Surfaces keyed by cardinal category, with `other` as fallback.
        values: BTreeMap<PluralCategory, BundleTermSurface>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One localized surface of a numbered term form.
pub struct BundleTermSurface {
    /// Locale from which this surface was inherited.
    pub origin_locale: String,
    /// Surface text.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Stable machine-readable categories emitted by runtime recovery.
pub enum DiagnosticCode {
    /// Target and source catalog fingerprints differ.
    CatalogMismatch,
    /// A compatible target message entry is unavailable.
    MissingMessage,
    /// The selected target expansion row is unavailable.
    MissingRow,
    /// A translated placeholder has no runtime binding.
    MissingArgument,
    /// A dynamic term ID is absent from the active bundle.
    UnknownTerm,
    /// A term lacks the requested form or number contract.
    MissingTermForm,
    /// A translated pattern has invalid brace syntax.
    MalformedTranslation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Structured event emitted when normal resolution performs recovery.
pub struct Diagnostic {
    /// Stable recovery category.
    pub code: DiagnosticCode,
    /// Affected message ID, when the event concerns one message.
    pub entry_id: Option<String>,
    /// Human-readable details suitable for logs and debugging.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Rendered text plus whether the whole message used its source fallback.
pub struct ResolveOutcome {
    /// Display-ready resolved text.
    pub text: String,
    /// True when no compatible target row could be used.
    pub used_source_fallback: bool,
}

/// One display-ready run produced from a localized message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedLocalizedPart<'a, T> {
    /// Translator-authored literal text between placeholders.
    Literal {
        /// Display-ready literal text.
        value: String,
    },
    /// One occurrence of a named placeholder in the selected translation.
    Placeholder {
        /// Semantic placeholder name from the source message contract.
        name: String,
        /// Display-ready placeholder surface, including configured bidi isolation.
        value: String,
        /// Opaque application metadata associated with this placeholder, if any.
        annotation: Option<&'a T>,
    },
}

impl<T> ResolvedLocalizedPart<'_, T> {
    fn value(&self) -> &str {
        match self {
            Self::Literal { value } | Self::Placeholder { value, .. } => value,
        }
    }
}

/// Structured resolution plus whether the complete message used source fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLocalizedPartsOutcome<'a, T> {
    /// Display-ready literal and placeholder runs in translated order.
    pub parts: Vec<ResolvedLocalizedPart<'a, T>>,
    /// True when no compatible target row could be used.
    pub used_source_fallback: bool,
}

impl Bundle {
    /// Parses, canonical-encoding checks, and structurally validates bundle JSON.
    pub fn from_canonical_json(input: &str) -> Result<Self, DeserializeError> {
        let value: Value = serde_json::from_str(input)?;
        let canonical = canonical_json(&value)
            .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
        if canonical != input {
            return Err(DeserializeError::NoncanonicalJson);
        }
        let bundle: Bundle = serde_json::from_value(value)?;
        bundle.validate()?;
        Ok(bundle)
    }

    /// Encodes the bundle as deterministic RFC 8785 JSON.
    pub fn to_canonical_json(&self) -> Result<String, crate::SerializeError> {
        canonical_json(self)
    }

    /// Builds the identity and term authorization catalog from a source bundle.
    pub fn source_catalog(&self) -> Result<SourceCatalog, DeserializeError> {
        self.validate()?;
        if self.locale != self.source_locale {
            return Err(DeserializeError::InvalidBundle(
                "SourceCatalog requires a source-locale bundle".into(),
            ));
        }
        SourceCatalog::from_validated_bundle(self)
    }

    /// Validates all bundle structure, identifiers, locale data, and row hashes.
    pub fn validate(&self) -> Result<(), DeserializeError> {
        validation::validate_bundle(self)
    }
}

type DiagnosticHook = Arc<dyn Fn(Diagnostic) + Send + Sync>;

#[derive(Debug)]
struct CompiledBundle {
    wire: Bundle,
    plurals: CompiledPluralRules,
    row_index: BTreeMap<String, BTreeMap<ExpansionKey, String>>,
}

impl CompiledBundle {
    fn new(wire: Bundle) -> Result<Self, DeserializeError> {
        let plurals = CompiledPluralRules::compile(&wire.plural_rules)?;
        let row_index = wire
            .entries
            .iter()
            .map(|(entry_id, entry)| {
                let rows = entry
                    .rows
                    .iter()
                    .map(|(row_id, row)| {
                        expansion_key_from_json(&row.expansion.path)
                            .map(|key| (key, row_id.clone()))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?;
                Ok((entry_id.clone(), rows))
            })
            .collect::<Result<BTreeMap<_, _>, DeserializeError>>()?;
        Ok(Self {
            wire,
            plurals,
            row_index,
        })
    }

    fn category(&self, ordinal: bool, value: u64) -> PluralCategory {
        self.plurals.category(ordinal, value)
    }

    fn row<'a>(&'a self, entry_id: &str, key: &ExpansionKey) -> Option<&'a BundleRow> {
        let row_id = self.row_index.get(entry_id)?.get(key)?;
        self.entries.get(entry_id)?.rows.get(row_id)
    }
}

impl Deref for CompiledBundle {
    type Target = Bundle;

    fn deref(&self) -> &Self::Target {
        &self.wire
    }
}

/// Resolves immutable localized values through source configuration or a bundle pair.
pub struct Localizer {
    target: CompiledBundle,
    source: CompiledBundle,
    catalog: SourceCatalog,
    source_development: bool,
    hook: Option<DiagnosticHook>,
    pending: Vec<Diagnostic>,
}

impl fmt::Debug for Localizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Localizer")
            .field("target", &self.target.locale)
            .field("source", &self.source.locale)
            .field("source_development", &self.source_development)
            .finish()
    }
}

impl Localizer {
    /// Constructs a bundle-free localizer for ordinary source-language development.
    ///
    /// Message patterns and selectors are read directly from each
    /// [`LocalizedString`]. The supplied configuration contributes only pinned
    /// locale behavior, isolation, and human-authored term surfaces; no
    /// extraction report or generated bundle is consulted.
    pub fn for_source(source: SourceLocale) -> Result<Self, DeserializeError> {
        let bundle = source.into_runtime_bundle();
        bundle.validate()?;
        let catalog = SourceCatalog::from_validated_bundle(&bundle)?;
        Ok(Self {
            target: CompiledBundle::new(bundle.clone())?,
            source: CompiledBundle::new(bundle)?,
            catalog,
            source_development: true,
            hook: None,
            pending: vec![],
        })
    }

    /// Constructs a recovering localizer and permits entry-level compatibility on catalog mismatch.
    pub fn new(target: Bundle, source: Bundle) -> Result<Self, DeserializeError> {
        Self::new_inner(target, source, false)
    }
    /// Constructs a localizer that rejects any target/source catalog mismatch.
    pub fn new_strict(target: Bundle, source: Bundle) -> Result<Self, DeserializeError> {
        Self::new_inner(target, source, true)
    }

    fn new_inner(target: Bundle, source: Bundle, strict: bool) -> Result<Self, DeserializeError> {
        target.validate()?;
        source.validate()?;
        if source.locale != source.source_locale || target.source_locale != source.locale {
            return Err(DeserializeError::InvalidBundle(
                "target and source locale relationship is invalid".into(),
            ));
        }
        if strict && target.source_catalog_fingerprint != source.source_catalog_fingerprint {
            return Err(DeserializeError::InvalidBundle(
                "source catalog fingerprint mismatch".into(),
            ));
        }
        let catalog = SourceCatalog::from_validated_bundle(&source)?;
        let mut pending = Vec::new();
        if target.source_catalog_fingerprint != source.source_catalog_fingerprint {
            pending.push(Diagnostic { code: DiagnosticCode::CatalogMismatch, entry_id: None, message: "target and source catalog fingerprints differ; compatible entries remain usable".into() });
        }
        let localizer = Self {
            target: CompiledBundle::new(target)?,
            source: CompiledBundle::new(source)?,
            catalog,
            source_development: false,
            hook: None,
            pending,
        };
        localizer.validate_entry_compatibility()?;
        Ok(localizer)
    }

    /// Installs a thread-safe hook and flushes diagnostics queued during construction.
    pub fn with_diagnostic_hook(
        mut self,
        hook: impl Fn(Diagnostic) + Send + Sync + 'static,
    ) -> Self {
        let hook: DiagnosticHook = Arc::new(hook);
        for diagnostic in self.pending.drain(..) {
            invoke_diagnostic_hook(&hook, diagnostic);
        }
        self.hook = Some(hook);
        self
    }
    /// Returns the source authorization catalog used for wire decoding.
    ///
    /// A source-development localizer has no extracted message authorization
    /// entries, so its catalog accepts only the reserved [`crate::ls`] wire
    /// shape. Decode application message wires with a production source bundle.
    pub fn source_catalog(&self) -> &SourceCatalog {
        &self.catalog
    }
    /// Decodes a canonical localized value through this localizer's source catalog.
    ///
    /// Source-development mode intentionally cannot authorize ordinary message
    /// wires because no extracted catalog exists.
    pub fn localized_string_from_json(
        &self,
        input: &str,
    ) -> Result<LocalizedString, DeserializeError> {
        self.catalog.localized_string_from_json(input)
    }

    fn validate_entry_compatibility(&self) -> Result<(), DeserializeError> {
        for (id, target_entry) in &self.target.entries {
            let Some(source_entry) = self.source.entries.get(id) else {
                continue;
            };
            let source_contract = source_entry.contract_signature.clone().or_else(|| {
                Some(
                    crate::value::contract_signature(
                        source_entry.identity.as_ref()?,
                        source_entry.arguments.as_ref()?,
                    )
                    .expect("validated source contracts are canonically encodable"),
                )
            });
            if target_entry.source_signature != source_entry.source_signature
                || (target_entry.contract_signature.is_some()
                    && target_entry.contract_signature != source_contract)
            {
                continue;
            }
            let identity = source_entry
                .identity
                .as_ref()
                .expect("validated source identity");
            let declared = declared_placeholders(&identity.pattern)
                .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
            for row in target_entry.rows.values() {
                validate_message_expansion(&identity.pattern, &row.expansion.path, &self.target)?;
                let translated = placeholders(&row.translation)
                    .map_err(|error| DeserializeError::InvalidBundle(error.to_string()))?;
                if !translated.is_subset(&declared) {
                    return Err(DeserializeError::InvalidBundle(format!(
                        "translation for `{id}` contains unknown placeholders"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Resolves through the live source pattern or target row and returns the first failure.
    pub fn resolve_checked(&self, value: &LocalizedString) -> Result<String, ResolveError> {
        let no_annotation = |_: &str| Option::<&()>::None;
        self.resolve_parts_checked_with(value, &no_annotation)
            .map(|parts| join_resolved_parts(&parts))
    }

    fn target_row<'a>(&'a self, value: &LocalizedString) -> Result<&'a BundleRow, ResolveError> {
        let _entry = self
            .target
            .entries
            .get(value.entry_id())
            .filter(|entry| {
                entry.source_signature == value.source_signature()
                    && entry
                        .contract_signature
                        .as_deref()
                        .is_none_or(|signature| signature == value.contract_signature())
            })
            .ok_or_else(|| ResolveError::MissingMessage {
                entry_id: value.entry_id().to_owned(),
            })?;
        let selection = select_pattern(&self.target, value)?;
        if let Some(row) = self.target.row(value.entry_id(), &selection.path) {
            return Ok(row);
        }
        let expansion = ExpansionDescriptor {
            entry_signature: value.source_signature().to_owned(),
            path: selection.path.iter().map(ExpansionStep::to_json).collect(),
        };
        let signature = signature(&expansion)
            .map_err(|error| ResolveError::MalformedTranslation(error.to_string()))?;
        let row_id = short_id("row1_", &signature);
        Err(ResolveError::MissingRow { row_id })
    }

    /// Resolves infallibly, emitting diagnostics and preserving visible recovery markers.
    pub fn resolve(&self, value: &LocalizedString) -> String {
        self.resolve_outcome(value).text
    }

    /// Resolves infallibly and reports whether the entire message fell back to source.
    pub fn resolve_outcome(&self, value: &LocalizedString) -> ResolveOutcome {
        let no_annotation = |_: &str| Option::<&()>::None;
        let outcome = self.resolve_parts_outcome_with(value, &no_annotation);
        ResolveOutcome {
            text: join_resolved_parts(&outcome.parts),
            used_source_fallback: outcome.used_source_fallback,
        }
    }

    /// Resolves an annotated value through the target row and returns the first failure.
    pub fn resolve_parts_checked<'a, T>(
        &self,
        value: &'a AnnotatedLocalizedString<T>,
    ) -> Result<Vec<ResolvedLocalizedPart<'a, T>>, ResolveError> {
        let annotation_for = |name: &str| value.annotations().get(name);
        self.resolve_parts_checked_with(value.localized(), &annotation_for)
    }

    /// Resolves annotated parts with the same diagnostics and source recovery as [`Self::resolve`].
    pub fn resolve_parts<'a, T>(
        &self,
        value: &'a AnnotatedLocalizedString<T>,
    ) -> Vec<ResolvedLocalizedPart<'a, T>> {
        self.resolve_parts_outcome(value).parts
    }

    /// Resolves annotated parts and reports whether the complete message used source fallback.
    pub fn resolve_parts_outcome<'a, T>(
        &self,
        value: &'a AnnotatedLocalizedString<T>,
    ) -> ResolvedLocalizedPartsOutcome<'a, T> {
        let annotation_for = |name: &str| value.annotations().get(name);
        self.resolve_parts_outcome_with(value.localized(), &annotation_for)
    }

    fn resolve_parts_checked_with<'a, T, F>(
        &self,
        value: &LocalizedString,
        annotation_for: &F,
    ) -> Result<Vec<ResolvedLocalizedPart<'a, T>>, ResolveError>
    where
        F: Fn(&str) -> Option<&'a T>,
    {
        if let Some(pattern) = value.asserted_localized_pattern() {
            return self.interpolate_parts(pattern, value, false, annotation_for);
        }
        if self.source_development {
            let selection = select_pattern(&self.source, value)?;
            return self.interpolate_parts(&selection.text, value, true, annotation_for);
        }
        let row = self.target_row(value)?;
        self.interpolate_parts(&row.translation, value, true, annotation_for)
    }

    fn resolve_parts_outcome_with<'a, T, F>(
        &self,
        value: &LocalizedString,
        annotation_for: &F,
    ) -> ResolvedLocalizedPartsOutcome<'a, T>
    where
        F: Fn(&str) -> Option<&'a T>,
    {
        if let Some(pattern) = value.asserted_localized_pattern() {
            let parts = self
                .interpolate_parts_recovering(pattern, value, false, annotation_for)
                .unwrap_or_else(|_| unreachable!("asserted-localized patterns are validated"));
            return ResolvedLocalizedPartsOutcome {
                parts,
                used_source_fallback: false,
            };
        }
        if self.source_development {
            let parts = select_pattern(&self.source, value)
                .and_then(|selection| {
                    self.interpolate_parts_recovering(&selection.text, value, false, annotation_for)
                })
                .unwrap_or_else(|error| {
                    self.emit(resolve_diagnostic(value.entry_id(), &error));
                    vec![ResolvedLocalizedPart::Literal {
                        value: format!("⟦{}⟧", value.entry_id()),
                    }]
                });
            return ResolvedLocalizedPartsOutcome {
                parts,
                used_source_fallback: false,
            };
        }
        match self.target_row(value) {
            Ok(row) => match self.interpolate_parts_recovering(
                &row.translation,
                value,
                true,
                annotation_for,
            ) {
                Ok(parts) => ResolvedLocalizedPartsOutcome {
                    parts,
                    used_source_fallback: false,
                },
                Err(error) => self.resolve_source_parts_after(value, error, annotation_for),
            },
            Err(error) => self.resolve_source_parts_after(value, error, annotation_for),
        }
    }

    fn resolve_source_parts_after<'a, T, F>(
        &self,
        value: &LocalizedString,
        target_error: ResolveError,
        annotation_for: &F,
    ) -> ResolvedLocalizedPartsOutcome<'a, T>
    where
        F: Fn(&str) -> Option<&'a T>,
    {
        self.emit(resolve_diagnostic(value.entry_id(), &target_error));
        match select_pattern(&self.source, value).and_then(|selection| {
            self.interpolate_parts_recovering(&selection.text, value, false, annotation_for)
        }) {
            Ok(parts) => ResolvedLocalizedPartsOutcome {
                parts,
                used_source_fallback: true,
            },
            Err(source_error) => {
                self.emit(resolve_diagnostic(value.entry_id(), &source_error));
                ResolvedLocalizedPartsOutcome {
                    parts: vec![ResolvedLocalizedPart::Literal {
                        value: format!("⟦{}⟧", value.entry_id()),
                    }],
                    used_source_fallback: true,
                }
            }
        }
    }

    fn interpolate_parts_recovering<'a, T, F>(
        &self,
        pattern: &str,
        value: &LocalizedString,
        prefer_target_terms: bool,
        annotation_for: &F,
    ) -> Result<Vec<ResolvedLocalizedPart<'a, T>>, ResolveError>
    where
        F: Fn(&str) -> Option<&'a T>,
    {
        let mut output = Vec::new();
        let bytes = pattern.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'{' && bytes.get(index + 1) == Some(&b'{') {
                push_literal(&mut output, '{');
                index += 2;
                continue;
            }
            if bytes[index] == b'}' && bytes.get(index + 1) == Some(&b'}') {
                push_literal(&mut output, '}');
                index += 2;
                continue;
            }
            if bytes[index] == b'{' {
                let Some(end) = pattern[index + 1..].find('}') else {
                    return Err(ResolveError::MalformedTranslation(
                        "unclosed placeholder".into(),
                    ));
                };
                let name = &pattern[index + 1..index + 1 + end];
                let surface = match value.arguments().get(name) {
                    None => {
                        self.emit(resolve_diagnostic(
                            value.entry_id(),
                            &ResolveError::MissingArgument { name: name.into() },
                        ));
                        format!("{{{name}}}")
                    }
                    Some(Argument::Term {
                        term_id,
                        form,
                        number,
                    }) => {
                        match self.term_surface(
                            term_id.as_str(),
                            form.as_deref(),
                            *number,
                            prefer_target_terms,
                        ) {
                            Ok(surface) => surface,
                            Err(error) => {
                                self.emit(resolve_diagnostic(value.entry_id(), &error));
                                self.source
                                    .terms
                                    .get(term_id.as_str())
                                    .and_then(|term| term.forms.get("$default"))
                                    .and_then(|form| match form {
                                        BundleTermForm::Scalar { text, .. } => Some(text.clone()),
                                        _ => None,
                                    })
                                    .unwrap_or_else(|| format!("⟦term:{}⟧", term_id.as_str()))
                            }
                        }
                    }
                    Some(Argument::Opaque { value }) => {
                        let nested = LocalizedString::from_validated_wire((**value).clone());
                        self.resolve(&nested)
                    }
                    Some(argument) => self
                        .argument_surface(argument, prefer_target_terms)
                        .unwrap_or_else(|_| format!("{{{name}}}")),
                };
                push_placeholder(
                    &mut output,
                    name,
                    self.isolate_surface(surface),
                    annotation_for(name),
                );
                index += end + 2;
                continue;
            }
            if bytes[index] == b'}' {
                return Err(ResolveError::MalformedTranslation(
                    "unmatched closing brace".into(),
                ));
            }
            let ch = pattern[index..].chars().next().expect("valid UTF-8");
            push_literal(&mut output, ch);
            index += ch.len_utf8();
        }
        Ok(output)
    }

    fn interpolate_parts<'a, T, F>(
        &self,
        pattern: &str,
        value: &LocalizedString,
        prefer_target_terms: bool,
        annotation_for: &F,
    ) -> Result<Vec<ResolvedLocalizedPart<'a, T>>, ResolveError>
    where
        F: Fn(&str) -> Option<&'a T>,
    {
        let mut output = Vec::new();
        let bytes = pattern.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'{' if bytes.get(index + 1) == Some(&b'{') => {
                    push_literal(&mut output, '{');
                    index += 2;
                }
                b'}' if bytes.get(index + 1) == Some(&b'}') => {
                    push_literal(&mut output, '}');
                    index += 2;
                }
                b'{' => {
                    let end = pattern[index + 1..].find('}').ok_or_else(|| {
                        ResolveError::MalformedTranslation("unclosed placeholder".into())
                    })?;
                    let name = &pattern[index + 1..index + 1 + end];
                    let argument = value
                        .arguments()
                        .get(name)
                        .ok_or_else(|| ResolveError::MissingArgument { name: name.into() })?;
                    let surface = self.argument_surface(argument, prefer_target_terms)?;
                    push_placeholder(
                        &mut output,
                        name,
                        self.isolate_surface(surface),
                        annotation_for(name),
                    );
                    index += end + 2;
                }
                b'}' => {
                    return Err(ResolveError::MalformedTranslation(
                        "unmatched closing brace".into(),
                    ));
                }
                _ => {
                    let ch = pattern[index..].chars().next().expect("valid UTF-8");
                    push_literal(&mut output, ch);
                    index += ch.len_utf8();
                }
            }
        }
        Ok(output)
    }

    fn isolate_surface(&self, surface: String) -> String {
        if self.target.isolation == IsolationPolicy::Isolate {
            format!("\u{2068}{surface}\u{2069}")
        } else {
            surface
        }
    }

    fn argument_surface(
        &self,
        argument: &Argument,
        prefer_target: bool,
    ) -> Result<String, ResolveError> {
        match argument {
            Argument::Text { value } => Ok(value.clone()),
            Argument::Number { value } => Ok(format_number(*value, &self.target.number_format)),
            Argument::Boolean { value } => Ok(value.to_string()),
            Argument::Opaque { value } => {
                let nested = LocalizedString::from_validated_wire((**value).clone());
                if prefer_target {
                    self.resolve_checked(&nested)
                } else {
                    Ok(self.resolve(&nested))
                }
            }
            Argument::Term {
                term_id,
                form,
                number,
            } => self.term_surface(term_id.as_str(), form.as_deref(), *number, prefer_target),
        }
    }

    fn term_surface(
        &self,
        term_id: &str,
        form: Option<&str>,
        number: Option<u64>,
        prefer_target: bool,
    ) -> Result<String, ResolveError> {
        let form_id = form.unwrap_or("$default");
        let bundle = if prefer_target {
            &self.target
        } else {
            &self.source
        };
        let term = bundle
            .terms
            .get(term_id)
            .ok_or_else(|| ResolveError::UnknownTerm {
                term_id: term_id.into(),
            })?;
        let form = term
            .forms
            .get(form_id)
            .ok_or_else(|| ResolveError::MissingTermForm {
                term_id: term_id.into(),
                form: form_id.into(),
            })?;
        match (form, number) {
            (BundleTermForm::Scalar { text, .. }, None) => Ok(text.clone()),
            (BundleTermForm::Number { values }, Some(number)) => {
                let category = bundle.category(false, number);
                values
                    .get(&category)
                    .or_else(|| values.get(&PluralCategory::Other))
                    .map(|surface| surface.text.clone())
                    .ok_or_else(|| ResolveError::MissingTermForm {
                        term_id: term_id.into(),
                        form: form_id.into(),
                    })
            }
            _ => Err(ResolveError::MissingTermForm {
                term_id: term_id.into(),
                form: form_id.into(),
            }),
        }
    }

    fn emit(&self, diagnostic: Diagnostic) {
        if let Some(hook) = &self.hook {
            invoke_diagnostic_hook(hook, diagnostic);
        }
    }
}

fn push_literal<'a, T>(parts: &mut Vec<ResolvedLocalizedPart<'a, T>>, ch: char) {
    if let Some(ResolvedLocalizedPart::Literal { value }) = parts.last_mut() {
        value.push(ch);
    } else {
        parts.push(ResolvedLocalizedPart::Literal {
            value: ch.to_string(),
        });
    }
}

fn push_placeholder<'a, T>(
    parts: &mut Vec<ResolvedLocalizedPart<'a, T>>,
    name: &str,
    value: String,
    annotation: Option<&'a T>,
) {
    parts.push(ResolvedLocalizedPart::Placeholder {
        name: name.to_owned(),
        value,
        annotation,
    });
}

fn join_resolved_parts<T>(parts: &[ResolvedLocalizedPart<'_, T>]) -> String {
    parts.iter().map(ResolvedLocalizedPart::value).collect()
}

fn invoke_diagnostic_hook(hook: &DiagnosticHook, diagnostic: Diagnostic) {
    let _ = catch_unwind(AssertUnwindSafe(|| hook(diagnostic)));
}

struct PatternSelection {
    text: String,
    path: ExpansionKey,
}

fn select_pattern(
    bundle: &CompiledBundle,
    value: &LocalizedString,
) -> Result<PatternSelection, ResolveError> {
    let mut path = Vec::new();
    let mut structural_path = Vec::new();
    let text = walk_pattern(
        bundle,
        &value.identity().pattern,
        value.selectors(),
        &mut structural_path,
        &mut path,
    )?;
    for (argument_name, argument) in value.arguments() {
        if let Argument::Term { term_id, .. } = argument
            && let Some(term) = bundle.terms.get(term_id.as_str())
        {
            for facet_id in &bundle.message_facets {
                if let Some(facet_value) = term.facets.get(facet_id) {
                    path.push(ExpansionStep::Facet {
                        argument: argument_name.clone(),
                        facet: facet_id.clone(),
                        value: facet_value.clone(),
                    });
                }
            }
        }
    }
    Ok(PatternSelection { text, path })
}

fn walk_pattern(
    bundle: &CompiledBundle,
    pattern: &Pattern,
    selectors: &[SelectorRecord],
    structural_path: &mut Vec<usize>,
    expansion: &mut ExpansionKey,
) -> Result<String, ResolveError> {
    match pattern {
        Pattern::Text { text } => Ok(text.clone()),
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            let is_ordinal = matches!(pattern, Pattern::Ordinal { .. });
            let record = selector_at_path(selectors, structural_path).ok_or_else(|| {
                ResolveError::MalformedTranslation("missing selector record".into())
            })?;
            let number = match (is_ordinal, record) {
                (false, SelectorRecord::Plural { value, .. })
                | (true, SelectorRecord::Ordinal { value, .. }) => *value,
                _ => {
                    return Err(ResolveError::MalformedTranslation(
                        "selector record kind mismatch".into(),
                    ));
                }
            };
            let exact = branches
                .iter()
                .position(|branch| branch.key.exact() == Some(number));
            let category = bundle.category(is_ordinal, number);
            let index = exact
                .or_else(|| {
                    branches
                        .iter()
                        .position(|branch| branch.key.category() == Some(category))
                })
                .or_else(|| {
                    branches
                        .iter()
                        .position(|branch| branch.key.category() == Some(PluralCategory::Other))
                })
                .ok_or_else(|| {
                    ResolveError::MalformedTranslation("numeric selector lacks fallback".into())
                })?;
            let step = if exact.is_some() {
                ExpansionStep::Exact {
                    branch: index,
                    exact: number,
                    ordinal: is_ordinal,
                }
            } else {
                ExpansionStep::Category {
                    branch: index,
                    category,
                    ordinal: is_ordinal,
                }
            };
            expansion.push(step);
            structural_path.push(index);
            let result = walk_pattern(
                bundle,
                &branches[index].pattern,
                selectors,
                structural_path,
                expansion,
            );
            structural_path.pop();
            result
        }
        Pattern::Select { branches } => {
            let record = selector_at_path(selectors, structural_path).ok_or_else(|| {
                ResolveError::MalformedTranslation("missing selector record".into())
            })?;
            let (branch_keys, current) = match record {
                SelectorRecord::Select {
                    branch_keys, value, ..
                } => (branch_keys, value),
                _ => {
                    return Err(ResolveError::MalformedTranslation(
                        "selector record kind mismatch".into(),
                    ));
                }
            };
            let index = branch_keys
                .iter()
                .position(|key| key == current)
                .unwrap_or(branches.len() - 1);
            expansion.push(ExpansionStep::Select { branch: index });
            structural_path.push(index);
            let child = match &branches[index] {
                SelectIdentityBranch::When { pattern }
                | SelectIdentityBranch::Otherwise { pattern } => pattern,
            };
            let result = walk_pattern(bundle, child, selectors, structural_path, expansion);
            structural_path.pop();
            result
        }
    }
}

fn selector_at_path<'a>(
    selectors: &'a [SelectorRecord],
    path: &[usize],
) -> Option<&'a SelectorRecord> {
    selectors
        .binary_search_by(|record| record.path().cmp(path))
        .ok()
        .map(|index| &selectors[index])
}

fn declared_placeholders(pattern: &Pattern) -> Result<BTreeSet<String>, crate::TroxValueError> {
    let mut result = BTreeSet::new();
    match pattern {
        Pattern::Text { text } => result.extend(placeholders(text)?),
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            for branch in branches {
                result.extend(declared_placeholders(&branch.pattern)?);
            }
        }
        Pattern::Select { branches } => {
            for branch in branches {
                result.extend(declared_placeholders(branch.pattern())?);
            }
        }
    }
    Ok(result)
}

fn resolve_diagnostic(entry_id: &str, error: &ResolveError) -> Diagnostic {
    let code = match error {
        ResolveError::MissingMessage { .. } => DiagnosticCode::MissingMessage,
        ResolveError::MissingRow { .. } => DiagnosticCode::MissingRow,
        ResolveError::MissingArgument { .. } => DiagnosticCode::MissingArgument,
        ResolveError::UnknownTerm { .. } => DiagnosticCode::UnknownTerm,
        ResolveError::MissingTermForm { .. } => DiagnosticCode::MissingTermForm,
        ResolveError::MalformedTranslation(_) => DiagnosticCode::MalformedTranslation,
    };
    Diagnostic {
        code,
        entry_id: Some(entry_id.into()),
        message: error.to_string(),
    }
}
