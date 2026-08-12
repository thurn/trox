use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::canonical::{canonical_json, short_id, signature};
use crate::model::{
    Argument, IdentityDescriptor, IntoArgument, Pattern, SelectorRecord, Version,
    validate_arguments, validate_identity, validate_selectors,
};
use crate::pattern::ASSERT_LOCALIZED_MEANING;
use crate::{SerializeError, TroxValueError};

/// Canonical serialized representation of a [`LocalizedString`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalizedStringWire {
    /// Arguments keyed by placeholder name.
    pub arguments: BTreeMap<String, Argument>,
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
struct LocalizedStringWireRef<'a> {
    arguments: &'a BTreeMap<String, Argument>,
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
            .field("identity", &self.data.identity)
            .field("selectors", &self.data.selectors)
            .finish()
    }
}

impl PartialEq for LocalizedString {
    fn eq(&self, other: &Self) -> bool {
        self.data.arguments == other.data.arguments
            && self.data.identity == other.data.identity
            && self.data.selectors == other.data.selectors
    }
}

impl LocalizedString {
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

    /// Returns the locale-independent identity descriptor.
    pub fn identity(&self) -> &IdentityDescriptor {
        &self.data.identity
    }

    /// Returns the placeholder arguments owned by this value.
    pub fn arguments(&self) -> &BTreeMap<String, Argument> {
        &self.data.arguments
    }

    /// Returns runtime selector values in canonical path order.
    pub fn selectors(&self) -> &[SelectorRecord] {
        &self.data.selectors
    }

    /// Returns whether this value is a plain text leaf without arguments or selectors.
    pub fn is_atomic(&self) -> bool {
        matches!(self.data.identity.pattern, Pattern::Text { .. })
            && self.data.arguments.is_empty()
            && self.data.selectors.is_empty()
    }

    pub(crate) fn is_asserted_localized(&self) -> bool {
        self.data.identity.meaning.as_deref() == Some(ASSERT_LOCALIZED_MEANING)
            && matches!(self.data.identity.pattern, Pattern::Text { .. })
            && self.data.arguments.is_empty()
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
        let ids = self.data.ids();
        canonical_json(&LocalizedStringWireRef {
            arguments: &self.data.arguments,
            entry_id: &ids.entry_id,
            format: "trox-localized-string",
            identity: &self.data.identity,
            selectors: &self.data.selectors,
            source_signature: &ids.source_signature,
            version: Version::V1,
        })
    }

    pub(crate) fn into_wire(self) -> LocalizedStringWire {
        let data = Arc::unwrap_or_clone(self.data);
        let ids = data.identity_ids.into_inner().unwrap_or_else(|| {
            compute_identity_ids(&data.identity)
                .expect("validated Trox identities always have a canonical encoding")
        });
        LocalizedStringWire {
            arguments: data.arguments,
            entry_id: ids.entry_id,
            format: "trox-localized-string".to_owned(),
            identity: data.identity,
            selectors: data.selectors,
            source_signature: ids.source_signature,
            version: Version::V1,
        }
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
