//! Recoverable multi-file replacement for generated CLI artifacts.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile};

use crate::diagnostic::DiagnosticResultExt;

const MANIFEST_NAME: &str = ".trox-transaction.json";
const COMMIT_MARKER_NAME: &str = ".trox-transaction.committed";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEntry {
    destination: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    had_original: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionManifest {
    version: u8,
    entries: Vec<ManifestEntry>,
}

struct StagedReplacement {
    manifest: ManifestEntry,
    temporary: Option<NamedTempFile>,
}

/// Recover an interrupted transaction before reading project artifacts.
pub fn recover_pending_transaction(root: &Path, allowed_destinations: &[PathBuf]) -> Result<()> {
    recover_pending_impl(root, allowed_destinations).diagnostic(
        "trox.transaction-recovery",
        Some(manifest_path(root)),
        "Retry after the other Trox process exits, or inspect the retained manifest and backup paths before manual recovery.",
    )
}

pub(crate) fn is_control_path(root: &Path, path: &Path) -> bool {
    path == manifest_path(root) || path == commit_marker_path(root)
}

fn recover_pending_impl(root: &Path, allowed_destinations: &[PathBuf]) -> Result<()> {
    let path = manifest_path(root);
    let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            remove_commit_marker(root)?;
            return Ok(());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to open {}", path.display()));
        }
    };
    file.try_lock().with_context(|| {
        format!(
            "another Trox process owns the pending transaction {}",
            path.display()
        )
    })?;
    let manifest: TransactionManifest = serde_json::from_reader(&mut file)
        .with_context(|| format!("invalid transaction manifest {}", path.display()))?;
    validate_manifest(root, &manifest, allowed_destinations)?;
    finish_manifest(root, &path, &manifest)?;
    drop(file);
    remove_transaction_files(&path, root)
}

/// Stage and replace all destinations, restoring prior contents on failure.
pub fn atomic_replace_all(root: &Path, replacements: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    replace_all_impl(root, replacements, |_| Ok(()), |_| Ok(())).diagnostic(
        "trox.transaction",
        replacements.first().map(|(path, _)| path.clone()),
        "No requested artifact was intentionally committed; inspect any retained manifest and backup paths before retrying.",
    )
}

fn replace_all_impl(
    root: &Path,
    replacements: &[(PathBuf, Vec<u8>)],
    mut before_backup: impl FnMut(usize) -> Result<()>,
    mut before_commit: impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    if replacements.is_empty() {
        return Ok(());
    }
    let manifest_path = manifest_path(root);
    if manifest_path.exists() {
        bail!(
            "pending transaction {} must be recovered before starting another",
            manifest_path.display()
        );
    }

    let mut destinations = BTreeSet::new();
    let mut staged = Vec::new();
    for (destination, bytes) in replacements {
        let destination = absolute_path(destination)?;
        if !destinations.insert(destination.clone()) {
            bail!(
                "duplicate replacement destination {}",
                destination.display()
            );
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let metadata = destination.symlink_metadata().ok();
        if metadata
            .as_ref()
            .is_some_and(|metadata| metadata.file_type().is_dir())
        {
            bail!("cannot replace directory {}", destination.display());
        }
        let mut temporary = Builder::new()
            .prefix(".trox-staged-")
            .tempfile_in(parent)
            .with_context(|| format!("failed to stage {}", destination.display()))?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        let backup = reserve_backup_path(parent)?;
        staged.push(StagedReplacement {
            manifest: ManifestEntry {
                destination,
                staged: temporary.path().to_path_buf(),
                backup,
                had_original: metadata.is_some(),
            },
            temporary: Some(temporary),
        });
    }

    let manifest = TransactionManifest {
        version: 1,
        entries: staged.iter().map(|item| item.manifest.clone()).collect(),
    };
    let manifest_file = create_manifest(root, &manifest)?;

    let operation = (|| -> Result<()> {
        for (index, replacement) in staged.iter().enumerate() {
            before_backup(index)?;
            if replacement.manifest.had_original {
                fs::rename(
                    &replacement.manifest.destination,
                    &replacement.manifest.backup,
                )
                .with_context(|| {
                    format!(
                        "failed to preserve {} before replacement",
                        replacement.manifest.destination.display()
                    )
                })?;
            }
        }
        sync_manifest_parents(&manifest)?;

        for (index, replacement) in staged.iter_mut().enumerate() {
            before_commit(index)?;
            replacement
                .temporary
                .take()
                .expect("staged temporary is present")
                .persist(&replacement.manifest.destination)
                .map_err(|error| error.error)
                .with_context(|| {
                    format!(
                        "failed to replace {}",
                        replacement.manifest.destination.display()
                    )
                })?;
        }
        sync_manifest_parents(&manifest)?;
        create_commit_marker(root)?;
        Ok(())
    })();

    if let Err(error) = operation {
        for replacement in &mut staged {
            replacement.temporary.take();
        }
        drop(manifest_file);
        return match rollback(&manifest) {
            Ok(()) => remove_transaction_files(&manifest_path, root).and(Err(
                error.context("generated artifact transaction rolled back")
            )),
            Err(rollback) => Err(anyhow::anyhow!(
                "generated artifact transaction failed: {error:#}; rollback failed and backups were retained in {}: {rollback:#}",
                manifest_path.display()
            )),
        };
    }

    if let Err(error) = cleanup_committed(&manifest) {
        drop(manifest_file);
        return Err(error.context(format!(
            "artifacts were committed, but cleanup is incomplete; retained transaction manifest {} will finish cleanup on the next startup",
            manifest_path.display()
        )));
    }

    drop(manifest_file);
    remove_transaction_files(&manifest_path, root)
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join(MANIFEST_NAME)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn reserve_backup_path(parent: &Path) -> Result<PathBuf> {
    let candidate = Builder::new().prefix(".trox-backup-").tempfile_in(parent)?;
    let path = candidate.path().to_path_buf();
    candidate.close()?;
    Ok(path)
}

fn create_manifest(root: &Path, manifest: &TransactionManifest) -> Result<File> {
    let path = manifest_path(root);
    let mut temporary = Builder::new()
        .prefix(".trox-manifest-")
        .tempfile_in(root)
        .with_context(|| format!("failed to stage transaction manifest {}", path.display()))?;
    temporary
        .as_file()
        .try_lock()
        .context("failed to lock the new transaction manifest")?;
    serde_json::to_writer(&mut temporary, manifest)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    let file = temporary.persist_noclobber(&path).map_err(|error| {
        anyhow::anyhow!(error.error).context(format!(
            "failed to create transaction manifest {}",
            path.display()
        ))
    })?;
    sync_directory(root)?;
    Ok(file)
}

fn validate_manifest(
    root: &Path,
    manifest: &TransactionManifest,
    allowed_destinations: &[PathBuf],
) -> Result<()> {
    if manifest.version != 1 || manifest.entries.is_empty() {
        bail!("unsupported or empty transaction manifest");
    }
    let manifest_path = manifest_path(root);
    let allowed = allowed_destinations
        .iter()
        .map(|path| absolute_path(path))
        .collect::<Result<BTreeSet<_>>>()?;
    let mut destinations = BTreeSet::new();
    for entry in &manifest.entries {
        if !entry.destination.is_absolute()
            || !entry.staged.is_absolute()
            || !entry.backup.is_absolute()
        {
            bail!("transaction manifest paths must be absolute");
        }
        let parent = entry.destination.parent();
        let valid_staged = entry.staged.parent() == parent
            && entry
                .staged
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".trox-staged-"));
        let valid_backup = entry.backup.parent() == parent
            && entry
                .backup
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".trox-backup-"));
        if entry.destination == manifest_path
            || !allowed.contains(&entry.destination)
            || !valid_staged
            || !valid_backup
            || !destinations.insert(&entry.destination)
        {
            bail!("transaction manifest contains an invalid destination");
        }
    }
    Ok(())
}

fn finish_manifest(root: &Path, path: &Path, manifest: &TransactionManifest) -> Result<()> {
    if commit_marker_path(root).exists() {
        cleanup_committed(manifest).with_context(|| {
            format!(
                "failed to finish committed transaction cleanup {}; artifacts are committed and retained paths remain recorded",
                path.display()
            )
        })
    } else {
        rollback(manifest).with_context(|| {
            format!(
                "failed to recover interrupted transaction {}; backups remain recorded in the manifest",
                path.display()
            )
        })
    }?;
    sync_directory(root)
}

fn rollback(manifest: &TransactionManifest) -> Result<()> {
    let mut failures = Vec::new();
    for entry in manifest.entries.iter().rev() {
        if entry.had_original {
            if path_exists(&entry.backup) {
                if path_exists(&entry.destination)
                    && let Err(error) = fs::remove_file(&entry.destination)
                {
                    failures.push(format!(
                        "failed to remove replacement {}: {error}",
                        entry.destination.display()
                    ));
                    continue;
                }
                if let Err(error) = fs::rename(&entry.backup, &entry.destination) {
                    failures.push(format!(
                        "failed to restore {} from {}: {error}",
                        entry.destination.display(),
                        entry.backup.display()
                    ));
                }
            } else if !path_exists(&entry.destination) {
                failures.push(format!(
                    "both destination {} and backup {} are missing",
                    entry.destination.display(),
                    entry.backup.display()
                ));
            }
        } else if path_exists(&entry.destination)
            && let Err(error) = fs::remove_file(&entry.destination)
        {
            failures.push(format!(
                "failed to remove partial replacement {}: {error}",
                entry.destination.display()
            ));
        }
        if path_exists(&entry.staged)
            && let Err(error) = fs::remove_file(&entry.staged)
        {
            failures.push(format!(
                "failed to remove staged file {}: {error}",
                entry.staged.display()
            ));
        }
    }
    if let Err(error) = sync_manifest_parents(manifest) {
        failures.push(format!(
            "failed to synchronize restored directories: {error}"
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(failures.join("; "))
    }
}

fn cleanup_committed(manifest: &TransactionManifest) -> Result<()> {
    let mut failures = Vec::new();
    for entry in &manifest.entries {
        for path in [&entry.backup, &entry.staged] {
            if path_exists(path)
                && let Err(error) = fs::remove_file(path)
            {
                failures.push(format!("failed to remove {}: {error}", path.display()));
            }
        }
    }
    if let Err(error) = sync_manifest_parents(manifest) {
        failures.push(format!(
            "failed to synchronize artifact directories: {error}"
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(failures.join("; "))
    }
}

fn remove_transaction_files(path: &Path, root: &Path) -> Result<()> {
    remove_commit_marker(root)?;
    match fs::remove_file(path) {
        Ok(()) => sync_directory(root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn commit_marker_path(root: &Path) -> PathBuf {
    root.join(COMMIT_MARKER_NAME)
}

fn create_commit_marker(root: &Path) -> Result<()> {
    let path = commit_marker_path(root);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(b"committed\n")?;
    file.sync_all()?;
    sync_directory(root)
}

fn remove_commit_marker(root: &Path) -> Result<()> {
    let path = commit_marker_path(root);
    match fs::remove_file(&path) {
        Ok(()) => sync_directory(root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn sync_manifest_parents(manifest: &TransactionManifest) -> Result<()> {
    let parents: BTreeSet<_> = manifest
        .entries
        .iter()
        .filter_map(|entry| entry.destination.parent())
        .collect();
    for parent in parents {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn path_exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_failure_restores_every_original() {
        let directory = tempfile::tempdir().unwrap();
        let absent = directory.path().join("absent");
        let existing = directory.path().join("existing");
        let failing = directory.path().join("failing");
        fs::write(&existing, b"old existing").unwrap();
        fs::write(&failing, b"old failing").unwrap();

        let error = replace_all_impl(
            directory.path(),
            &[
                (absent.clone(), b"new absent".to_vec()),
                (existing.clone(), b"new existing".to_vec()),
                (failing.clone(), b"new failing".to_vec()),
            ],
            |_| Ok(()),
            |index| {
                if index == 2 {
                    bail!("injected failure");
                }
                Ok(())
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("rolled back"));
        assert!(!absent.exists());
        assert_eq!(fs::read(existing).unwrap(), b"old existing");
        assert_eq!(fs::read(failing).unwrap(), b"old failing");
        assert!(!manifest_path(directory.path()).exists());
    }

    #[test]
    fn backup_setup_failure_restores_originals_already_moved() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        fs::write(&first, b"old first").unwrap();
        fs::write(&second, b"old second").unwrap();

        replace_all_impl(
            directory.path(),
            &[
                (first.clone(), b"new first".to_vec()),
                (second.clone(), b"new second".to_vec()),
            ],
            |index| {
                if index == 1 {
                    bail!("injected backup failure");
                }
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap_err();

        assert_eq!(fs::read(first).unwrap(), b"old first");
        assert_eq!(fs::read(second).unwrap(), b"old second");
    }

    #[test]
    fn startup_recovery_rolls_back_an_interrupted_commit() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("artifact");
        let backup = directory.path().join(".trox-backup-test");
        let staged = directory.path().join(".trox-staged-test");
        fs::write(&destination, b"new").unwrap();
        fs::write(&backup, b"old").unwrap();
        fs::write(&staged, b"unused").unwrap();
        let manifest = TransactionManifest {
            version: 1,
            entries: vec![ManifestEntry {
                destination: destination.clone(),
                staged,
                backup,
                had_original: true,
            }],
        };
        let file = create_manifest(directory.path(), &manifest).unwrap();
        drop(file);

        recover_pending_transaction(directory.path(), std::slice::from_ref(&destination)).unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"old");
        assert!(!manifest_path(directory.path()).exists());
    }

    #[test]
    fn startup_recovery_finishes_committed_cleanup_without_rolling_back() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("artifact");
        let backup = directory.path().join(".trox-backup-test");
        let staged = directory.path().join(".trox-staged-test");
        fs::write(&destination, b"new").unwrap();
        fs::write(&backup, b"old").unwrap();
        fs::write(&staged, b"unused").unwrap();
        let manifest = TransactionManifest {
            version: 1,
            entries: vec![ManifestEntry {
                destination: destination.clone(),
                staged,
                backup,
                had_original: true,
            }],
        };
        let file = create_manifest(directory.path(), &manifest).unwrap();
        drop(file);
        create_commit_marker(directory.path()).unwrap();

        recover_pending_transaction(directory.path(), std::slice::from_ref(&destination)).unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"new");
        assert!(!manifest_path(directory.path()).exists());
    }

    #[test]
    fn panic_during_commit_is_recovered_on_the_next_startup() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        fs::write(&first, b"old first").unwrap();
        fs::write(&second, b"old second").unwrap();

        let result = std::panic::catch_unwind(|| {
            let _ = replace_all_impl(
                directory.path(),
                &[
                    (first.clone(), b"new first".to_vec()),
                    (second.clone(), b"new second".to_vec()),
                ],
                |_| Ok(()),
                |index| {
                    assert_ne!(index, 1, "injected panic");
                    Ok(())
                },
            );
        });
        assert!(result.is_err());
        recover_pending_transaction(directory.path(), &[first.clone(), second.clone()]).unwrap();

        assert_eq!(fs::read(first).unwrap(), b"old first");
        assert_eq!(fs::read(second).unwrap(), b"old second");
    }

    #[test]
    fn failed_startup_rollback_retains_manifest_and_backup() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("artifact");
        let backup = directory.path().join(".trox-backup-test");
        let staged = directory.path().join(".trox-staged-test");
        fs::create_dir(&destination).unwrap();
        fs::write(&backup, b"old").unwrap();
        let manifest = TransactionManifest {
            version: 1,
            entries: vec![ManifestEntry {
                destination: destination.clone(),
                staged,
                backup: backup.clone(),
                had_original: true,
            }],
        };
        let file = create_manifest(directory.path(), &manifest).unwrap();
        drop(file);

        let error =
            recover_pending_transaction(directory.path(), std::slice::from_ref(&destination))
                .unwrap_err()
                .to_string();

        assert!(error.contains("backups remain recorded"), "{error}");
        assert!(manifest_path(directory.path()).exists());
        assert!(backup.exists());
    }
}
