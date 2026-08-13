use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Argument, ArgumentSchema, IdentityDescriptor, Version};
use crate::value::LocalizedString;
use crate::{SourceCatalog, TroxValueError};

/// Canonical, argument-free reference to a source-catalog message contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMessageRef {
    /// Signature of the source identity and placeholder schemas.
    pub contract_signature: String,
    /// Stable message entry ID.
    pub entry_id: String,
    /// Wire format discriminator.
    pub format: String,
    /// Full stable source identity signature.
    pub source_signature: String,
    /// Source-reference wire version.
    pub version: Version,
}

/// Immutable, catalog-authorized source message awaiting argument binding.
#[derive(Debug, Clone)]
pub struct SourceMessage {
    pub(crate) catalog: SourceCatalog,
    pub(crate) argument_schemas: BTreeMap<String, ArgumentSchema>,
    pub(crate) identity: IdentityDescriptor,
    pub(crate) reference: SourceMessageRef,
}

impl SourceMessage {
    /// Returns the authorized placeholder schemas.
    pub fn argument_schemas(&self) -> &BTreeMap<String, ArgumentSchema> {
        &self.argument_schemas
    }

    /// Returns the canonical source-message reference.
    pub fn source_ref(&self) -> &SourceMessageRef {
        &self.reference
    }

    /// Binds an exact argument set and returns a normal localized value.
    pub fn bind(
        &self,
        arguments: BTreeMap<String, Argument>,
    ) -> Result<LocalizedString, TroxValueError> {
        self.catalog.bind_source_message(self, arguments)
    }
}

impl SourceMessageRef {
    /// Encodes the reference as deterministic RFC 8785 canonical JSON.
    pub fn to_canonical_json(&self) -> Result<String, crate::SerializeError> {
        crate::canonical::canonical_json(self)
    }
}
