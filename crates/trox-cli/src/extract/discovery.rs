//! Source-file discovery and scanner assignment.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

use crate::config::{Language, ProjectConfig, SourceConfig};
use crate::diagnostic::DiagnosticResultExt;

struct ScannerRecord {
    language: Language,
    includes: GlobSet,
    excludes: GlobSet,
}

impl ScannerRecord {
    fn compile(source: &SourceConfig) -> Result<Self> {
        Ok(Self {
            language: source.language,
            includes: compile_globs(&source.include)?,
            excludes: compile_globs(&source.exclude)?,
        })
    }

    fn matches(&self, relative: &Path) -> bool {
        self.includes.is_match(relative) && !self.excludes.is_match(relative)
    }
}

#[derive(Debug)]
struct DescriptionDefault {
    matcher: GlobMatcher,
    description: String,
}

pub(super) fn discover_source_files(
    config: &ProjectConfig,
) -> Result<Vec<(PathBuf, Language, Option<String>)>> {
    discover_source_files_impl(config).diagnostic(
        "trox.invalid-source-discovery",
        Some(config.path.clone()),
        "Correct source globs and keep generated outputs outside included source paths.",
    )
}

fn discover_source_files_impl(
    config: &ProjectConfig,
) -> Result<Vec<(PathBuf, Language, Option<String>)>> {
    let scanners = config
        .sources
        .iter()
        .map(ScannerRecord::compile)
        .collect::<Result<Vec<_>>>()?;
    let description_defaults = compile_description_defaults(config)?;
    let outputs = configured_outputs(config)?;
    let root_identity = crate::config::destination_identity(&config.root)?;
    for output in &outputs {
        let Ok(relative) = output.strip_prefix(&root_identity) else {
            continue;
        };
        if scanners.iter().any(|scanner| scanner.matches(relative)) {
            bail!(
                "configured output {} is inside an included source glob",
                relative.display()
            );
        }
    }
    let mut result = BTreeMap::<PathBuf, (Language, Option<String>)>::new();

    for entry in WalkDir::new(&config.root).follow_links(false) {
        let entry = entry.map_err(|error| {
            let path = error
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| config.root.clone());
            anyhow::anyhow!(error).context(format!("failed to traverse {}", path.display()))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(&config.root).with_context(|| {
            format!(
                "discovered source {} is outside project root {}",
                entry.path().display(),
                config.root.display()
            )
        })?;
        for scanner in scanners.iter().filter(|scanner| scanner.matches(relative)) {
            let default = (scanner.language == Language::Ron)
                .then(|| ron_default(&description_defaults, relative))
                .flatten();
            if let Some((prior, _)) =
                result.insert(entry.path().to_path_buf(), (scanner.language, default))
                && prior != scanner.language
            {
                bail!(
                    "source file {} is matched by multiple languages",
                    relative.display()
                );
            }
        }
    }

    Ok(result
        .into_iter()
        .map(|(path, (language, default))| (path, language, default))
        .collect())
}

fn configured_outputs(config: &ProjectConfig) -> Result<BTreeSet<PathBuf>> {
    std::iter::once(&config.source_bundle)
        .chain(config.source_report.iter())
        .chain(
            config
                .locales
                .values()
                .flat_map(|locale| [&locale.profile, &locale.csv, &locale.bundle]),
        )
        .map(|output| crate::config::destination_identity(&config.resolve(output)))
        .collect()
}

fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid source glob `{pattern}`"))?);
    }
    builder.build().context("failed to compile source globs")
}

fn compile_description_defaults(config: &ProjectConfig) -> Result<Vec<DescriptionDefault>> {
    config
        .ron_description_defaults
        .iter()
        .map(|(pattern, description)| {
            if description.trim().is_empty() {
                bail!("RON description default for `{pattern}` must not be empty");
            }
            if description.nfc().collect::<String>() != *description || description.contains('\r') {
                bail!(
                    "RON description default for `{pattern}` must be NFC and use LF line endings"
                );
            }
            Ok(DescriptionDefault {
                matcher: Glob::new(pattern)
                    .with_context(|| format!("invalid RON description-default glob `{pattern}`"))?
                    .compile_matcher(),
                description: description.clone(),
            })
        })
        .collect()
}

fn ron_default(defaults: &[DescriptionDefault], relative: &Path) -> Option<String> {
    let relative = relative.to_string_lossy().replace('\\', "/");
    defaults
        .iter()
        .find(|default| default.matcher.is_match(&relative))
        .map(|default| default.description.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_default_globs_are_checked_during_discovery_setup() {
        let mut config: ProjectConfig = ron::from_str(
            r#"(
                source_locale: "en-US",
                terms: "terms.ron",
                source_bundle: "bundle.json",
                sources: [(language: Ron, include: ["**/*.ron"])],
                locales: {},
                ron_description_defaults: { "[": "broken" },
            )"#,
        )
        .unwrap();
        config.root = PathBuf::from(".");
        assert!(
            compile_description_defaults(&config)
                .unwrap_err()
                .to_string()
                .contains("invalid RON description-default glob")
        );
    }

    #[test]
    fn nonexistent_outputs_are_preflighted_against_source_globs() {
        let directory = tempfile::tempdir().unwrap();
        let mut config: ProjectConfig = ron::from_str(
            r#"(
                source_locale: "en-US",
                terms: "terms.ron",
                source_bundle: "generated/bundle.json",
                sources: [(language: TypeScript, include: ["generated/**/*"])],
                locales: {},
            )"#,
        )
        .unwrap();
        config.root = directory.path().to_path_buf();
        config.path = directory.path().join("trox.ron");

        let error = discover_source_files(&config).unwrap_err().to_string();
        assert!(error.contains("configured output generated/bundle.json"));
    }

    #[test]
    fn explicit_globs_can_include_conventionally_ignored_directories() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("target/generated")).unwrap();
        std::fs::write(
            directory.path().join("target/generated/message.rs"),
            r#"tx("Generated", "Configured generated source.")"#,
        )
        .unwrap();
        let mut config: ProjectConfig = ron::from_str(
            r#"(
                source_locale: "en-US",
                terms: "terms.ron",
                source_bundle: "bundle.json",
                sources: [(language: Rust, include: ["target/**/*.rs"])],
                locales: {},
            )"#,
        )
        .unwrap();
        config.root = directory.path().to_path_buf();
        config.path = directory.path().join("trox.ron");

        let files = discover_source_files(&config).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].0.ends_with("target/generated/message.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_outputs_are_checked_at_their_actual_destination() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("src")).unwrap();
        symlink(directory.path().join("src"), directory.path().join("out")).unwrap();
        let mut config: ProjectConfig = ron::from_str(
            r#"(
                source_locale: "en-US",
                terms: "terms.ron",
                source_bundle: "out/generated.ts",
                sources: [(language: TypeScript, include: ["src/**/*.ts"])],
                locales: {},
            )"#,
        )
        .unwrap();
        config.root = directory.path().to_path_buf();
        config.path = directory.path().join("trox.ron");

        let error = discover_source_files(&config).unwrap_err().to_string();
        assert!(
            error.contains("configured output src/generated.ts"),
            "{error}"
        );
    }
}
