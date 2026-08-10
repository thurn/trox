use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};

use crate::diagnostic::DiagnosticResultExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Language {
    Rust,
    TypeScript,
    Ron,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub language: Language,
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocaleConfig {
    pub profile: PathBuf,
    pub csv: PathBuf,
    pub bundle: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum LintLevel {
    Allow,
    #[default]
    Warn,
    Deny,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintRuleConfig {
    pub level: LintLevel,
    #[serde(default, deserialize_with = "plain_optional")]
    pub reason: Option<String>,
}

impl LintRuleConfig {
    pub fn level(&self) -> LintLevel {
        self.level
    }

    fn validate(&self, rule: &str) -> Result<()> {
        match (self.level, self.reason.as_deref()) {
            (LintLevel::Allow, None) => {
                bail!("lint rule `{rule}` cannot be allowed without a reason")
            }
            (LintLevel::Allow, Some(reason)) if reason.trim().is_empty() => {
                bail!("lint rule `{rule}` has an empty allow reason")
            }
            (level, Some(_)) if level != LintLevel::Allow => {
                bail!("lint rule `{rule}` may include a reason only when its level is Allow")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    #[serde(default)]
    pub default_warning: LintLevel,
    #[serde(default)]
    pub rules: BTreeMap<String, LintRuleConfig>,
}

impl LintConfig {
    pub fn level(&self, rule: &str) -> LintLevel {
        self.rules
            .get(rule)
            .map(LintRuleConfig::level)
            .unwrap_or(self.default_warning)
    }

    pub fn set_cli_level(
        &mut self,
        rule: String,
        level: LintLevel,
        reason: Option<String>,
    ) -> Result<()> {
        if !valid_rule_id(&rule) {
            bail!("invalid lint rule ID `{rule}`");
        }
        let setting = LintRuleConfig { level, reason };
        setting.validate(&rule)?;
        self.rules.insert(rule, setting);
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        if self.default_warning == LintLevel::Allow {
            bail!(
                "lint.default_warning cannot be Allow because suppressions require a named rule and reason"
            );
        }
        for (rule, setting) in &self.rules {
            if !valid_rule_id(rule) {
                bail!("invalid lint rule ID `{rule}`");
            }
            setting.validate(rule)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum NumberPolicy {
    #[default]
    Forbidden,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum SourceFallback {
    Default,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TermFormDeclaration {
    pub description: String,
    #[serde(default)]
    pub number: NumberPolicy,
    #[serde(default, deserialize_with = "plain_optional")]
    pub source_fallback: Option<SourceFallback>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub source_locale: String,
    pub terms: PathBuf,
    pub source_bundle: PathBuf,
    #[serde(default, deserialize_with = "plain_optional")]
    pub source_report: Option<PathBuf>,
    pub sources: Vec<SourceConfig>,
    pub locales: BTreeMap<String, LocaleConfig>,
    #[serde(default = "default_row_cap")]
    pub max_expanded_rows_per_entry: usize,
    #[serde(default)]
    pub lint: LintConfig,
    #[serde(default)]
    pub term_forms: BTreeMap<String, TermFormDeclaration>,
    #[serde(default)]
    pub ron_description_defaults: BTreeMap<String, String>,
    #[serde(skip)]
    pub root: PathBuf,
    #[serde(skip)]
    pub path: PathBuf,
}

fn default_row_cap() -> usize {
    256
}

pub(crate) fn plain_optional<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl ProjectConfig {
    pub fn load(path: &Path) -> Result<Self> {
        (|| {
            let text = fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let mut config: Self = ron::from_str(&text)
                .with_context(|| format!("invalid project config {}", path.display()))?;
            config.path = path.to_path_buf();
            config.root = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .canonicalize()
                .with_context(|| {
                    format!("failed to resolve config directory for {}", path.display())
                })?;
            config.validate()?;
            Ok(config)
        })()
        .diagnostic(
            "trox.invalid-config",
            Some(path.to_path_buf()),
            "Correct the project configuration and rerun the command.",
        )
    }

    pub fn discover(start: &Path) -> Result<PathBuf> {
        let mut current = start
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", start.display()))?;
        loop {
            let candidate = current.join("trox.ron");
            if candidate.is_file() {
                return Ok(candidate);
            }
            if !current.pop() {
                bail!("no trox.ron found from {}", start.display());
            }
        }
    }

    pub fn resolve(&self, path: &Path) -> PathBuf {
        normalize_path(if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        })
    }

    fn validate(&self) -> Result<()> {
        if self.sources.is_empty() {
            bail!("project must configure at least one source scanner");
        }
        if self.max_expanded_rows_per_entry == 0 || self.max_expanded_rows_per_entry > 4096 {
            bail!("max_expanded_rows_per_entry must be 1..=4096");
        }
        self.lint.validate()?;
        let language = self.source_locale.split(['-', '_']).next().unwrap_or("");
        validate_locale_id(&self.source_locale)?;
        crate::cldr::ensure_supported_locale(&self.source_locale)?;
        if language != "en" {
            bail!("source_locale must be an English BCP 47 locale");
        }
        if self.locales.contains_key(&self.source_locale) {
            bail!(
                "source locale must use source_report/source_bundle rather than a target locale record"
            );
        }
        let mut configured_paths = BTreeMap::<PathBuf, String>::new();
        let mut add_path = |path: &Path, label: String| -> Result<()> {
            let resolved = self.resolve(path);
            if crate::transaction::is_control_path(&self.root, &resolved) {
                bail!(
                    "configured path {} for {label} is reserved for transaction recovery",
                    resolved.display()
                );
            }
            let identity = destination_identity(&resolved)?;
            if let Some(prior) = configured_paths.insert(identity, label.clone()) {
                bail!(
                    "configured path {} is shared by {prior} and {label}",
                    resolved.display()
                );
            }
            Ok(())
        };
        add_path(&self.terms, "term catalog".into())?;
        add_path(&self.source_bundle, "source bundle".into())?;
        if let Some(report) = &self.source_report {
            add_path(report, "source report".into())?;
        }
        for (locale, output) in &self.locales {
            validate_locale_id(locale)?;
            crate::cldr::ensure_supported_locale(locale)?;
            add_path(&output.profile, format!("{locale} profile"))?;
            add_path(&output.csv, format!("{locale} CSV"))?;
            add_path(&output.bundle, format!("{locale} bundle"))?;
        }
        Ok(())
    }
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

pub(crate) fn destination_identity(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let absolute = normalize_path(absolute);
    let mut existing = absolute.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        suffix.push(name.to_owned());
        existing = existing.parent().unwrap_or(existing);
    }
    let mut identity = existing
        .canonicalize()
        .with_context(|| format!("failed to resolve output path {}", path.display()))?;
    for component in suffix.into_iter().rev() {
        identity.push(component);
    }
    Ok(identity)
}

fn valid_rule_id(rule: &str) -> bool {
    rule.starts_with("trox.")
        && rule.len() <= 96
        && rule.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
}

pub fn validate_locale_id(locale: &str) -> Result<()> {
    if locale.contains('_') || !locale.is_ascii() {
        bail!("locale `{locale}` is not a canonical BCP 47 identifier");
    }
    let parts: Vec<_> = locale.split('-').collect();
    let language = parts.first().copied().unwrap_or("");
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        bail!("locale `{locale}` has an invalid language subtag");
    }
    for part in &parts[1..] {
        let valid_script = part.len() == 4
            && part.as_bytes()[0].is_ascii_uppercase()
            && part.as_bytes()[1..].iter().all(u8::is_ascii_lowercase);
        let valid_region = part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_uppercase())
            || part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_digit());
        let valid_variant =
            (4..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric());
        if !(valid_script || valid_region || valid_variant) {
            bail!("locale `{locale}` has invalid or noncanonical subtag `{part}`");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_lint(lint: &str) -> String {
        format!(
            r#"(
                source_locale: "en-US",
                terms: "terms.ron",
                source_bundle: "en-US.json",
                sources: [(language: Rust, include: ["src/**/*.rs"])],
                locales: {{}},
                lint: (rules: {{"trox.human-row-expansion": {lint}}}),
            )"#
        )
    }

    #[test]
    fn lint_allow_requires_a_nonempty_reason() {
        let bare: ProjectConfig = ron::from_str(&config_with_lint("(level: Allow)")).unwrap();
        assert!(
            bare.validate()
                .unwrap_err()
                .to_string()
                .contains("without a reason")
        );
        let empty: ProjectConfig =
            ron::from_str(&config_with_lint(r#"(level: Allow, reason: "")"#)).unwrap();
        assert!(
            empty
                .validate()
                .unwrap_err()
                .to_string()
                .contains("empty allow reason")
        );
    }

    #[test]
    fn reasoned_allow_and_named_severities_are_effective() {
        let allowed: ProjectConfig = ron::from_str(&config_with_lint(
            r#"(level: Allow, reason: "Reviewed translator cost.")"#,
        ))
        .unwrap();
        allowed.validate().unwrap();
        assert_eq!(
            allowed.lint.level("trox.human-row-expansion"),
            LintLevel::Allow
        );
        assert_eq!(
            allowed.lint.level("trox.some-other-warning"),
            LintLevel::Warn
        );
    }

    #[test]
    fn default_allow_and_invalid_cli_rule_ids_are_rejected() {
        let input = r#"(
            source_locale: "en-US",
            terms: "terms.ron",
            source_bundle: "en-US.json",
            sources: [(language: Rust, include: ["src/**/*.rs"])],
            locales: {},
            lint: (default_warning: Allow),
        )"#;
        let config: ProjectConfig = ron::from_str(input).unwrap();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("named rule")
        );

        let mut lint = LintConfig::default();
        assert!(
            lint.set_cli_level("invalid".into(), LintLevel::Deny, None)
                .unwrap_err()
                .to_string()
                .contains("invalid lint rule ID")
        );
    }

    #[test]
    fn lexically_aliased_output_paths_are_rejected() {
        let input = r#"(
            source_locale: "en-US",
            terms: "terms.ron",
            source_bundle: "out/catalog.json",
            source_report: "out/nested/../catalog.json",
            sources: [(language: Rust, include: ["src/**/*.rs"])],
            locales: {},
        )"#;
        let config: ProjectConfig = ron::from_str(input).unwrap();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("is shared")
        );
    }

    #[test]
    fn project_config_rejects_locales_without_pinned_data() {
        for locale in ["cs", "he", "en-IN"] {
            let input = format!(
                r#"(
                    source_locale: "en-US",
                    terms: "terms.ron",
                    source_bundle: "en-US.json",
                    sources: [(language: Rust, include: ["src/**/*.rs"])],
                    locales: {{
                        "{locale}": (profile: "locale.ron", csv: "locale.csv", bundle: "locale.json"),
                    }},
                )"#
            );
            let config: ProjectConfig = ron::from_str(&input).unwrap();
            let error = config.validate().unwrap_err().to_string();
            assert!(error.contains("not supported"), "{locale}: {error}");
        }
    }
}
