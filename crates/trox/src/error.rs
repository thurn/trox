use thiserror::Error;

/// A violation of the invariants for a Trox value.
///
/// This error is returned while constructing or validating locale-independent
/// values, before a bundle is involved. The stable [`Self::code`] is suitable
/// for classifying the failure; [`Self::message`] supplies human-readable
/// context for logs and diagnostics.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct TroxValueError {
    /// A stable, machine-readable identifier for the violated invariant.
    pub code: &'static str,
    /// Human-readable details about the invalid value.
    pub message: String,
}

impl TroxValueError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// An error produced while encoding a Trox value or bundle.
#[derive(Debug, Error)]
pub enum SerializeError {
    /// Canonical JSON encoding failed.
    #[error("failed to encode canonical Trox JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// An error produced while decoding or validating Trox wire data.
///
/// Deserialization includes semantic validation. Successfully parsed JSON can
/// therefore still be rejected when it is noncanonical, unsupported, invalid,
/// or not authorized by the source catalog.
#[derive(Debug, Error)]
pub enum DeserializeError {
    /// The input is not valid JSON for the requested wire type.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The input is valid JSON but is not encoded in the required canonical
    /// representation.
    #[error("noncanonical JSON encoding")]
    NoncanonicalJson,
    /// The wire object uses a format version this crate does not support.
    #[error("unsupported {format} version {major}.{minor}")]
    UnsupportedVersion {
        /// The name of the wire format whose version was rejected.
        format: &'static str,
        /// The unsupported major version.
        major: u32,
        /// The unsupported minor version.
        minor: u32,
    },
    /// A localized value violates its structural or identity invariants.
    #[error("invalid localized value: {0}")]
    InvalidValue(String),
    /// A localized value refers to content not authorized by the source
    /// catalog.
    #[error("localized value is not authorized by the source catalog: {0}")]
    Unauthorized(String),
    /// A bundle violates the Trox bundle contract.
    #[error("invalid bundle: {0}")]
    InvalidBundle(String),
}

/// An error produced by strict localization resolution.
///
/// [`crate::Localizer::resolve_checked`] returns these errors directly.
/// Applications that prefer source-language recovery can use
/// [`crate::Localizer::resolve`] or [`crate::Localizer::resolve_outcome`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// The target bundle has no compatible entry for the localized value.
    #[error("message `{entry_id}` is unavailable or incompatible")]
    MissingMessage {
        /// The stable message entry identifier that could not be resolved.
        entry_id: String,
    },
    /// The target entry has no translation row for the selected expansion.
    #[error("translation row `{row_id}` is unavailable")]
    MissingRow {
        /// The selected translation row identifier.
        row_id: String,
    },
    /// The selected pattern references an argument absent from the value.
    #[error("argument `{name}` is missing")]
    MissingArgument {
        /// The placeholder name of the absent argument.
        name: String,
    },
    /// The selected pattern references a term absent from the bundle.
    #[error("term `{term_id}` is unknown")]
    UnknownTerm {
        /// The stable identifier of the absent term.
        term_id: String,
    },
    /// A term exists but does not provide the requested grammatical form.
    #[error("term `{term_id}` has no requested form `{form}`")]
    MissingTermForm {
        /// The stable identifier of the term.
        term_id: String,
        /// The requested grammatical form.
        form: String,
    },
    /// A translated pattern cannot be interpreted according to the validated
    /// message identity.
    #[error("translated pattern is malformed: {0}")]
    MalformedTranslation(String),
}
