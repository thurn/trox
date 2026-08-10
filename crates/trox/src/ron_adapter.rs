use serde::ser::{Error as _, SerializeStructVariant};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{LocalizedString, Pattern, TroxValueError, tx_owned};

/// The checked payload of RON's static `Tx(text: ..., ...)` authoring variant.
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
    },
}

#[derive(Deserialize)]
enum PositionalRonLocalizedString {
    Tx(String),
}

impl<'de> Deserialize<'de> for LocalizedString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<ron::value::RawValue>::deserialize(deserializer)?;
        let (text, description, meaning) =
            match ron::from_str::<PositionalRonLocalizedString>(raw.get_ron()) {
                Ok(PositionalRonLocalizedString::Tx(text)) => (text, None, None),
                Err(_) => {
                    let NamedRonLocalizedString::Tx {
                        text,
                        description,
                        meaning,
                    } = ron::from_str(raw.get_ron()).map_err(serde::de::Error::custom)?;
                    (text, description, meaning)
                }
            };
        RonTx {
            text,
            description,
            meaning,
        }
        .into_localized_string()
        .map_err(serde::de::Error::custom)
    }
}

impl Serialize for LocalizedString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
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
        tx_owned(self.text, self.meaning)
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
