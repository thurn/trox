use std::collections::BTreeMap;

use serde::ser::{Error as _, SerializeStructVariant};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    ArgumentSchema, IdentityDescriptor, LocalizedString, Pattern, TroxValueError, tx_owned,
};

/// Declared runtime value category for a placeholder in a RON `Tx` template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RonPlaceholder {
    /// Text, a finite number, or a boolean value.
    Scalar,
    /// An atomic nested localized value.
    Opaque,
    /// A project term with an exact form and number-presence contract.
    Term {
        /// Requested named form, or the term default when omitted.
        #[serde(default, deserialize_with = "plain_optional")]
        form: Option<String>,
        /// Whether the term binding requires a cardinal number.
        #[serde(default)]
        number: bool,
    },
}

impl From<RonPlaceholder> for ArgumentSchema {
    fn from(value: RonPlaceholder) -> Self {
        match value {
            RonPlaceholder::Scalar => Self::Scalar,
            RonPlaceholder::Opaque => Self::Opaque,
            RonPlaceholder::Term { form, number } => Self::Term { form, number },
        }
    }
}

impl From<ArgumentSchema> for RonPlaceholder {
    fn from(value: ArgumentSchema) -> Self {
        match value {
            ArgumentSchema::Scalar => Self::Scalar,
            ArgumentSchema::Opaque => Self::Opaque,
            ArgumentSchema::Term { form, number } => Self::Term { form, number },
        }
    }
}

/// The checked payload of RON's `Tx(text: ..., ...)` authoring variant.
///
/// Applications normally embed this as the payload for their own enum variant;
/// the CLI scanner recognizes the conventional `Tx` spelling independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RonTx {
    /// Source-language message text.
    pub text: String,
    /// Translator-facing context that does not affect message identity.
    #[serde(default, deserialize_with = "plain_optional")]
    pub description: Option<String>,
    /// Optional semantic discriminator used when computing message identity.
    #[serde(default, deserialize_with = "plain_optional")]
    pub meaning: Option<String>,
    /// Unbound placeholder schemas for a source template.
    #[serde(default)]
    pub placeholders: BTreeMap<String, RonPlaceholder>,
}

fn plain_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl TryFrom<RonTx> for LocalizedString {
    type Error = TroxValueError;
    fn try_from(value: RonTx) -> Result<Self, Self::Error> {
        value.into_localized_string()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum NamedRonLocalizedString {
    Tx {
        text: String,
        #[serde(default, deserialize_with = "plain_optional")]
        description: Option<String>,
        #[serde(default, deserialize_with = "plain_optional")]
        meaning: Option<String>,
        #[serde(default)]
        placeholders: BTreeMap<String, RonPlaceholder>,
    },
}

#[derive(Deserialize)]
enum PositionalRonLocalizedString {
    Tx(String),
}

impl<'de> Deserialize<'de> for LocalizedString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<ron::value::RawValue>::deserialize(deserializer)?;
        let (text, description, meaning, placeholders) =
            match ron::from_str::<PositionalRonLocalizedString>(raw.get_ron()) {
                Ok(PositionalRonLocalizedString::Tx(text)) => (text, None, None, BTreeMap::new()),
                Err(_) => {
                    let NamedRonLocalizedString::Tx {
                        text,
                        description,
                        meaning,
                        placeholders,
                    } = ron::from_str(raw.get_ron()).map_err(serde::de::Error::custom)?;
                    (text, description, meaning, placeholders)
                }
            };
        RonTx {
            text,
            description,
            meaning,
            placeholders,
        }
        .into_localized_string()
        .map_err(serde::de::Error::custom)
    }
}

impl Serialize for LocalizedString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(arguments) = self.ron_template_arguments() {
            if !self.selectors().is_empty() {
                return Err(S::Error::custom(
                    "RON Tx templates do not support selectors",
                ));
            }
            let Pattern::Text { text } = &self.identity().pattern else {
                return Err(S::Error::custom(
                    "RON Tx templates support only text patterns",
                ));
            };
            let placeholders: BTreeMap<String, RonPlaceholder> = arguments
                .iter()
                .map(|(name, schema)| (name.clone(), schema.clone().into()))
                .collect();
            let fields = if self.identity().meaning.is_some() {
                3
            } else {
                2
            };
            let mut variant =
                serializer.serialize_struct_variant("LocalizedString", 0, "Tx", fields)?;
            variant.serialize_field("text", text)?;
            if let Some(meaning) = self.identity().meaning.as_deref() {
                variant.serialize_field("meaning", meaning)?;
            }
            variant.serialize_field("placeholders", &placeholders)?;
            return variant.end();
        }
        if !self.arguments().is_empty() || !self.selectors().is_empty() {
            return Err(S::Error::custom(
                "RON Tx serialization supports only static text values",
            ));
        }
        let Pattern::Text { text } = &self.identity().pattern else {
            return Err(S::Error::custom(
                "RON Tx serialization supports only static text values",
            ));
        };

        let Some(meaning) = self.identity().meaning.as_deref() else {
            return serializer.serialize_newtype_variant("LocalizedString", 0, "Tx", text);
        };

        let mut variant = serializer.serialize_struct_variant("LocalizedString", 0, "Tx", 2)?;
        variant.serialize_field("text", text)?;
        variant.serialize_field("meaning", meaning)?;
        variant.end()
    }
}

impl RonTx {
    /// Validates this RON payload and converts it into an owned localized string.
    ///
    /// The description is authoring metadata and is intentionally not retained
    /// in the runtime value or used to compute its identity.
    pub fn into_localized_string(self) -> Result<LocalizedString, TroxValueError> {
        if self.placeholders.is_empty() {
            return tx_owned(self.text, self.meaning);
        }
        LocalizedString::build_ron_template(
            IdentityDescriptor {
                identity_version: 1,
                meaning: self.meaning,
                pattern: Pattern::Text { text: self.text },
            },
            self.placeholders
                .into_iter()
                .map(|(name, schema)| (name, schema.into()))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ron_variant_deserializes_directly_to_localized_string() {
        let value: LocalizedString =
            ron::from_str(r#"Tx(text:"Open",description:"Room state.",meaning:"open-state")"#)
                .unwrap();
        assert_eq!(value.identity().meaning.as_deref(), Some("open-state"));
        let omitted: LocalizedString = ron::from_str(r#"Tx(text:"Close")"#).unwrap();
        assert!(omitted.is_atomic());
        assert_eq!(
            omitted,
            crate::tx("Close", "Description does not enter identity.")
        );
    }

    #[test]
    fn ron_placeholder_template_round_trips_and_binds() {
        let source = r#"Tx(text:"Deck: {deck_name} ({count})",placeholders:{"count":Scalar,"deck_name":Opaque})"#;
        let template: LocalizedString = ron::from_str(source).unwrap();
        assert!(!template.is_atomic());
        assert_eq!(
            template.ron_template_arguments().unwrap(),
            &BTreeMap::from([
                ("count".into(), ArgumentSchema::Scalar),
                ("deck_name".into(), ArgumentSchema::Opaque),
            ])
        );
        assert_eq!(ron::to_string(&template).unwrap(), source);
        assert!(matches!(
            template.to_canonical_json(),
            Err(crate::SerializeError::UnboundRonTemplate)
        ));

        let deck_name = crate::tx("Night Garden", "Deck name.");
        let bound = template
            .bind_ron_template(crate::tx_args![
                count => 3_u32,
                deck_name => crate::opaque(deck_name),
            ])
            .unwrap();
        assert!(bound.to_canonical_json().is_ok());

        let wrong_kind = template
            .bind_ron_template(crate::tx_args![
                count => crate::opaque(crate::tx("Three", "Count.")),
                deck_name => crate::opaque(crate::tx("Night Garden", "Deck name.")),
            ])
            .unwrap_err();
        assert_eq!(wrong_kind.code, "trox.argument-mismatch");

        let missing = template
            .bind_ron_template(crate::tx_args![count => 3_u32])
            .unwrap_err();
        assert_eq!(missing.code, "trox.argument-mismatch");
    }

    #[test]
    fn ron_serializes_static_localized_strings_as_tx_values() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Record {
            label: LocalizedString,
        }

        let value = crate::tx("Hello", "Greeting.");
        let record = Record { label: value };
        let serialized = ron::to_string(&record).unwrap();
        assert_eq!(serialized, r#"(label:Tx("Hello"))"#);
        assert_eq!(ron::from_str::<Record>(&serialized).unwrap(), record);

        let distinguished = crate::tx(
            crate::meaning("open-state", "Open"),
            "Status for an open room.",
        );
        let serialized = ron::to_string(&distinguished).unwrap();
        assert_eq!(serialized, r#"Tx(text:"Open",meaning:"open-state")"#);
        assert_eq!(
            ron::from_str::<LocalizedString>(&serialized).unwrap(),
            distinguished
        );
    }

    #[test]
    fn ron_rejects_placeholders() {
        let error =
            ron::from_str::<LocalizedString>(r#"Tx(text:"Deck: {deck_name}")"#).unwrap_err();
        assert!(error.to_string().contains("argument-mismatch"));
    }

    #[test]
    fn ron_rejects_placeholder_declaration_mismatches() {
        for source in [
            r#"Tx(text:"Deck: {deck_name}",placeholders:{"other":Opaque})"#,
            r#"Tx(text:"Deck",placeholders:{"deck_name":Opaque})"#,
        ] {
            let error = ron::from_str::<LocalizedString>(source).unwrap_err();
            assert!(error.to_string().contains("argument-mismatch"), "{error}");
        }
    }

    #[test]
    fn ron_localized_string_rejects_unknown_tx_fields() {
        let error = ron::from_str::<LocalizedString>(
            r#"Tx(text:"Open",description:"Room state.",unexpected:"ignored")"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unexpected"), "{error}");
    }

    #[test]
    fn ron_tx_rejects_unknown_fields() {
        let error = ron::from_str::<RonTx>(r#"(text:"Open",unexpected:"ignored")"#).unwrap_err();

        assert!(error.to_string().contains("unexpected"), "{error}");
    }
}
