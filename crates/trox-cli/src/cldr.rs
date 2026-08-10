use std::collections::BTreeMap;

use anyhow::{Result, bail};
use trox::{NumberFormat, PluralCategory, PluralRules, TextDirection};

pub const CLDR_VERSION: &str = "48";

const SUPPORTED_LOCALES: &[&str] = &[
    "ar", "de", "en-US", "es", "fr", "ja", "ko", "pl", "pt-BR", "pt-PT", "ru", "zh-Hans", "zh-Hant",
];

pub fn ensure_supported_locale(locale: &str) -> Result<()> {
    if !SUPPORTED_LOCALES.contains(&locale) {
        bail!(
            "locale `{locale}` is not supported by Trox's pinned CLDR {CLDR_VERSION} data; supported locales: {}",
            SUPPORTED_LOCALES.join(", ")
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LocaleData {
    pub cardinal_categories: Vec<PluralCategory>,
    pub ordinal_categories: Vec<PluralCategory>,
    pub rules: PluralRules,
    pub number_format: NumberFormat,
    pub direction: TextDirection,
}

pub fn locale_data(locale: &str) -> LocaleData {
    let language = locale.split(['-', '_']).next().unwrap_or(locale);
    let mut cardinal = BTreeMap::new();
    let mut ordinal = BTreeMap::new();
    let mut number_format = NumberFormat::default();
    let mut direction = TextDirection::Ltr;
    match language {
        "ru" => {
            cardinal.insert(
                PluralCategory::One,
                "v = 0 and i % 10 = 1 and i % 100 != 11".into(),
            );
            cardinal.insert(
                PluralCategory::Few,
                "v = 0 and i % 10 = 2..4 and i % 100 != 12..14".into(),
            );
            cardinal.insert(
                PluralCategory::Many,
                "v = 0 and i % 10 = 0 or v = 0 and i % 10 = 5..9 or v = 0 and i % 100 = 11..14"
                    .into(),
            );
            number_format.group = " ".into();
            number_format.decimal = ",".into();
        }
        "pl" => {
            cardinal.insert(PluralCategory::One, "i = 1 and v = 0".into());
            cardinal.insert(
                PluralCategory::Few,
                "v = 0 and i % 10 = 2..4 and i % 100 != 12..14".into(),
            );
            cardinal.insert(PluralCategory::Many, "v = 0 and i != 1 and i % 10 = 0..1 or v = 0 and i % 10 = 5..9 or v = 0 and i % 100 = 12..14".into());
            number_format.group = " ".into();
            number_format.decimal = ",".into();
            number_format.minimum_grouping_digits = 2;
        }
        "ar" => {
            cardinal.insert(PluralCategory::Zero, "n = 0".into());
            cardinal.insert(PluralCategory::One, "n = 1".into());
            cardinal.insert(PluralCategory::Two, "n = 2".into());
            cardinal.insert(PluralCategory::Few, "n % 100 = 3..10".into());
            cardinal.insert(PluralCategory::Many, "n % 100 = 11..99".into());
            // CLDR 48's default numbering system for `ar` is `latn`.
            number_format.minus = "\u{200e}-".into();
            number_format.plus = "\u{200e}+".into();
            direction = TextDirection::Rtl;
        }
        "fr" => {
            cardinal.insert(PluralCategory::One, "i = 0,1".into());
            cardinal.insert(PluralCategory::Many, "i != 0 and i % 1000000 = 0".into());
            ordinal.insert(PluralCategory::One, "n = 1".into());
            number_format.group = " ".into();
            number_format.decimal = ",".into();
        }
        "pt" => {
            cardinal.insert(
                PluralCategory::One,
                if locale.eq_ignore_ascii_case("pt-PT") {
                    "i = 1 and v = 0"
                } else {
                    "i = 0..1"
                }
                .into(),
            );
            cardinal.insert(PluralCategory::Many, "i != 0 and i % 1000000 = 0".into());
            number_format.group = if locale.eq_ignore_ascii_case("pt-PT") {
                " "
            } else {
                "."
            }
            .into();
            number_format.decimal = ",".into();
            if locale.eq_ignore_ascii_case("pt-PT") {
                number_format.minimum_grouping_digits = 2;
            }
        }
        "es" | "de" | "en" => {
            cardinal.insert(PluralCategory::One, "i = 1 and v = 0".into());
            if language == "es" {
                cardinal.insert(PluralCategory::Many, "i != 0 and i % 1000000 = 0".into());
                number_format.minimum_grouping_digits = 2;
            }
            if language != "en" {
                number_format.group = ".".into();
                number_format.decimal = ",".into();
            }
        }
        _ => {}
    }
    if language == "en" {
        ordinal.insert(PluralCategory::One, "n % 10 = 1 and n % 100 != 11".into());
        ordinal.insert(PluralCategory::Two, "n % 10 = 2 and n % 100 != 12".into());
        ordinal.insert(PluralCategory::Few, "n % 10 = 3 and n % 100 != 13".into());
    }
    cardinal.insert(PluralCategory::Other, String::new());
    ordinal.insert(PluralCategory::Other, String::new());
    let categories = |rules: &BTreeMap<PluralCategory, String>| {
        [
            PluralCategory::Zero,
            PluralCategory::One,
            PluralCategory::Two,
            PluralCategory::Few,
            PluralCategory::Many,
            PluralCategory::Other,
        ]
        .into_iter()
        .filter(|category| rules.contains_key(category))
        .collect()
    };
    LocaleData {
        cardinal_categories: categories(&cardinal),
        ordinal_categories: categories(&ordinal),
        rules: PluralRules { cardinal, ordinal },
        number_format,
        direction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locale_categories_cover_stress_targets() {
        assert_eq!(
            locale_data("ja").cardinal_categories,
            vec![PluralCategory::Other]
        );
        assert!(
            locale_data("ru")
                .cardinal_categories
                .contains(&PluralCategory::Few)
        );
        assert_eq!(locale_data("ar").direction, TextDirection::Rtl);
    }

    #[test]
    fn unsupported_locales_are_rejected_instead_of_using_default_data() {
        for locale in ["cs", "he", "en-IN"] {
            assert!(ensure_supported_locale(locale).is_err(), "{locale}");
        }
        for locale in ["ar", "es", "ja", "ru"] {
            ensure_supported_locale(locale).unwrap();
        }
    }

    #[test]
    fn supported_locale_data_matches_cldr_48_edge_cases() {
        for locale in ["es", "fr", "pt-BR", "pt-PT"] {
            assert!(
                locale_data(locale)
                    .cardinal_categories
                    .contains(&PluralCategory::Many),
                "{locale}"
            );
        }
        assert_eq!(locale_data("es").number_format.minimum_grouping_digits, 2);
        assert_eq!(locale_data("pl").number_format.minimum_grouping_digits, 2);
        assert_eq!(
            locale_data("pt-PT").number_format.minimum_grouping_digits,
            2
        );
        assert_eq!(locale_data("pt-PT").number_format.group, " ");
        assert_eq!(locale_data("pt-BR").number_format.group, ".");
        assert_eq!(locale_data("ar").number_format.digits, "0123456789");
        assert_eq!(locale_data("ar").number_format.decimal, ".");
    }
}
