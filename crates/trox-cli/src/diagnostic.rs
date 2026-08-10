use std::fmt;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub rule_id: String,
    pub severity: Severity,
    pub path: Option<PathBuf>,
    pub span: Option<Span>,
    pub message: String,
    pub guidance: String,
}

impl Diagnostic {
    pub fn error(rule_id: impl Into<String>, message: impl Into<String>) -> Self {
        let rule_id = rule_id.into();
        Self {
            guidance: "Correct the reported contract violation and rerun `trox check`.".into(),
            rule_id,
            severity: Severity::Error,
            path: None,
            span: None,
            message: message.into(),
        }
    }
    pub fn warning(rule_id: impl Into<String>, message: impl Into<String>) -> Self {
        let rule_id = rule_id.into();
        Self {
            guidance: format!(
                "Fix the finding, change `{rule_id}` severity, or add a reasoned allow in `lint.rules`."
            ),
            rule_id,
            severity: Severity::Warning,
            path: None,
            span: None,
            message: message.into(),
        }
    }
    pub fn at(mut self, path: impl Into<PathBuf>, span: Span) -> Self {
        self.path = Some(path.into());
        self.span = Some(span);
        self
    }
    pub fn with_guidance(mut self, guidance: impl Into<String>) -> Self {
        self.guidance = guidance.into();
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{source:#}")]
pub struct ClassifiedError {
    pub diagnostic: Diagnostic,
    #[source]
    source: anyhow::Error,
}

pub trait DiagnosticResultExt<T> {
    fn diagnostic(
        self,
        rule_id: impl Into<String>,
        path: Option<PathBuf>,
        guidance: impl Into<String>,
    ) -> Result<T>;
}

impl<T> DiagnosticResultExt<T> for Result<T> {
    fn diagnostic(
        self,
        rule_id: impl Into<String>,
        path: Option<PathBuf>,
        guidance: impl Into<String>,
    ) -> Result<T> {
        self.map_err(|source| {
            if source
                .chain()
                .any(|cause| cause.downcast_ref::<ClassifiedError>().is_some())
            {
                return source;
            }
            let mut diagnostic =
                Diagnostic::error(rule_id, format!("{source:#}")).with_guidance(guidance);
            diagnostic.path = path;
            anyhow::Error::new(ClassifiedError { diagnostic, source })
        })
    }
}

pub fn classified_diagnostic(error: &anyhow::Error) -> Option<Diagnostic> {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<ClassifiedError>())
        .last()
        .map(|classified| classified.diagnostic.clone())
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        if let Some(path) = &self.path {
            if let Some(span) = self.span {
                write!(
                    formatter,
                    "{}:{}:{}: {severity}[{}]: {}",
                    path.display(),
                    span.line,
                    span.column,
                    self.rule_id,
                    self.message
                )?;
            } else {
                write!(
                    formatter,
                    "{}: {severity}[{}]: {}",
                    path.display(),
                    self.rule_id,
                    self.message
                )?;
            }
        } else {
            write!(formatter, "{severity}[{}]: {}", self.rule_id, self.message)?;
        }
        if !self.guidance.is_empty() {
            write!(formatter, "\n  help: {}", self.guidance)?;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.items.push(diagnostic);
    }
    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.items.extend(diagnostics);
    }
    pub fn has_errors(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.severity == Severity::Error)
    }
    pub fn has_warnings(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.severity == Severity::Warning)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn apply_warning_policy(
        &mut self,
        mut level: impl FnMut(&str) -> crate::config::LintLevel,
    ) {
        self.items.retain_mut(|item| {
            if item.severity != Severity::Warning {
                return true;
            }
            match level(&item.rule_id) {
                crate::config::LintLevel::Allow => false,
                crate::config::LintLevel::Warn => true,
                crate::config::LintLevel::Deny => {
                    item.severity = Severity::Error;
                    true
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LintLevel;

    #[test]
    fn warning_policy_can_allow_warn_and_deny_named_rules() {
        let mut diagnostics = Diagnostics::default();
        diagnostics.push(Diagnostic::warning("trox.allowed", "allowed"));
        diagnostics.push(Diagnostic::warning("trox.warned", "warned"));
        diagnostics.push(Diagnostic::warning("trox.denied", "denied"));
        diagnostics.apply_warning_policy(|rule| match rule {
            "trox.allowed" => LintLevel::Allow,
            "trox.denied" => LintLevel::Deny,
            _ => LintLevel::Warn,
        });
        let items = diagnostics.into_vec();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].severity, Severity::Warning);
        assert_eq!(items[1].severity, Severity::Error);
        assert!(items.iter().all(|item| !item.guidance.is_empty()));
    }
}
