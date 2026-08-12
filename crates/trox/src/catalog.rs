use std::collections::BTreeMap;

use serde_json::Value;

use crate::bundle::{Bundle, BundleTermForm};
use crate::canonical::canonical_json;
use crate::model::{Argument, ArgumentSchema, IdentityDescriptor, identity_ids};
use crate::value::LocalizedStringWire;
use crate::{DeserializeError, LocalizedString};

/// The validated message identities and term schemas authorized by a source bundle.
///
/// A catalog is the security boundary for decoding serialized localized values: a
/// self-consistent hash is not sufficient unless its identity and argument schemas
/// are also present here.
#[derive(Debug, Clone)]
pub struct SourceCatalog {
    fingerprint: String,
    entries: BTreeMap<String, CatalogEntry>,
    terms: BTreeMap<String, BTreeMap<String, TermNumberPolicy>>,
}

#[derive(Debug, Clone)]
struct CatalogEntry {
    arguments: BTreeMap<String, ArgumentSchema>,
    identity: IdentityDescriptor,
    source_signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermNumberPolicy {
    Forbidden,
    Required,
}

impl SourceCatalog {
    /// Returns the full source-catalog fingerprint recorded by the source bundle.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn from_validated_bundle(bundle: &Bundle) -> Result<Self, DeserializeError> {
        let mut entries = BTreeMap::new();
        for (id, entry) in &bundle.entries {
            let identity = entry.identity.clone().ok_or_else(|| {
                DeserializeError::InvalidBundle(format!("source entry `{id}` lacks identity"))
            })?;
            let arguments = entry.arguments.clone().ok_or_else(|| {
                DeserializeError::InvalidBundle(format!("source entry `{id}` lacks arguments"))
            })?;
            entries.insert(
                id.clone(),
                CatalogEntry {
                    arguments,
                    identity,
                    source_signature: entry.source_signature.clone(),
                },
            );
        }
        let terms = bundle
            .terms
            .iter()
            .map(|(term_id, term)| {
                let forms = term
                    .forms
                    .iter()
                    .map(|(form_id, form)| {
                        let policy = match form {
                            BundleTermForm::Scalar { .. } => TermNumberPolicy::Forbidden,
                            BundleTermForm::Number { .. } => TermNumberPolicy::Required,
                        };
                        (form_id.clone(), policy)
                    })
                    .collect();
                (term_id.clone(), forms)
            })
            .collect();
        Ok(Self {
            fingerprint: bundle.source_catalog_fingerprint.clone(),
            entries,
            terms,
        })
    }

    /// Decodes canonical JSON after authorizing its identity and term schemas.
    ///
    /// Unknown messages, forms, and incompatible numbered-term contracts are
    /// rejected before a [`LocalizedString`] is returned. The reserved,
    /// catalog-independent wire shape produced by [`crate::localization_todo`]
    /// is the sole exception.
    pub fn localized_string_from_json(
        &self,
        input: &str,
    ) -> Result<LocalizedString, DeserializeError> {
        let value: Value = serde_json::from_str(input)?;
        let canonical = canonical_json(&value)
            .map_err(|error| DeserializeError::InvalidValue(error.to_string()))?;
        if canonical != input {
            return Err(DeserializeError::NoncanonicalJson);
        }
        let wire: LocalizedStringWire = serde_json::from_value(value)?;
        if wire.format != "trox-localized-string"
            || wire.version.major != 1
            || wire.version.minor != 0
        {
            return Err(DeserializeError::UnsupportedVersion {
                format: "localized string",
                major: wire.version.major,
                minor: wire.version.minor,
            });
        }
        let (entry_id, signature) = identity_ids(&wire.identity)
            .map_err(|error| DeserializeError::InvalidValue(error.to_string()))?;
        if entry_id != wire.entry_id || signature != wire.source_signature {
            return Err(DeserializeError::InvalidValue(
                "identity hash does not match wire IDs".into(),
            ));
        }
        if wire.identity.meaning.as_deref() == Some(crate::pattern::LOCALIZATION_TODO_MEANING) {
            let value = LocalizedString::build_with_known_ids(
                wire.identity,
                wire.arguments,
                wire.selectors,
                wire.entry_id,
                wire.source_signature,
            )
            .map_err(|error| DeserializeError::InvalidValue(error.to_string()))?;
            if value.is_localization_todo() {
                return Ok(value);
            }
            return Err(DeserializeError::Unauthorized(
                "invalid localization TODO value".into(),
            ));
        }
        let Some(authorized) = self.entries.get(&entry_id) else {
            return Err(DeserializeError::Unauthorized(entry_id));
        };
        if authorized.source_signature != signature || authorized.identity != wire.identity {
            return Err(DeserializeError::Unauthorized(format!(
                "incompatible identity for `{entry_id}`"
            )));
        }
        authorize_argument_schemas(&authorized.arguments, &wire.arguments)?;
        if wire
            .selectors
            .windows(2)
            .any(|pair| pair[0].path() >= pair[1].path())
        {
            return Err(DeserializeError::InvalidValue(
                "selector records are duplicate or noncanonical".into(),
            ));
        }
        for argument in wire.arguments.values() {
            match argument {
                Argument::Term {
                    term_id,
                    form,
                    number,
                } => {
                    let forms = self.terms.get(term_id.as_str()).ok_or_else(|| {
                        DeserializeError::Unauthorized(format!(
                            "unknown term `{}`",
                            term_id.as_str()
                        ))
                    })?;
                    let form_id = form.as_deref().unwrap_or("$default");
                    let policy = forms.get(form_id).ok_or_else(|| {
                        DeserializeError::Unauthorized(format!(
                            "unknown term form `{}.{form_id}`",
                            term_id.as_str()
                        ))
                    })?;
                    let contract_matches = matches!(
                        (policy, number),
                        (TermNumberPolicy::Forbidden, None) | (TermNumberPolicy::Required, Some(_))
                    );
                    if !contract_matches {
                        return Err(DeserializeError::Unauthorized(format!(
                            "term form number contract mismatch for `{}.{form_id}`",
                            term_id.as_str()
                        )));
                    }
                }
                Argument::Opaque { value } => {
                    let nested = canonical_json(value)
                        .map_err(|error| DeserializeError::InvalidValue(error.to_string()))?;
                    self.localized_string_from_json(&nested)?;
                }
                _ => {}
            }
        }
        LocalizedString::build_with_known_ids(
            wire.identity,
            wire.arguments,
            wire.selectors,
            wire.entry_id,
            wire.source_signature,
        )
        .map_err(|error| DeserializeError::InvalidValue(error.to_string()))
    }
}

fn authorize_argument_schemas(
    schemas: &BTreeMap<String, ArgumentSchema>,
    arguments: &BTreeMap<String, Argument>,
) -> Result<(), DeserializeError> {
    if schemas.len() != arguments.len() {
        return Err(DeserializeError::Unauthorized(
            "argument schemas differ from the source entry".into(),
        ));
    }
    for (name, schema) in schemas {
        let Some(argument) = arguments.get(name) else {
            return Err(DeserializeError::Unauthorized(format!(
                "argument `{name}` is absent from the source entry schema"
            )));
        };
        let compatible = match (schema, argument) {
            (
                ArgumentSchema::Scalar,
                Argument::Text { .. } | Argument::Number { .. } | Argument::Boolean { .. },
            )
            | (ArgumentSchema::Opaque, Argument::Opaque { .. }) => true,
            (
                ArgumentSchema::Term {
                    form: expected_form,
                    number: expected_number,
                },
                Argument::Term { form, number, .. },
            ) => expected_form == form && *expected_number == number.is_some(),
            _ => false,
        };
        if !compatible {
            return Err(DeserializeError::Unauthorized(format!(
                "argument `{name}` does not match its source entry schema"
            )));
        }
    }
    Ok(())
}
