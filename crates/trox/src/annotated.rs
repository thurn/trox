use std::collections::BTreeMap;

use crate::{LocalizedString, TroxValueError};

/// A lazy localized value paired with application-owned placeholder metadata.
///
/// The metadata is intentionally not part of Trox's canonical wire format.
/// Serialize [`Self::localized`] and the application metadata separately when
/// crossing a process boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotatedLocalizedString<T> {
    localized: LocalizedString,
    annotations: BTreeMap<String, T>,
}

impl<T> AnnotatedLocalizedString<T> {
    /// Returns the underlying unresolved localized value.
    pub fn localized(&self) -> &LocalizedString {
        &self.localized
    }

    /// Returns application metadata keyed by semantic placeholder name.
    pub fn annotations(&self) -> &BTreeMap<String, T> {
        &self.annotations
    }

    /// Splits the wrapper back into its independently serializable owners.
    pub fn into_parts(self) -> (LocalizedString, BTreeMap<String, T>) {
        (self.localized, self.annotations)
    }
}

impl LocalizedString {
    /// Associates opaque application metadata with declared placeholders
    /// without resolving this value.
    pub fn annotate<I, K, T>(
        self,
        annotations: I,
    ) -> Result<AnnotatedLocalizedString<T>, TroxValueError>
    where
        I: IntoIterator<Item = (K, T)>,
        K: Into<String>,
    {
        let mut by_name = BTreeMap::new();
        for (name, annotation) in annotations {
            let name = name.into();
            if by_name.insert(name.clone(), annotation).is_some() {
                return Err(TroxValueError::new(
                    "trox.duplicate-annotation",
                    format!("placeholder annotation `{name}` was supplied more than once"),
                ));
            }
        }
        if let Some(name) = by_name.keys().find(|name| {
            !self.arguments().contains_key(name.as_str())
                && !self
                    .ron_template_arguments()
                    .is_some_and(|schemas| schemas.contains_key(name.as_str()))
        }) {
            return Err(TroxValueError::new(
                "trox.unknown-annotation",
                format!("annotation `{name}` does not name a declared placeholder"),
            ));
        }
        Ok(AnnotatedLocalizedString {
            localized: self,
            annotations: by_name,
        })
    }
}
