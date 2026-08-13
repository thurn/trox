use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

use crate::TroxValueError;
use crate::canonical::{short_id, signature, signature_hex};
use crate::value::{LocalizedStringWire, identity_id_pair};

/// Maximum number of selector levels permitted in a message pattern.
pub const MAX_SELECTOR_DEPTH: usize = 16;
/// Maximum number of branches permitted on one selector.
pub const MAX_SELECTOR_BRANCHES: usize = 256;
/// Maximum number of nodes permitted in one message pattern tree.
pub const MAX_PATTERN_NODES: usize = 4096;
/// Maximum number of placeholder arguments permitted in one message.
pub const MAX_ARGUMENTS: usize = 256;
/// Maximum number of expanded translation rows permitted for one message entry.
pub const MAX_EXPANDED_ROWS: usize = 4096;
/// Largest integer that has an exact representation in the wire format's number model.
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Major and minor version of a Trox wire value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    /// Wire format major version.
    pub major: u32,
    /// Wire format minor version.
    pub minor: u32,
}

impl Version {
    /// Version 1.0 of the Trox wire format.
    pub const V1: Self = Self { major: 1, minor: 0 };
    /// Version 1.1 of the Trox wire format.
    pub const V1_1: Self = Self { major: 1, minor: 1 };

    pub(crate) fn is_supported_v1(self) -> bool {
        self.major == 1 && self.minor <= 1
    }
}

/// Locale-independent fields used to derive a message's stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityDescriptor {
    /// Version of the identity canonicalization rules.
    pub identity_version: u32,
    /// Optional stable identifier distinguishing semantically different uses of the same text.
    pub meaning: Option<String>,
    /// Source-authored message pattern.
    pub pattern: Pattern,
}

/// Locale-independent source pattern for a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Pattern {
    /// A text leaf, which may contain named placeholders.
    Text {
        /// Source text for the leaf.
        text: String,
    },
    /// A cardinal-plural selector.
    Plural {
        /// Canonically ordered exact and category branches.
        branches: Vec<NumericBranch>,
    },
    /// An ordinal-plural selector.
    Ordinal {
        /// Canonically ordered exact and category branches.
        branches: Vec<NumericBranch>,
    },
    /// A stable semantic selector.
    Select {
        /// Ordered conditional branches followed by an otherwise branch.
        branches: Vec<SelectIdentityBranch>,
    },
}

/// One branch of a cardinal or ordinal numeric selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericBranch {
    /// Exact value or plural category selecting this branch.
    pub key: NumericBranchKey,
    /// Pattern rendered when the branch is selected.
    pub pattern: Pattern,
}

/// Identity-bearing key for a numeric selector branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum NumericBranchKey {
    /// An exact non-negative integer match.
    Exact {
        /// Integer that selects this branch.
        exact: u64,
    },
    /// A cardinal plural-category match.
    Plural {
        /// CLDR cardinal category that selects this branch.
        plural: PluralCategory,
    },
    /// An ordinal plural-category match.
    Ordinal {
        /// CLDR ordinal category that selects this branch.
        ordinal: PluralCategory,
    },
}

impl NumericBranchKey {
    pub(crate) fn exact(&self) -> Option<u64> {
        match self {
            Self::Exact { exact } => Some(*exact),
            _ => None,
        }
    }

    pub(crate) fn category(&self) -> Option<PluralCategory> {
        match self {
            Self::Plural { plural } => Some(*plural),
            Self::Ordinal { ordinal } => Some(*ordinal),
            Self::Exact { .. } => None,
        }
    }
}

/// Identity-bearing branch of a stable semantic selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
pub enum SelectIdentityBranch {
    /// A branch associated with a runtime key stored in the selector record.
    When {
        /// Pattern rendered when the corresponding key matches.
        pattern: Pattern,
    },
    /// Final fallback branch when no key matches.
    Otherwise {
        /// Pattern rendered when no conditional branch matches.
        pattern: Pattern,
    },
}

impl SelectIdentityBranch {
    pub(crate) fn pattern(&self) -> &Pattern {
        match self {
            Self::When { pattern } | Self::Otherwise { pattern } => pattern,
        }
    }
}

/// CLDR plural category used by cardinal and ordinal patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluralCategory {
    /// The `zero` category.
    Zero,
    /// The `one` category.
    One,
    /// The `two` category.
    Two,
    /// The `few` category.
    Few,
    /// The `many` category.
    Many,
    /// The required catch-all `other` category.
    Other,
}

impl PluralCategory {
    /// Categories in their canonical wire ordering.
    pub const CANONICAL: [Self; 6] = [
        Self::Zero,
        Self::One,
        Self::Two,
        Self::Few,
        Self::Many,
        Self::Other,
    ];

    /// Returns the lowercase CLDR spelling of this category.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::One => "one",
            Self::Two => "two",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }
}

/// Runtime key used to select a semantic branch.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SelectorKey {
    /// Stable application-defined string key.
    String(String),
    /// Boolean selector key.
    Boolean(bool),
}

impl SelectorKey {
    pub(crate) fn validate(&self) -> Result<(), TroxValueError> {
        if let Self::String(value) = self {
            validate_stable_id(value, "selector key")?;
        }
        Ok(())
    }
}

/// Runtime selector input paired with its path in the identity pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum SelectorRecord {
    /// Runtime value for a cardinal-plural selector.
    Plural {
        /// Branch-index path from the pattern root to the selector.
        path: Vec<usize>,
        /// Non-negative integer being selected.
        value: u64,
    },
    /// Runtime value for an ordinal-plural selector.
    Ordinal {
        /// Branch-index path from the pattern root to the selector.
        path: Vec<usize>,
        /// Non-negative integer being selected.
        value: u64,
    },
    /// Runtime value and branch keys for a semantic selector.
    Select {
        /// Branch-index path from the pattern root to the selector.
        path: Vec<usize>,
        /// Keys corresponding to the pattern's ordered `when` branches.
        branch_keys: Vec<SelectorKey>,
        /// Runtime key being selected.
        value: SelectorKey,
    },
}

impl SelectorRecord {
    /// Returns the branch-index path from the pattern root to this selector.
    pub fn path(&self) -> &[usize] {
        match self {
            Self::Plural { path, .. } | Self::Ordinal { path, .. } | Self::Select { path, .. } => {
                path
            }
        }
    }
}

/// Validated, stable identifier for an application term.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TermId(String);

impl<'de> Deserialize<'de> for TermId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(serde::de::Error::custom)
    }
}

impl TermId {
    /// Creates a term ID, panicking when `value` is not a valid stable ID.
    ///
    /// Use [`Self::try_new`] when the ID originates outside source code.
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        if let Err(error) = validate_stable_id(&value, "term ID") {
            panic!("TROX_ASSERT {error}");
        }
        Self(value)
    }

    /// Creates a term ID after validating its stable-ID syntax.
    pub fn try_new(value: impl Into<String>) -> Result<Self, TroxValueError> {
        let value = value.into();
        validate_stable_id(&value, "term ID")?;
        Ok(Self(value))
    }

    /// Returns the validated string representation of this term ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reference to a term, optionally specifying a form and count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TermArgument {
    /// Referenced application term.
    pub term_id: TermId,
    /// Stable term form requested from the bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    /// Count supplied to a numbered term form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
}

/// Runtime value bound to a named placeholder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Argument {
    /// User- or application-provided text.
    Text {
        /// Text value inserted at resolution time.
        value: String,
    },
    /// Finite number formatted according to the target locale.
    Number {
        /// Numeric value inserted at resolution time.
        value: f64,
    },
    /// Boolean rendered as a scalar value.
    Boolean {
        /// Boolean value inserted at resolution time.
        value: bool,
    },
    /// Bundle-authorized term reference.
    Term {
        /// Referenced application term.
        term_id: TermId,
        /// Stable term form requested from the bundle.
        #[serde(skip_serializing_if = "Option::is_none")]
        form: Option<String>,
        /// Count supplied to a numbered term form.
        #[serde(skip_serializing_if = "Option::is_none")]
        number: Option<u64>,
    },
    /// Atomic localized value embedded without exposing its source text to translators.
    Opaque {
        /// Complete wire representation of the atomic nested value.
        value: Box<LocalizedStringWire>,
    },
}

/// Coarse placeholder category used to compare source and translated rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentKind {
    /// Text, number, or boolean value.
    Scalar,
    /// Term reference.
    Term,
    /// Atomic nested localized value.
    Opaque,
}

/// Source-authored contract for one visible placeholder binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ArgumentSchema {
    /// Text, finite number, or boolean runtime value.
    Scalar,
    /// Atomic localized value.
    Opaque,
    /// Project term with an exact form and number-presence contract.
    Term {
        /// Requested named form, or the term default when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        form: Option<String>,
        /// Whether the term binding requires a cardinal number.
        number: bool,
    },
}

/// Conversion contract used by [`crate::tx_args!`] to borrow application values.
pub trait IntoArgument {
    /// Converts the borrowed value into an owned Trox argument.
    fn to_argument(&self) -> Argument;
}

impl Argument {
    /// Converts a borrowed application value into an owned argument.
    pub fn from_borrowed<T: IntoArgument + ?Sized>(value: &T) -> Self {
        value.to_argument()
    }

    /// Returns the coarse category of this argument.
    pub fn kind(&self) -> ArgumentKind {
        match self {
            Self::Text { .. } | Self::Number { .. } | Self::Boolean { .. } => ArgumentKind::Scalar,
            Self::Term { .. } => ArgumentKind::Term,
            Self::Opaque { .. } => ArgumentKind::Opaque,
        }
    }
}

impl IntoArgument for String {
    fn to_argument(&self) -> Argument {
        Argument::Text {
            value: self.clone(),
        }
    }
}
impl IntoArgument for str {
    fn to_argument(&self) -> Argument {
        Argument::Text {
            value: self.to_owned(),
        }
    }
}
impl<T: IntoArgument + ?Sized> IntoArgument for &T {
    fn to_argument(&self) -> Argument {
        (*self).to_argument()
    }
}
impl IntoArgument for bool {
    fn to_argument(&self) -> Argument {
        Argument::Boolean { value: *self }
    }
}

macro_rules! scalar_integer {
    ($($ty:ty),* $(,)?) => {$(
        impl IntoArgument for $ty {
            fn to_argument(&self) -> Argument { Argument::Number { value: *self as f64 } }
        }
    )*};
}
scalar_integer!(i8, i16, i32, u8, u16, u32);

impl IntoArgument for TroxNumber {
    fn to_argument(&self) -> Argument {
        Argument::Number { value: self.0 }
    }
}
impl IntoArgument for TermArgument {
    fn to_argument(&self) -> Argument {
        Argument::Term {
            term_id: self.term_id.clone(),
            form: self.form.clone(),
            number: self.number,
        }
    }
}

/// Finite floating-point scalar accepted by the Trox wire model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TroxNumber(f64);

impl TroxNumber {
    /// Validates and normalizes a floating-point scalar.
    pub fn new(value: f64) -> Result<Self, TroxValueError> {
        if !value.is_finite() {
            return Err(TroxValueError::new(
                "trox.invalid-number",
                "Trox numbers must be finite",
            ));
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }

    /// Returns the validated numeric value.
    pub fn get(self) -> f64 {
        self.0
    }
}

/// Non-negative safe integer accepted by selectors and numbered terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TroxInteger(u64);

impl TroxInteger {
    /// Validates that `value` does not exceed 2^53 - 1.
    pub fn new(value: u64) -> Result<Self, TroxValueError> {
        if value > MAX_SAFE_INTEGER {
            return Err(TroxValueError::new(
                "trox.invalid-selector-number",
                "selector integer exceeds 2^53 - 1",
            ));
        }
        Ok(Self(value))
    }
    /// Returns the validated integer value.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u8> for TroxInteger {
    fn from(value: u8) -> Self {
        Self(value.into())
    }
}
impl From<u16> for TroxInteger {
    fn from(value: u16) -> Self {
        Self(value.into())
    }
}
impl From<u32> for TroxInteger {
    fn from(value: u32) -> Self {
        Self(value.into())
    }
}

pub(crate) fn validate_stable_id(value: &str, label: &str) -> Result<(), TroxValueError> {
    let valid = !value.is_empty()
        && value.len() <= 96
        && value.is_ascii()
        && value.as_bytes()[0].is_ascii_lowercase()
        && !value.ends_with(['.', '-'])
        && value.split(['.', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(TroxValueError::new(
            "trox.invalid-stable-id",
            format!("invalid {label} `{value}`"),
        ))
    }
}

pub(crate) fn validate_placeholder_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.is_ascii()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.split('_').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

pub(crate) fn validate_nfc(value: &str, label: &str) -> Result<(), TroxValueError> {
    if value.nfc().eq(value.chars()) {
        Ok(())
    } else {
        Err(TroxValueError::new(
            "trox.non-nfc",
            format!("{label} must be NFC"),
        ))
    }
}

pub(crate) fn placeholders(text: &str) -> Result<BTreeSet<String>, TroxValueError> {
    validate_nfc(text, "source text")?;
    let mut names = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' if bytes.get(index + 1) == Some(&b'{') => index += 2,
            b'}' if bytes.get(index + 1) == Some(&b'}') => index += 2,
            b'{' => {
                let rest = &text[index + 1..];
                let Some(end) = rest.find('}') else {
                    return Err(TroxValueError::new("trox.invalid-braces", "unclosed `{`"));
                };
                let name = &rest[..end];
                if !validate_placeholder_name(name) {
                    return Err(TroxValueError::new(
                        "trox.invalid-placeholder",
                        format!("invalid placeholder `{{{name}}}`"),
                    ));
                }
                names.insert(name.to_owned());
                index += end + 2;
            }
            b'}' => return Err(TroxValueError::new("trox.invalid-braces", "unmatched `}`")),
            _ => index += 1,
        }
    }
    Ok(names)
}

pub(crate) fn validate_identity(identity: &IdentityDescriptor) -> Result<(), TroxValueError> {
    if identity.identity_version != 1 {
        return Err(TroxValueError::new(
            "trox.identity-version",
            "identity version must be 1",
        ));
    }
    if let Some(meaning) = &identity.meaning {
        validate_stable_id(meaning, "meaning")?;
    }
    let mut count = 0;
    validate_pattern(&identity.pattern, 0, &mut count)
}

fn validate_pattern(
    pattern: &Pattern,
    depth: usize,
    count: &mut usize,
) -> Result<(), TroxValueError> {
    *count += 1;
    if *count > MAX_PATTERN_NODES {
        return Err(TroxValueError::new(
            "trox.pattern-limit",
            "pattern exceeds 4,096 nodes",
        ));
    }
    if depth > MAX_SELECTOR_DEPTH {
        return Err(TroxValueError::new(
            "trox.selector-depth",
            "pattern exceeds 16 nested selectors",
        ));
    }
    match pattern {
        Pattern::Text { text } => {
            placeholders(text)?;
        }
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            if branches.is_empty() || branches.len() > MAX_SELECTOR_BRANCHES {
                return Err(TroxValueError::new(
                    "trox.branch-limit",
                    "numeric selector must have 1..=256 branches",
                ));
            }
            let is_ordinal = matches!(pattern, Pattern::Ordinal { .. });
            let mut has_other = false;
            let mut previous: Option<(u8, u64)> = None;
            for branch in branches {
                let order = match &branch.key {
                    NumericBranchKey::Exact { exact } => {
                        if *exact > MAX_SAFE_INTEGER {
                            return Err(TroxValueError::new(
                                "trox.invalid-selector-number",
                                "exact branch integer exceeds 2^53 - 1",
                            ));
                        }
                        (0, *exact)
                    }
                    NumericBranchKey::Plural { plural } if !is_ordinal => (
                        1,
                        PluralCategory::CANONICAL
                            .iter()
                            .position(|category| category == plural)
                            .unwrap() as u64,
                    ),
                    NumericBranchKey::Ordinal { ordinal } if is_ordinal => (
                        1,
                        PluralCategory::CANONICAL
                            .iter()
                            .position(|category| category == ordinal)
                            .unwrap() as u64,
                    ),
                    _ => {
                        return Err(TroxValueError::new(
                            "trox.branch-kind",
                            "numeric branch key kind differs from selector",
                        ));
                    }
                };
                if previous.is_some_and(|prior| prior >= order) {
                    return Err(TroxValueError::new(
                        "trox.branch-order",
                        "numeric branches are duplicate or noncanonical",
                    ));
                }
                previous = Some(order);
                has_other |= branch.key.category() == Some(PluralCategory::Other);
                validate_pattern(&branch.pattern, depth + 1, count)?;
            }
            if !has_other {
                return Err(TroxValueError::new(
                    "trox.missing-other",
                    "plural and ordinal selectors require `other`",
                ));
            }
        }
        Pattern::Select { branches } => {
            if branches.is_empty() || branches.len() > MAX_SELECTOR_BRANCHES {
                return Err(TroxValueError::new(
                    "trox.branch-limit",
                    "select must have 1..=256 branches",
                ));
            }
            if !matches!(
                branches.last(),
                Some(SelectIdentityBranch::Otherwise { .. })
            ) {
                return Err(TroxValueError::new(
                    "trox.missing-otherwise",
                    "select requires a final `otherwise`",
                ));
            }
            if branches[..branches.len() - 1]
                .iter()
                .any(|branch| matches!(branch, SelectIdentityBranch::Otherwise { .. }))
            {
                return Err(TroxValueError::new(
                    "trox.otherwise-order",
                    "otherwise must appear exactly once at the end",
                ));
            }
            for branch in branches {
                validate_pattern(branch.pattern(), depth + 1, count)?;
            }
        }
    }
    Ok(())
}

fn collect_placeholders(
    pattern: &Pattern,
    target: &mut BTreeSet<String>,
) -> Result<(), TroxValueError> {
    match pattern {
        Pattern::Text { text } => target.extend(placeholders(text)?),
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            for branch in branches {
                collect_placeholders(&branch.pattern, target)?;
            }
        }
        Pattern::Select { branches } => {
            for branch in branches {
                collect_placeholders(branch.pattern(), target)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_arguments(
    pattern: &Pattern,
    arguments: &BTreeMap<String, Argument>,
) -> Result<(), TroxValueError> {
    if arguments.len() > MAX_ARGUMENTS {
        return Err(TroxValueError::new(
            "trox.argument-limit",
            "message exceeds 256 arguments",
        ));
    }
    let mut expected = BTreeSet::new();
    collect_placeholders(pattern, &mut expected)?;
    let actual: BTreeSet<_> = arguments.keys().cloned().collect();
    if expected != actual {
        return Err(TroxValueError::new(
            "trox.argument-mismatch",
            format!("placeholder bindings differ: expected {expected:?}, got {actual:?}"),
        ));
    }
    for argument in arguments.values() {
        match argument {
            Argument::Text { value } => validate_nfc(value, "argument text")?,
            Argument::Number { value } if !value.is_finite() => {
                return Err(TroxValueError::new(
                    "trox.invalid-number",
                    "argument number is not finite",
                ));
            }
            Argument::Term {
                term_id,
                form,
                number,
            } => {
                validate_stable_id(term_id.as_str(), "term ID")?;
                if let Some(form) = form {
                    validate_stable_id(form, "term form")?;
                }
                if number.is_some_and(|value| value > MAX_SAFE_INTEGER) {
                    return Err(TroxValueError::new(
                        "trox.invalid-selector-number",
                        "term number exceeds 2^53 - 1",
                    ));
                }
            }
            Argument::Opaque { value } => {
                if value.format != "trox-localized-string" || !value.version.is_supported_v1() {
                    return Err(TroxValueError::new(
                        "trox.invalid-opaque-wire",
                        "opaque value has an invalid format or version",
                    ));
                }
                if !matches!(value.identity.pattern, Pattern::Text { .. })
                    || !value.arguments.is_empty()
                    || !value.selectors.is_empty()
                {
                    return Err(TroxValueError::new(
                        "trox.non-atomic-opaque",
                        "opaque value must be a text leaf without arguments or selectors",
                    ));
                }
                validate_identity(&value.identity)?;
                validate_arguments(&value.identity.pattern, &value.arguments)?;
                validate_selectors(&value.identity.pattern, &value.selectors)?;
                let ids = identity_id_pair(&value.identity)?;
                if value.entry_id != ids.0 || value.source_signature != ids.1 {
                    return Err(TroxValueError::new(
                        "trox.identity-mismatch",
                        "opaque value identity hash does not match its wire IDs",
                    ));
                }
                let contract = crate::value::contract_signature(
                    &value.identity,
                    &crate::value::schemas_from_arguments(&value.arguments),
                )
                .map_err(|error| TroxValueError::new("trox.contract", error.to_string()))?;
                if value
                    .contract_signature
                    .as_deref()
                    .is_some_and(|value| value != contract)
                    || (value.version == Version::V1_1 && value.contract_signature.is_none())
                {
                    return Err(TroxValueError::new(
                        "trox.contract-mismatch",
                        "opaque value contract signature does not match its arguments",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn validate_argument_schemas(
    pattern: &Pattern,
    arguments: &BTreeMap<String, ArgumentSchema>,
) -> Result<(), TroxValueError> {
    if arguments.len() > MAX_ARGUMENTS {
        return Err(TroxValueError::new(
            "trox.argument-limit",
            "message exceeds 256 arguments",
        ));
    }
    let mut expected = BTreeSet::new();
    collect_placeholders(pattern, &mut expected)?;
    let actual: BTreeSet<_> = arguments.keys().cloned().collect();
    if expected != actual {
        return Err(TroxValueError::new(
            "trox.argument-mismatch",
            format!("placeholder declarations differ: expected {expected:?}, got {actual:?}"),
        ));
    }
    for schema in arguments.values() {
        if let ArgumentSchema::Term {
            form: Some(form), ..
        } = schema
        {
            validate_stable_id(form, "term form")?;
        }
    }
    Ok(())
}

pub(crate) fn validate_selectors(
    pattern: &Pattern,
    selectors: &[SelectorRecord],
) -> Result<(), TroxValueError> {
    let mut expected = Vec::new();
    collect_selector_paths(pattern, &mut Vec::new(), &mut expected);
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    let actual: Vec<_> = selectors
        .iter()
        .map(|record| {
            (
                record.path().to_vec(),
                match record {
                    SelectorRecord::Plural { .. } => "plural",
                    SelectorRecord::Ordinal { .. } => "ordinal",
                    SelectorRecord::Select { .. } => "select",
                },
            )
        })
        .collect();
    let expected_paths: Vec<_> = expected
        .iter()
        .map(|(path, kind, _)| (path.clone(), *kind))
        .collect();
    if expected_paths != actual {
        return Err(TroxValueError::new(
            "trox.selector-records",
            format!("selector records differ: expected {expected_paths:?}, got {actual:?}"),
        ));
    }
    for (record, (_, _, select_branches)) in selectors.iter().zip(expected) {
        match record {
            SelectorRecord::Plural { value, .. } | SelectorRecord::Ordinal { value, .. }
                if *value > MAX_SAFE_INTEGER =>
            {
                return Err(TroxValueError::new(
                    "trox.invalid-selector-number",
                    "selector integer exceeds 2^53 - 1",
                ));
            }
            SelectorRecord::Select {
                branch_keys, value, ..
            } => {
                if branch_keys.len() != select_branches {
                    return Err(TroxValueError::new(
                        "trox.selector-records",
                        "select branch key count differs from when branch count",
                    ));
                }
                value.validate()?;
                let mut unique = BTreeSet::new();
                for key in branch_keys {
                    key.validate()?;
                    if !unique.insert(key) {
                        return Err(TroxValueError::new(
                            "trox.duplicate-selector-key",
                            "select branch keys must be unique",
                        ));
                    }
                }
                if branch_keys
                    .iter()
                    .any(|key| std::mem::discriminant(key) != std::mem::discriminant(value))
                {
                    return Err(TroxValueError::new(
                        "trox.selector-key-type",
                        "select keys must have one JSON type",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_selector_paths(
    pattern: &Pattern,
    path: &mut Vec<usize>,
    target: &mut Vec<(Vec<usize>, &'static str, usize)>,
) {
    match pattern {
        Pattern::Text { .. } => {}
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            target.push((
                path.clone(),
                if matches!(pattern, Pattern::Ordinal { .. }) {
                    "ordinal"
                } else {
                    "plural"
                },
                0,
            ));
            for (index, branch) in branches.iter().enumerate() {
                path.push(index);
                collect_selector_paths(&branch.pattern, path, target);
                path.pop();
            }
        }
        Pattern::Select { branches } => {
            target.push((path.clone(), "select", branches.len().saturating_sub(1)));
            for (index, branch) in branches.iter().enumerate() {
                path.push(index);
                collect_selector_paths(branch.pattern(), path, target);
                path.pop();
            }
        }
    }
}

/// Derives the canonical short entry ID and full source signature for an identity.
pub fn identity_ids(identity: &IdentityDescriptor) -> Result<(String, String), TroxValueError> {
    validate_identity(identity)?;
    identity_id_pair(identity)
}

/// Derives the canonical short row ID for an expansion descriptor.
pub fn expansion_row_id<T: Serialize>(expansion: &T) -> Result<String, TroxValueError> {
    let bytes = signature(expansion)
        .map_err(|error| TroxValueError::new("trox.expansion", error.to_string()))?;
    Ok(short_id("row1_", &bytes))
}

/// Derives the full revision ID for a serializable bundle revision value.
pub fn revision_id<T: Serialize>(revision: &T) -> Result<String, TroxValueError> {
    Ok(format!(
        "rev1_{}",
        signature_hex(revision)
            .map_err(|error| TroxValueError::new("trox.revision", error.to_string()))?
    ))
}
