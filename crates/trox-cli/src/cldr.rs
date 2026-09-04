use anyhow::{Result, anyhow};
use trox::{NumberFormat, PluralCategory, PluralRules, SourceLocale, TextDirection};

pub use trox::CLDR_VERSION;

pub fn ensure_supported_locale(locale: &str) -> Result<()> {
    SourceLocale::new(locale)
        .map(|_| ())
        .map_err(|error| anyhow!(error.message))
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
    let source = SourceLocale::new(locale)
        .expect("project configuration validates supported locales before requesting CLDR data");
    let categories = |rules: &std::collections::BTreeMap<PluralCategory, String>| {
        PluralCategory::CANONICAL
            .into_iter()
            .filter(|category| rules.contains_key(category))
            .collect()
    };
    LocaleData {
        cardinal_categories: categories(&source.plural_rules().cardinal),
        ordinal_categories: categories(&source.plural_rules().ordinal),
        rules: source.plural_rules().clone(),
        number_format: source.number_format().clone(),
        direction: source.direction(),
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
