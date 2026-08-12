use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::UnicodeNormalization;

use crate::model::{
    IdentityDescriptor, IntoArgument, NumericBranch, NumericBranchKey, Pattern, PluralCategory,
    SelectIdentityBranch, SelectorKey, SelectorRecord, TermArgument, TermId, TroxInteger,
    validate_stable_id,
};
use crate::value::LocalizedStringWire;
use crate::{Argument, LocalizedString};

pub(crate) const ASSERT_LOCALIZED_MEANING: &str = "trox.assert-localized";

/// A fully owned pattern under construction.
#[derive(Debug, Clone)]
pub struct PatternValue {
    pub(crate) pattern: Pattern,
    pub(crate) selectors: Vec<SelectorRecord>,
    pub(crate) meaning: Option<String>,
}

impl PatternValue {
    fn text(value: impl Into<String>) -> Self {
        Self {
            pattern: Pattern::Text { text: value.into() },
            selectors: vec![],
            meaning: None,
        }
    }
}

impl From<&'static str> for PatternValue {
    fn from(value: &'static str) -> Self {
        Self::text(value)
    }
}

/// One source-authored branch of a cardinal or ordinal selector.
#[derive(Debug, Clone)]
pub struct NumericArm {
    key: NumericArmKey,
    value: PatternValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumericArmKey {
    Exact(u64),
    Category(PluralCategory),
}

/// Conversion into a non-negative safe integer for selector and term counts.
pub trait IntoTroxInteger {
    /// Converts the value into a validated Trox integer.
    fn into_trox_integer(self) -> TroxInteger;
}
impl IntoTroxInteger for TroxInteger {
    fn into_trox_integer(self) -> TroxInteger {
        self
    }
}
impl IntoTroxInteger for u8 {
    fn into_trox_integer(self) -> TroxInteger {
        self.into()
    }
}
impl IntoTroxInteger for u16 {
    fn into_trox_integer(self) -> TroxInteger {
        self.into()
    }
}
impl IntoTroxInteger for u32 {
    fn into_trox_integer(self) -> TroxInteger {
        self.into()
    }
}

/// Creates an exact-value branch for a numeric selector.
pub fn exact<N: IntoTroxInteger, P: Into<PatternValue>>(value: N, pattern: P) -> NumericArm {
    let pattern = pattern.into();
    assert_no_nested_meaning(&pattern);
    NumericArm {
        key: NumericArmKey::Exact(value.into_trox_integer().get()),
        value: pattern,
    }
}

macro_rules! category_arm {
    ($name:ident, $category:ident) => {
        #[doc = concat!("Creates a `", stringify!($name), "` category branch for a numeric selector.")]
        pub fn $name<P: Into<PatternValue>>(pattern: P) -> NumericArm {
            let value = pattern.into();
            assert_no_nested_meaning(&value);
            NumericArm {
                key: NumericArmKey::Category(PluralCategory::$category),
                value,
            }
        }
    };
}
category_arm!(zero, Zero);
category_arm!(one, One);
category_arm!(two, Two);
category_arm!(few, Few);
category_arm!(many, Many);
category_arm!(other, Other);

/// Creates a cardinal-plural pattern selected by `value`.
///
/// Branches must be in canonical order and include [`other`].
pub fn plural<N: IntoTroxInteger, const SIZE: usize>(
    value: N,
    branches: [NumericArm; SIZE],
) -> PatternValue {
    numeric_pattern(value.into_trox_integer().get(), branches, false)
}

/// Creates an ordinal-plural pattern selected by `value`.
///
/// Branches must be in canonical order and include [`other`].
pub fn ordinal<N: IntoTroxInteger, const SIZE: usize>(
    value: N,
    branches: [NumericArm; SIZE],
) -> PatternValue {
    numeric_pattern(value.into_trox_integer().get(), branches, true)
}

fn numeric_pattern<const SIZE: usize>(
    value: u64,
    branches: [NumericArm; SIZE],
    ordinal: bool,
) -> PatternValue {
    if branches.is_empty() {
        panic!("TROX_ASSERT numeric selector has no branches");
    }
    let mut previous: Option<(u8, u64)> = None;
    let mut seen = BTreeSet::new();
    let mut has_other = false;
    let mut identity_branches = Vec::with_capacity(SIZE);
    let mut selectors = Vec::new();
    for (index, arm) in branches.into_iter().enumerate() {
        let order = match arm.key {
            NumericArmKey::Exact(exact) => (0, exact),
            NumericArmKey::Category(category) => {
                let ordinal = PluralCategory::CANONICAL
                    .iter()
                    .position(|candidate| *candidate == category)
                    .unwrap() as u64;
                (1, ordinal)
            }
        };
        if previous.is_some_and(|prior| prior >= order) || !seen.insert(order) {
            panic!("TROX_ASSERT numeric selector branches are duplicate or noncanonical");
        }
        previous = Some(order);
        has_other |= arm.key == NumericArmKey::Category(PluralCategory::Other);
        selectors.extend(prefix_selectors(arm.value.selectors, index));
        let key = match arm.key {
            NumericArmKey::Exact(exact) => NumericBranchKey::Exact { exact },
            NumericArmKey::Category(category) if ordinal => {
                NumericBranchKey::Ordinal { ordinal: category }
            }
            NumericArmKey::Category(category) => NumericBranchKey::Plural { plural: category },
        };
        identity_branches.push(NumericBranch {
            key,
            pattern: arm.value.pattern,
        });
    }
    if !has_other {
        panic!("TROX_ASSERT numeric selector requires `other`");
    }
    selectors.push(if ordinal {
        SelectorRecord::Ordinal {
            path: vec![],
            value,
        }
    } else {
        SelectorRecord::Plural {
            path: vec![],
            value,
        }
    });
    PatternValue {
        pattern: if ordinal {
            Pattern::Ordinal {
                branches: identity_branches,
            }
        } else {
            Pattern::Plural {
                branches: identity_branches,
            }
        },
        selectors,
        meaning: None,
    }
}

/// Stable semantic selector contract. Applications explicitly map variants to
/// stable keys. Boolean implements this trait with JSON boolean keys.
pub trait TroxSelector {
    /// Returns the stable source-level key for this selector value.
    fn trox_key(&self) -> &'static str;
    #[doc(hidden)]
    fn trox_boolean_key(&self) -> Option<bool> {
        None
    }
}

impl TroxSelector for bool {
    fn trox_key(&self) -> &'static str {
        if *self { "true" } else { "false" }
    }
    fn trox_boolean_key(&self) -> Option<bool> {
        Some(*self)
    }
}

fn selector_key<T: TroxSelector>(value: &T) -> SelectorKey {
    let key = if let Some(value) = value.trox_boolean_key() {
        SelectorKey::Boolean(value)
    } else {
        SelectorKey::String(value.trox_key().to_owned())
    };
    if let Err(error) = key.validate() {
        panic!("TROX_ASSERT {error}");
    }
    key
}

/// One source-authored branch of a semantic selector.
#[derive(Debug, Clone)]
pub struct SelectArm {
    key: Option<SelectorKey>,
    value: PatternValue,
}

/// Creates a conditional branch for a semantic selector.
pub fn when<T: TroxSelector, P: Into<PatternValue>>(key: T, pattern: P) -> SelectArm {
    let value = pattern.into();
    assert_no_nested_meaning(&value);
    SelectArm {
        key: Some(selector_key(&key)),
        value,
    }
}

/// Creates the required final fallback branch for a semantic selector.
pub fn otherwise<P: Into<PatternValue>>(pattern: P) -> SelectArm {
    let value = pattern.into();
    assert_no_nested_meaning(&value);
    SelectArm { key: None, value }
}

/// Creates a semantic selector pattern selected by a stable application key.
pub fn select<T: TroxSelector, const SIZE: usize>(
    value: T,
    branches: [SelectArm; SIZE],
) -> PatternValue {
    if branches.is_empty() {
        panic!("TROX_ASSERT select has no branches");
    }
    if branches.last().is_some_and(|branch| branch.key.is_some()) {
        panic!("TROX_ASSERT select requires final otherwise");
    }
    if branches[..branches.len() - 1]
        .iter()
        .any(|branch| branch.key.is_none())
    {
        panic!("TROX_ASSERT otherwise must appear exactly once at the end");
    }
    let value_key = selector_key(&value);
    let mut keys = BTreeSet::new();
    let mut branch_keys = Vec::with_capacity(SIZE.saturating_sub(1));
    let mut identity_branches = Vec::with_capacity(SIZE);
    let mut selectors = Vec::new();
    for (index, arm) in branches.into_iter().enumerate() {
        selectors.extend(prefix_selectors(arm.value.selectors, index));
        match arm.key {
            Some(key) => {
                if std::mem::discriminant(&key) != std::mem::discriminant(&value_key) {
                    panic!("TROX_ASSERT select branch key type differs from selector value");
                }
                if !keys.insert(key.clone()) {
                    panic!("TROX_ASSERT duplicate select branch key");
                }
                branch_keys.push(key);
                identity_branches.push(SelectIdentityBranch::When {
                    pattern: arm.value.pattern,
                });
            }
            None => identity_branches.push(SelectIdentityBranch::Otherwise {
                pattern: arm.value.pattern,
            }),
        }
    }
    selectors.push(SelectorRecord::Select {
        path: vec![],
        branch_keys,
        value: value_key,
    });
    PatternValue {
        pattern: Pattern::Select {
            branches: identity_branches,
        },
        selectors,
        meaning: None,
    }
}

fn prefix_selectors(mut selectors: Vec<SelectorRecord>, branch: usize) -> Vec<SelectorRecord> {
    for selector in &mut selectors {
        match selector {
            SelectorRecord::Plural { path, .. }
            | SelectorRecord::Ordinal { path, .. }
            | SelectorRecord::Select { path, .. } => path.insert(0, branch),
        }
    }
    selectors
}

fn assert_no_nested_meaning(pattern: &PatternValue) {
    if pattern.meaning.is_some() {
        panic!("TROX_ASSERT meaning may only wrap a complete top-level pattern");
    }
}

/// Assigns a stable semantic meaning to a complete message pattern.
///
/// A meaning changes message identity and may not be nested inside a selector arm.
pub fn meaning<P: Into<PatternValue>>(meaning: &'static str, pattern: P) -> PatternValue {
    if let Err(error) = validate_stable_id(meaning, "meaning") {
        panic!("TROX_ASSERT {error}");
    }
    let mut pattern = pattern.into();
    if pattern.meaning.replace(meaning.to_owned()).is_some() {
        panic!("TROX_ASSERT duplicate meaning wrapper");
    }
    pattern
}

fn validate_description(description: &str) {
    if description.trim().is_empty() {
        panic!("TROX_ASSERT description must not be empty");
    }
    if description.nfc().collect::<String>() != description {
        panic!("TROX_ASSERT description must be NFC");
    }
}

/// Creates a source-authored localized value without placeholder arguments.
///
/// Source patterns accept string literals, not owned or computed strings. Data
/// adapters that must validate owned text should use [`tx_owned`].
///
/// ```compile_fail
/// use trox::tx;
///
/// let computed = String::from("computed at runtime");
/// let _ = tx(computed, "Description for translators.");
/// ```
pub fn tx<P: Into<PatternValue>>(pattern: P, description: &'static str) -> LocalizedString {
    validate_description(description);
    build(pattern.into(), BTreeMap::new())
}

/// Creates a source-authored localized value with named placeholder arguments.
///
/// The pattern's placeholder set must exactly match `arguments`.
pub fn txa<P: Into<PatternValue>>(
    pattern: P,
    arguments: BTreeMap<String, Argument>,
    description: &'static str,
) -> LocalizedString {
    validate_description(description);
    build(pattern.into(), arguments)
}

/// Creates an atomic localized value from adapter-owned text after validation.
///
/// This is the deliberate owned-string boundary and does not accept dynamic
/// selector patterns or placeholder arguments.
pub fn tx_owned(
    text: String,
    meaning: Option<String>,
) -> Result<LocalizedString, crate::TroxValueError> {
    let pattern = PatternValue {
        pattern: Pattern::Text { text },
        selectors: vec![],
        meaning,
    };
    LocalizedString::build(
        IdentityDescriptor {
            identity_version: 1,
            meaning: pattern.meaning,
            pattern: pattern.pattern,
        },
        BTreeMap::new(),
        pattern.selectors,
    )
}

/// Asserts that runtime text is appropriate to display without translation.
///
/// Use this explicit escape hatch for migrations, raw user input, tests, and
/// developer-only surfaces. The returned value always resolves to `raw_string`
/// in every locale, and calls are deliberately ignored by source extraction.
pub fn assert_localized(raw_string: impl AsRef<str>) -> LocalizedString {
    let normalized: String = raw_string.as_ref().nfc().collect();
    let escaped = normalized.replace('{', "{{").replace('}', "}}");
    LocalizedString::build(
        IdentityDescriptor {
            identity_version: 1,
            meaning: Some(ASSERT_LOCALIZED_MEANING.to_owned()),
            pattern: Pattern::Text { text: escaped },
        },
        BTreeMap::new(),
        vec![],
    )
    .expect("normalized asserted-localized text is always a valid atomic pattern")
}

fn build(pattern: PatternValue, arguments: BTreeMap<String, Argument>) -> LocalizedString {
    LocalizedString::build(
        IdentityDescriptor {
            identity_version: 1,
            meaning: pattern.meaning,
            pattern: pattern.pattern,
        },
        arguments,
        pattern.selectors,
    )
    .unwrap_or_else(|error| panic!("TROX_ASSERT {error}"))
}

/// Builder for a bundle-authorized term argument.
#[derive(Debug, Clone)]
pub struct TermBuilder(TermArgument);

/// Starts building an argument that references `term_id`.
pub fn term(term_id: TermId) -> TermBuilder {
    TermBuilder(TermArgument {
        term_id,
        form: None,
        number: None,
    })
}

impl TermBuilder {
    /// Requests a stable form of the referenced term.
    ///
    /// # Panics
    ///
    /// Panics if the form is invalid or a form has already been specified.
    pub fn form(mut self, form: impl Into<String>) -> Self {
        let form = form.into();
        if let Err(error) = validate_stable_id(&form, "term form") {
            panic!("TROX_ASSERT {error}");
        }
        if self.0.form.replace(form).is_some() {
            panic!("TROX_ASSERT duplicate term form");
        }
        self
    }

    /// Supplies the count for a numbered term form and finishes the argument.
    pub fn number<N: IntoTroxInteger>(mut self, number: N) -> TermArgument {
        self.0.number = Some(number.into_trox_integer().get());
        self.0
    }

    /// Finishes the term argument without a count.
    pub fn finish(self) -> TermArgument {
        self.0
    }
}

impl IntoArgument for TermBuilder {
    fn to_argument(&self) -> Argument {
        self.0.to_argument()
    }
}

/// Creates an `indefinite` scalar-form term argument.
pub fn indefinite(term_id: TermId) -> TermArgument {
    term(term_id).form("indefinite").finish()
}
/// Creates a `counted` numbered-form term argument.
pub fn counted<N: IntoTroxInteger>(term_id: TermId, number: N) -> TermArgument {
    term(term_id).form("counted").number(number)
}

/// Atomic localized value prepared for opaque placeholder insertion.
#[derive(Debug, Clone)]
pub struct OpaqueArgument(LocalizedStringWire);

/// Wraps an atomic localized value as an opaque placeholder argument.
///
/// # Panics
///
/// Panics if `value` contains selectors or placeholder arguments.
pub fn opaque(value: LocalizedString) -> OpaqueArgument {
    if !value.is_atomic() {
        panic!("TROX_ASSERT opaque value must be atomic");
    }
    OpaqueArgument(value.into_wire())
}

impl IntoArgument for OpaqueArgument {
    fn to_argument(&self) -> Argument {
        Argument::Opaque {
            value: Box::new(self.0.clone()),
        }
    }
}
