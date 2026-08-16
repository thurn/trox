//! Locale-independent message values and deterministic bundle resolution.
//!
//! Trox separates authoring a message from rendering it. Authoring functions
//! never consult global locale state: they build immutable [`LocalizedString`]
//! values containing a stable identity and typed arguments. An explicit
//! [`Localizer`] later resolves those values against validated source and target
//! [`Bundle`]s.
//!
//! Most application code can import [`prelude`] and use [`tx`] for static text
//! or [`txa`] with [`tx_args!`] for interpolation and selection:
//!
//! ```
//! use trox::prelude::*;
//!
//! let message = tx("Close deck browser", "Accessible button label.");
//! assert!(message.entry_id().starts_with("tx1_"));
//!
//! let deck_name = String::from("Night Garden");
//! let heading = txa(
//!     "Deck: {deck_name}",
//!     tx_args![deck_name],
//!     "Heading for the currently open deck.",
//! );
//! assert!(!heading.is_atomic());
//! ```
//!
//! Serialized localized values should be decoded through a [`SourceCatalog`]
//! or [`Localizer`], which checks canonical encoding and source-catalog
//! authorization. Use [`Localizer::resolve_checked`] when resolution failures
//! must remain explicit, or [`Localizer::resolve`] for source-language recovery.

#![warn(missing_docs)]

mod annotated;
mod bundle;
mod canonical;
mod catalog;
mod error;
mod model;
mod pattern;
mod ron_adapter;
mod runtime;
mod source_message;
mod value;

pub use annotated::AnnotatedLocalizedString;
pub use bundle::{
    Bundle, BundleEntry, BundleRow, BundleTerm, BundleTermForm, BundleTermSurface, Diagnostic,
    DiagnosticCode, ExpansionDescriptor, IsolationPolicy, Localizer, NumberFormat, PluralRules,
    ResolveOutcome, ResolvedLocalizedPart, ResolvedLocalizedPartsOutcome, TextDirection,
};
pub use catalog::SourceCatalog;
pub use error::{DeserializeError, ResolveError, SerializeError, TroxValueError};
pub use model::{
    Argument, ArgumentKind, ArgumentSchema, IdentityDescriptor, NumericBranch, NumericBranchKey,
    Pattern, PluralCategory, SelectIdentityBranch, SelectorKey, SelectorRecord, TermArgument,
    TermId, TroxInteger, TroxNumber, Version, expansion_row_id, identity_ids, revision_id,
};
/// Computes the v1.1 compatibility signature for an identity and argument schema.
pub fn contract_signature(
    identity: &IdentityDescriptor,
    arguments: &std::collections::BTreeMap<String, ArgumentSchema>,
) -> Result<String, TroxValueError> {
    value::contract_signature(identity, arguments)
        .map_err(|error| TroxValueError::new("trox.contract", error.to_string()))
}
pub use pattern::{
    PatternValue, SelectArm, TroxSelector, assert_localized, counted, exact, few, indefinite, many,
    meaning, one, opaque, ordinal, other, otherwise, plural, select, term, two, tx, tx_owned, txa,
    when, zero,
};
pub use ron_adapter::{RonPlaceholder, RonTx};
pub use source_message::{SourceMessage, SourceMessageRef};
pub use value::LocalizedString;

/// Common types, authoring functions, and macros for application code.
///
/// Importing `trox::prelude::*` provides the normal message-authoring surface
/// without exposing bundle wire structures that are usually only needed by
/// localization infrastructure.
pub mod prelude {
    pub use crate::{
        AnnotatedLocalizedString, LocalizedString, Localizer, ResolvedLocalizedPart, TermId,
        TroxSelector, assert_localized, counted, exact, few, indefinite, many, meaning, one,
        opaque, ordinal, other, otherwise, plural, select, term, two, tx, tx_args, txa, when, zero,
    };
}

/// Build an owned argument map while evaluating every expression exactly once.
///
/// Shorthand entries use their Rust identifier as the placeholder name. A
/// renamed entry uses `placeholder => expression`.
#[macro_export]
macro_rules! tx_args {
    () => {{
        ::std::collections::BTreeMap::<::std::string::String, $crate::Argument>::new()
    }};
    ($($name:ident $(=> $value:expr)?),+ $(,)?) => {{
        let mut args = ::std::collections::BTreeMap::<
            ::std::string::String,
            $crate::Argument,
        >::new();
        $(
            $crate::tx_args!(@insert args, $name $(=> $value)?);
        )+
        args
    }};
    (@insert $args:ident, $name:ident) => {{
        let value = &$name;
        let argument = $crate::Argument::from_borrowed(value);
        if $args.insert(::std::string::String::from(stringify!($name)), argument).is_some() {
            panic!("TROX_ASSERT duplicate argument `{}`", stringify!($name));
        }
    }};
    (@insert $args:ident, $name:ident => $value:expr) => {{
        let evaluated_once = $value;
        let argument = $crate::Argument::from_borrowed(&evaluated_once);
        if $args.insert(::std::string::String::from(stringify!($name)), argument).is_some() {
            panic!("TROX_ASSERT duplicate argument `{}`", stringify!($name));
        }
    }};
}
