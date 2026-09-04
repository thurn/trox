use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::canonical::{canonical_json, short_id, signature, signature_hex};
use crate::model::{
    Argument, ArgumentSchema, IdentityDescriptor, IntoArgument, Pattern, SelectorRecord, Version,
    validate_argument_schemas, validate_arguments, validate_identity, validate_selectors,
};
use crate::pattern::LS_MEANING;
use crate::{SerializeError, SourceMessageRef, TroxValueError};

/// Canonical serialized representation of a [`LocalizedString`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalizedStringWire {
    /// Arguments keyed by placeholder name.
    pub arguments: BTreeMap<String, Argument>,
    /// Signature of the identity and placeholder contract (wire 1.1+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_signature: Option<String>,
    /// Short stable ID derived from `identity`.
    pub entry_id: String,
    /// Wire format discriminator.
    pub format: String,
    /// Locale-independent identity descriptor.
    pub identity: IdentityDescriptor,
    /// Runtime selector values ordered by selector path.
    pub selectors: Vec<SelectorRecord>,
    /// Full hexadecimal signature of `identity`.
    pub source_signature: String,
    /// Wire format version.
    pub version: Version,
}

/// Immutable source-authored localized value awaiting explicit resolution.
#[derive(Clone)]
pub struct LocalizedString {
    data: Arc<LocalizedStringData>,
}

#[derive(Debug, Clone)]
struct LocalizedStringData {
    arguments: BTreeMap<String, Argument>,
    ron_template_arguments: Option<BTreeMap<String, ArgumentSchema>>,
    identity: IdentityDescriptor,
    selectors: Vec<SelectorRecord>,
    identity_ids: OnceLock<IdentityIds>,
}

#[derive(Debug, Clone)]
struct IdentityIds {
    entry_id: String,
    source_signature: String,
}

#[derive(Serialize)]
struct ContractDescriptor<'a> {
    arguments: &'a BTreeMap<String, ArgumentSchema>,
    identity: &'a IdentityDescriptor,
}

#[derive(Serialize)]
struct LocalizedStringWireRef<'a> {
    arguments: &'a BTreeMap<String, Argument>,
    contract_signature: String,
    entry_id: &'a str,
    format: &'static str,
    identity: &'a IdentityDescriptor,
    selectors: &'a [SelectorRecord],
    source_signature: &'a str,
    version: Version,
}

impl LocalizedStringData {
    fn ids(&self) -> &IdentityIds {
        self.identity_ids.get_or_init(|| {
            compute_identity_ids(&self.identity)
                .expect("validated Trox identities always have a canonical encoding")
        })
    }
}

impl fmt::Debug for LocalizedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalizedString")
            .field("arguments", &self.data.arguments)
            .field("ron_template_arguments", &self.data.ron_template_arguments)
            .field("identity", &self.data.identity)
            .field("selectors", &self.data.selectors)
            .finish()
    }
}

impl PartialEq for LocalizedString {
    fn eq(&self, other: &Self) -> bool {
        self.data.arguments == other.data.arguments
            && self.data.ron_template_arguments == other.data.ron_template_arguments
            && self.data.identity == other.data.identity
            && self.data.selectors == other.data.selectors
    }
}

impl LocalizedString {
    /// Produces an argument-free reference to this static value or unbound RON template.
    pub fn source_message_ref(&self) -> Result<SourceMessageRef, TroxValueError> {
        if !self.data.arguments.is_empty() || !self.data.selectors.is_empty() {
            return Err(TroxValueError::new(
                "trox.bound-source-message",
                "source message references cannot retain runtime arguments or selectors",
            ));
        }
        let arguments = self.data.ron_template_arguments.clone().unwrap_or_default();
        let ids = self.data.ids();
        Ok(SourceMessageRef {
            contract_signature: contract_signature(&self.data.identity, &arguments)
                .map_err(|error| TroxValueError::new("trox.contract", error.to_string()))?,
            entry_id: ids.entry_id.clone(),
            format: "trox-source-message-ref".into(),
            source_signature: ids.source_signature.clone(),
            version: Version::V1,
        })
    }

    pub(crate) fn build(
        identity: IdentityDescriptor,
        arguments: BTreeMap<String, Argument>,
        selectors: Vec<SelectorRecord>,
    ) -> Result<Self, TroxValueError> {
        Self::build_with_ids(identity, arguments, selectors, None)
    }

    pub(crate) fn build_with_known_ids(
        identity: IdentityDescriptor,
        arguments: BTreeMap<String, Argument>,
        selectors: Vec<SelectorRecord>,
        entry_id: String,
        source_signature: String,
    ) -> Result<Self, TroxValueError> {
        Self::build_with_ids(
            identity,
            arguments,
            selectors,
            Some(IdentityIds {
                entry_id,
                source_signature,
            }),
        )
    }

    fn build_with_ids(
        identity: IdentityDescriptor,
        arguments: BTreeMap<String, Argument>,
        mut selectors: Vec<SelectorRecord>,
        known_ids: Option<IdentityIds>,
    ) -> Result<Self, TroxValueError> {
        validate_identity(&identity)?;
        validate_arguments(&identity.pattern, &arguments)?;
        selectors.sort_by(|left, right| left.path().cmp(right.path()));
        validate_selectors(&identity.pattern, &selectors)?;
        let identity_ids = OnceLock::new();
        if let Some(ids) = known_ids {
            identity_ids
                .set(ids)
                .expect("new identity cache is always empty");
        }
        Ok(Self {
            data: Arc::new(LocalizedStringData {
                arguments,
                ron_template_arguments: None,
                identity,
                selectors,
                identity_ids,
            }),
        })
    }

    pub(crate) fn from_validated_wire(wire: LocalizedStringWire) -> Self {
        let identity_ids = OnceLock::new();
        identity_ids
            .set(IdentityIds {
                entry_id: wire.entry_id,
                source_signature: wire.source_signature,
            })
            .expect("new identity cache is always empty");
        Self {
            data: Arc::new(LocalizedStringData {
                arguments: wire.arguments,
                ron_template_arguments: None,
                identity: wire.identity,
                selectors: wire.selectors,
                identity_ids,
            }),
        }
    }

    /// Returns the short stable message ID.
    pub fn entry_id(&self) -> &str {
        &self.data.ids().entry_id
    }

    /// Returns the full source identity signature as lowercase hexadecimal.
    pub fn source_signature(&self) -> &str {
        &self.data.ids().source_signature
    }

    /// Returns the signature of this value's source identity and argument contract.
    pub fn contract_signature(&self) -> String {
        contract_signature(
            &self.data.identity,
            &schemas_from_arguments(&self.data.arguments),
        )
        .expect("validated Trox contracts always have a canonical encoding")
    }

    /// Returns the locale-independent identity descriptor.
    pub fn identity(&self) -> &IdentityDescriptor {
        &self.data.identity
    }

    /// Returns the placeholder arguments owned by this value.
    pub fn arguments(&self) -> &BTreeMap<String, Argument> {
        &self.data.arguments
    }

    /// Returns the declared schemas when this value is an unbound RON template.
    ///
    /// Ordinary runtime values return `None`. RON templates must be bound with
    /// [`Self::bind_ron_template`] before canonical serialization or resolution.
    pub fn ron_template_arguments(&self) -> Option<&BTreeMap<String, ArgumentSchema>> {
        self.data.ron_template_arguments.as_ref()
    }

    /// Binds runtime arguments to a placeholder-bearing RON template.
    ///
    /// The argument names and kinds must exactly match the declarations in the
    /// source `Tx` value.
    pub fn bind_ron_template(
        &self,
        arguments: BTreeMap<String, Argument>,
    ) -> Result<Self, TroxValueError> {
        let Some(schemas) = self.data.ron_template_arguments.as_ref() else {
            return Err(TroxValueError::new(
                "trox.not-ron-template",
                "localized value is not an unbound RON template",
            ));
        };
        validate_bound_schemas(schemas, &arguments)?;
        Self::build(self.data.identity.clone(), arguments, vec![])
    }

    /// Returns runtime selector values in canonical path order.
    pub fn selectors(&self) -> &[SelectorRecord] {
        &self.data.selectors
    }

    /// Returns whether this value is a plain text leaf without arguments or selectors.
    pub fn is_atomic(&self) -> bool {
        matches!(self.data.identity.pattern, Pattern::Text { .. })
            && self.data.arguments.is_empty()
            && self.data.ron_template_arguments.is_none()
            && self.data.selectors.is_empty()
    }

    pub(crate) fn is_asserted_localized(&self) -> bool {
        self.data.identity.meaning.as_deref() == Some(LS_MEANING)
            && matches!(self.data.identity.pattern, Pattern::Text { .. })
            && self.data.arguments.is_empty()
            && self.data.ron_template_arguments.is_none()
            && self.data.selectors.is_empty()
    }

    pub(crate) fn asserted_localized_pattern(&self) -> Option<&str> {
        if !self.is_asserted_localized() {
            return None;
        }
        match &self.data.identity.pattern {
            Pattern::Text { text } => Some(text),
            _ => unreachable!("asserted-localized values always contain text patterns"),
        }
    }

    /// Serializes this value as RFC 8785 canonical JSON.
    pub fn to_canonical_json(&self) -> Result<String, SerializeError> {
        if self.data.ron_template_arguments.is_some() {
            return Err(SerializeError::UnboundRonTemplate);
        }
        let ids = self.data.ids();
        canonical_json(&LocalizedStringWireRef {
            arguments: &self.data.arguments,
            contract_signature: self.contract_signature(),
            entry_id: &ids.entry_id,
            format: "trox-localized-string",
            identity: &self.data.identity,
            selectors: &self.data.selectors,
            source_signature: &ids.source_signature,
            version: Version::V1_1,
        })
    }

    pub(crate) fn into_wire(self) -> LocalizedStringWire {
        let data = Arc::unwrap_or_clone(self.data);
        let ids = data.identity_ids.into_inner().unwrap_or_else(|| {
            compute_identity_ids(&data.identity)
                .expect("validated Trox identities always have a canonical encoding")
        });
        let contract_signature =
            contract_signature(&data.identity, &schemas_from_arguments(&data.arguments))
                .expect("validated Trox contracts always have a canonical encoding");
        LocalizedStringWire {
            arguments: data.arguments,
            contract_signature: Some(contract_signature),
            entry_id: ids.entry_id,
            format: "trox-localized-string".to_owned(),
            identity: data.identity,
            selectors: data.selectors,
            source_signature: ids.source_signature,
            version: Version::V1_1,
        }
    }
}

pub(crate) fn schemas_from_arguments(
    arguments: &BTreeMap<String, Argument>,
) -> BTreeMap<String, ArgumentSchema> {
    arguments
        .iter()
        .map(|(name, argument)| {
            let schema = match argument {
                Argument::Text { .. } | Argument::Number { .. } | Argument::Boolean { .. } => {
                    ArgumentSchema::Scalar
                }
                Argument::Opaque { .. } => ArgumentSchema::Opaque,
                Argument::Term { form, number, .. } => ArgumentSchema::Term {
                    form: form.clone(),
                    number: number.is_some(),
                },
            };
            (name.clone(), schema)
        })
        .collect()
}

pub(crate) fn contract_signature(
    identity: &IdentityDescriptor,
    arguments: &BTreeMap<String, ArgumentSchema>,
) -> Result<String, serde_json::Error> {
    signature_hex(&ContractDescriptor {
        arguments,
        identity,
    })
}

fn validate_bound_schemas(
    schemas: &BTreeMap<String, ArgumentSchema>,
    arguments: &BTreeMap<String, Argument>,
) -> Result<(), TroxValueError> {
    if schemas.len() != arguments.len() {
        return Err(TroxValueError::new(
            "trox.argument-mismatch",
            "bound RON template arguments differ from its declarations",
        ));
    }
    for (name, schema) in schemas {
        let Some(argument) = arguments.get(name) else {
            return Err(TroxValueError::new(
                "trox.argument-mismatch",
                format!("bound RON template argument `{name}` is missing"),
            ));
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
            return Err(TroxValueError::new(
                "trox.argument-mismatch",
                format!("bound RON template argument `{name}` has the wrong kind"),
            ));
        }
    }
    Ok(())
}

impl LocalizedString {
    pub(crate) fn build_ron_template(
        identity: IdentityDescriptor,
        arguments: BTreeMap<String, ArgumentSchema>,
    ) -> Result<Self, TroxValueError> {
        validate_identity(&identity)?;
        validate_argument_schemas(&identity.pattern, &arguments)?;
        Ok(Self {
            data: Arc::new(LocalizedStringData {
                arguments: BTreeMap::new(),
                ron_template_arguments: Some(arguments),
                identity,
                selectors: vec![],
                identity_ids: OnceLock::new(),
            }),
        })
    }
}

impl IntoArgument for LocalizedString {
    fn to_argument(&self) -> Argument {
        panic!("TROX_ASSERT arbitrary LocalizedString arguments are forbidden; use opaque(value)")
    }
}

pub(crate) fn identity_id_pair(
    identity: &IdentityDescriptor,
) -> Result<(String, String), TroxValueError> {
    let ids = compute_identity_ids(identity)?;
    Ok((ids.entry_id, ids.source_signature))
}

fn compute_identity_ids(identity: &IdentityDescriptor) -> Result<IdentityIds, TroxValueError> {
    let bytes = signature(identity)
        .map_err(|error| TroxValueError::new("trox.identity", error.to_string()))?;
    Ok(IdentityIds {
        entry_id: short_id("tx1_", &bytes),
        source_signature: blake3::Hash::from_bytes(bytes).to_hex().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_ids;

    fn value(text: &str) -> LocalizedString {
        LocalizedString::build(
            IdentityDescriptor {
                identity_version: 1,
                meaning: None,
                pattern: Pattern::Text { text: text.into() },
            },
            BTreeMap::new(),
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn clone_shares_immutable_storage() {
        let value = value("Shared");
        let cloned = value.clone();
        assert!(Arc::ptr_eq(&value.data, &cloned.data));
        assert_eq!(
            value.to_canonical_json().unwrap(),
            cloned.to_canonical_json().unwrap()
        );
    }

    #[test]
    fn identity_is_hashed_lazily_and_shared_by_clones() {
        let value = value("Lazy");
        let cloned = value.clone();

        assert!(value.data.identity_ids.get().is_none());
        let expected = identity_ids(value.identity()).unwrap().0;
        assert_eq!(value.entry_id(), expected);
        assert!(value.data.identity_ids.get().is_some());
        assert_eq!(cloned.entry_id(), expected);
        assert!(Arc::ptr_eq(&value.data, &cloned.data));
    }
}
