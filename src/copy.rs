use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use indicatif::ProgressBar;
use rayon::prelude::*;
use serde::Serialize;

use crate::cli::{Algorithm, Layout, SyncOptions};
use crate::compare::Comparison;
use crate::hash::hash_file;
use crate::naming::{flatten, validate_subdir};
use crate::scan::{ensure_unchanged, FileEntry};

#[derive(Debug, Serialize)]
pub struct CopyAction {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub size: u64,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub dry_run: bool,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub planned: usize,
    pub actions: Vec<CopyAction>,
}

pub fn sync(
    source: &[FileEntry],
    target: &Path,
    comparison: &Comparison,
    options: &SyncOptions,
    algorithm: Algorithm,
    progress: &ProgressBar,
) -> Result<SyncReport> {
    let subdir = options
        .dest_subdir
        .as_deref()
        .unwrap_or_else(|| Path::new(""));
    validate_subdir(subdir)?;
    let destination_root = target.join(subdir);
    let lookup: HashMap<_, _> = source
        .iter()
        .map(|file| (file.relative.as_path(), file))
        .collect();
    let mut reserved = HashSet::new();
    let mut planned = Vec::new();
    for group in &comparison.missing {
        let file = lookup[group.paths[0].as_path()];
        let destination = destination(
            &destination_root,
            &file.relative,
            options.layout,
            options.max_name_len,
            &mut reserved,
        )?;
        check_parents(
            target,
            destination.parent().context("destination without parent")?,
        )?;
        planned.push((file, destination));
    }
    progress.set_message(if options.execute {
        "Copying"
    } else {
        "Planning copies"
    });
    progress.set_position(0);
    progress.set_length(planned.len() as u64);
    let actions: Vec<_> = planned
        .par_iter()
        .map(|(file, destination)| {
            let result = if options.execute {
                copy_one(file, target, destination, algorithm, options.verify)
            } else {
                Ok(())
            };
            progress.inc(1);
            CopyAction {
                source: file.path.clone(),
                destination: destination.clone(),
                size: file.size,
                status: match (&result, options.execute) {
                    (Err(_), _) => "failed",
                    (Ok(_), true) => "copied",
                    (Ok(_), false) => "planned",
                }
                .into(),
                error: result.err().map(|e| format!("{e:#}")),
            }
        })
        .collect();
    let failed = actions
        .iter()
        .filter(|action| action.error.is_some())
        .count();
    let copied = if options.execute {
        actions.len() - failed
    } else {
        0
    };
    Ok(SyncReport {
        dry_run: !options.execute,
        planned: actions.len(),
        copied,
        skipped: comparison.source_files - comparison.missing.len(),
        failed,
        actions,
    })
}

pub(crate) fn destination(
    root: &Path,
    relative: &Path,
    layout: Layout,
    max_name_len: usize,
    reserved: &mut HashSet<String>,
) -> Result<PathBuf> {
    let mut path = if layout == Layout::Keep {
        root.join(relative)
    } else {
        root.join(flatten(relative, max_name_len, None))
    };
    if layout == Layout::Keep {
        anyhow::ensure!(
            !occupied(&path)?,
            "destination already exists (not overwritten): {}",
            path.display()
        );
    } else {
        let mut collision = 0u64;
        while occupied(&path)? || reserved.contains(&path_key(&path)) {
            let digest = blake3::hash(format!("{}:{collision}", relative.display()).as_bytes());
            let suffix = &digest.to_hex().to_string()[..10];
            path = root.join(flatten(relative, max_name_len, Some(suffix)));
            collision = collision
                .checked_add(1)
                .context("collision counter overflow")?;
        }
    }
    anyhow::ensure!(
        reserved.insert(path_key(&path)),
        "duplicate destination: {}",
        path.display()
    );
    Ok(path)
}

pub(crate) fn occupied(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("checking {}", path.display())),
    }
}

pub(crate) fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

pub(crate) fn check_parents(root: &Path, parent: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    let mut paths = vec![current.clone()];
    for component in parent.strip_prefix(root)?.components() {
        anyhow::ensure!(
            matches!(component, Component::Normal(_) | Component::CurDir),
            "unsafe destination path"
        );
        current.push(component);
        paths.push(current.clone());
    }
    for path in paths {
        match fs::symlink_metadata(&path) {
            Ok(metadata) => anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "destination parent is not a plain directory: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("checking {}", path.display()))
            }
        }
    }
    Ok(())
}

fn copy_one(
    file: &FileEntry,
    root: &Path,
    destination: &Path,
    algorithm: Algorithm,
    verify: bool,
) -> Result<()> {
    let temporary = stage_copy(file, root, destination)?;
    if verify {
        let expected = hash_file(&file.path, algorithm, None)?;
        let actual = hash_file(temporary.path(), algorithm, None)?;
        ensure_unchanged(file)?;
        anyhow::ensure!(
            expected == actual,
            "verification failed for {}",
            destination.display()
        );
    }
    publish(temporary, destination)
}

pub(crate) fn stage_copy(
    file: &FileEntry,
    root: &Path,
    destination: &Path,
) -> Result<tempfile::NamedTempFile> {
    ensure_unchanged(file)?;
    let parent = destination.parent().context("destination without parent")?;
    check_parents(root, parent)?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    check_parents(root, parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut source = fs::File::open(&file.path)?;
    let count = std::io::copy(&mut source, temporary.as_file_mut())?;
    anyhow::ensure!(count == file.size, "source size changed while copying");
    ensure_unchanged(file)?;
    filetime::set_file_mtime(
        temporary.path(),
        filetime::FileTime::from_system_time(file.modified),
    )?;
    Ok(temporary)
}

pub(crate) fn publish(temporary: tempfile::NamedTempFile, destination: &Path) -> Result<()> {
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.error)
        .with_context(|| format!("publishing without overwrite: {}", destination.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn fixture(root: &Path) -> FileEntry {
        let path = root.join("original");
        fs::write(&path, b"content").unwrap();
        let metadata = path.metadata().unwrap();
        FileEntry {
            path,
            relative: "original".into(),
            size: metadata.len(),
            modified: metadata.modified().unwrap(),
        }
    }

    #[test]
    fn publishing_never_overwrites_a_file_that_appears_after_planning() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let file = fixture(source.path());
        let destination = target.path().join("original");
        fs::write(&destination, b"existing").unwrap();
        assert!(copy_one(&file, target.path(), &destination, Algorithm::Blake3, true).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"existing");
        assert_eq!(fs::read_dir(target.path()).unwrap().count(), 1);
    }

    #[test]
    fn changed_sources_are_failed_actions_not_silent_skips() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let file = fixture(source.path());
        fs::write(&file.path, b"changed size").unwrap();
        let cli = Cli::parse_from(["yast", "sync", "source", "target", "--execute"]);
        let options = match cli.command {
            crate::cli::Command::Sync { options, .. } => options,
            _ => unreachable!(),
        };
        let comparison = Comparison {
            source_files: 1,
            missing: vec![crate::compare::ContentGroup {
                size: file.size,
                digest: None,
                paths: vec![file.relative.clone()],
            }],
            ..Comparison::default()
        };
        let result = sync(
            &[file],
            target.path(),
            &comparison,
            &options,
            cli.common.hash,
            &ProgressBar::hidden(),
        )
        .unwrap();
        assert_eq!(result.failed, 1);
        assert_eq!(result.copied, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.actions[0]
            .error
            .as_ref()
            .unwrap()
            .contains("file changed"));
        assert_eq!(fs::read_dir(target.path()).unwrap().count(), 0);
    }
}
