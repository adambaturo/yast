use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indicatif::ProgressBar;
use serde::Serialize;

use crate::cli::{DupesAction, DupesOptions, Layout};
use crate::compare::Comparison;
use crate::copy::{check_parents, destination, occupied, publish, stage_copy};
use crate::scan::{ensure_unchanged, FileEntry};

#[derive(Debug, Serialize)]
pub struct DedupeAction {
    pub keeper: PathBuf,
    pub path: PathBuf,
    pub action: &'static str,
    pub destination: Option<PathBuf>,
    pub size: u64,
    pub status: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct DedupeReport {
    pub dry_run: bool,
    pub planned: usize,
    pub done: usize,
    pub failed: usize,
    pub bytes_planned: u64,
    pub bytes_reclaimed: u64,
    pub actions: Vec<DedupeAction>,
}

struct PlannedAction<'a> {
    keeper: &'a FileEntry,
    file: &'a FileEntry,
    action: DedupeAction,
}

pub fn dedupe(
    root: &Path,
    files: &[FileEntry],
    comparison: &Comparison,
    options: &DupesOptions,
    target: Option<&Path>,
    progress: &ProgressBar,
) -> Result<DedupeReport> {
    let planned = plan(files, comparison, options, target)?;
    progress.set_message(if options.execute {
        "Acting on duplicates"
    } else {
        "Planning duplicate actions"
    });
    progress.set_position(0);
    progress.set_length(planned.len() as u64);
    let mut report = DedupeReport {
        dry_run: !options.execute,
        planned: planned.len(),
        ..DedupeReport::default()
    };
    for item in planned {
        let action = act(root, item, options.execute, target);
        progress.inc(1);
        report.bytes_planned += action.size;
        match action.status {
            "done" => {
                report.done += 1;
                // Trash and relocation retain the bytes; only permanent deletion reclaims them.
                if action.action == "delete" {
                    report.bytes_reclaimed += action.size;
                }
            }
            "failed" => report.failed += 1,
            _ => {}
        }
        report.actions.push(action);
    }
    Ok(report)
}

fn plan<'a>(
    files: &'a [FileEntry],
    comparison: &Comparison,
    options: &DupesOptions,
    target: Option<&Path>,
) -> Result<Vec<PlannedAction<'a>>> {
    let lookup: HashMap<_, _> = files
        .iter()
        .map(|file| (file.relative.as_path(), file))
        .collect();
    let mut reserved = HashSet::new();
    let mut planned = Vec::new();
    for group in &comparison.source_duplicates {
        let keeper = lookup
            .get(group.paths[0].as_path())
            .context("keeper missing from scan")?;
        for path in &group.paths[1..] {
            let file = lookup
                .get(path.as_path())
                .context("duplicate missing from scan")?;
            let mut action = DedupeAction {
                keeper: keeper.relative.clone(),
                path: file.relative.clone(),
                action: match options.action {
                    DupesAction::Delete if options.permanent => "delete",
                    DupesAction::Delete => "trash",
                    DupesAction::Move => "move",
                    DupesAction::Report => anyhow::bail!("report does not plan actions"),
                },
                destination: None,
                size: file.size,
                status: "planned",
                error: None,
            };
            if options.action == DupesAction::Move {
                let target = target.context("missing move destination")?;
                if options.layout == Layout::Keep {
                    action.destination = Some(target.join(&file.relative));
                }
                let result = (|| -> Result<()> {
                    let path = destination(
                        target,
                        &file.relative,
                        options.layout,
                        options.max_name_len,
                        &mut reserved,
                    )?;
                    action.destination = Some(path.clone());
                    check_parents(target, path.parent().context("destination without parent")?)
                })();
                if let Err(error) = result {
                    action.status = "failed";
                    action.error = Some(format!("{error:#}"));
                }
            }
            planned.push(PlannedAction {
                keeper,
                file,
                action,
            });
        }
    }
    Ok(planned)
}

fn act(
    root: &Path,
    mut planned: PlannedAction<'_>,
    execute: bool,
    target: Option<&Path>,
) -> DedupeAction {
    if planned.action.error.is_some() || !execute {
        return planned.action;
    }
    let result = (|| -> Result<()> {
        match planned.action.action {
            "move" => {
                let destination = planned
                    .action
                    .destination
                    .as_deref()
                    .context("missing destination")?;
                let target = target.context("missing move root")?;
                let parent = destination.parent().context("destination without parent")?;
                check_parents(target, parent)?;
                anyhow::ensure!(
                    !occupied(destination)?,
                    "destination already exists (not overwritten): {}",
                    destination.display()
                );
                fs::create_dir_all(parent)?;
                check_parents(target, parent)?;
                verify_pair(root, planned.file, planned.keeper)?;
                match rename_noclobber(&planned.file.path, destination) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                        copy_then_remove(root, planned.file, planned.keeper, target, destination)
                    }
                    Err(error) => Err(error).with_context(|| {
                        format!("moving without overwrite to {}", destination.display())
                    }),
                }
            }
            action => {
                verify_pair(root, planned.file, planned.keeper)?;
                if action == "trash" {
                    trash::delete(&planned.file.path).context("sending duplicate to OS trash")
                } else {
                    fs::remove_file(&planned.file.path).context("permanently deleting duplicate")
                }
            }
        }
    })();
    planned.action.status = if result.is_ok() { "done" } else { "failed" };
    planned.action.error = result.err().map(|error| format!("{error:#}"));
    planned.action
}

fn verify_pair(root: &Path, file: &FileEntry, keeper: &FileEntry) -> Result<()> {
    for entry in [file, keeper] {
        check_parents(root, entry.path.parent().context("file without parent")?)?;
        let metadata = fs::symlink_metadata(&entry.path)?;
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "not a plain file: {}",
            entry.path.display()
        );
        ensure_unchanged(entry)?;
    }
    anyhow::ensure!(
        equal_bytes(&file.path, &keeper.path)?,
        "duplicate no longer matches keeper: {}",
        file.path.display()
    );
    ensure_unchanged(file)?;
    ensure_unchanged(keeper)?;
    Ok(())
}

fn equal_bytes(left: &Path, right: &Path) -> Result<bool> {
    let mut left = BufReader::new(fs::File::open(left)?);
    let mut right = BufReader::new(fs::File::open(right)?);
    loop {
        let a = left.fill_buf()?;
        let b = right.fill_buf()?;
        if a.is_empty() || b.is_empty() {
            return Ok(a.is_empty() && b.is_empty());
        }
        let count = a.len().min(b.len());
        if a[..count] != b[..count] {
            return Ok(false);
        }
        left.consume(count);
        right.consume(count);
    }
}

fn copy_then_remove(
    root: &Path,
    file: &FileEntry,
    keeper: &FileEntry,
    target: &Path,
    destination: &Path,
) -> Result<()> {
    let temporary = stage_copy(file, target, destination)?;
    anyhow::ensure!(
        equal_bytes(&file.path, temporary.path())?,
        "staged move verification failed"
    );
    verify_pair(root, file, keeper)?;
    check_parents(
        target,
        destination.parent().context("destination without parent")?,
    )?;
    publish(temporary, destination)?;
    // A failed recheck/removal leaves the published copy and source intact, and is reported.
    verify_pair(root, file, keeper).with_context(|| {
        format!(
            "destination published at {}; source retained",
            destination.display()
        )
    })?;
    fs::remove_file(&file.path).with_context(|| {
        format!(
            "destination published at {}; removing source",
            destination.display()
        )
    })
}

#[cfg(windows)]
fn rename_noclobber(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let wide_path = |path: &Path| -> io::Result<Vec<u16>> {
        let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    };
    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    // Unlike fs::rename, flags=0 forbids replacement and cross-device copying.
    // SAFETY: Both pointers refer to live, NUL-terminated UTF-16 buffers.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_noclobber(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn rename_noclobber(_source: &Path, _destination: &Path) -> io::Result<()> {
    // Platforms without an atomic no-replace rename use the verified no-clobber copy path.
    Err(io::Error::from(io::ErrorKind::CrossesDevices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use crate::compare::ContentGroup;
    use clap::Parser;

    fn fixture() -> (tempfile::TempDir, Vec<FileEntry>, Comparison, DupesOptions) {
        let root = tempfile::tempdir().unwrap();
        let files: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let path = root.path().join(name);
                fs::write(&path, b"content").unwrap();
                let metadata = path.metadata().unwrap();
                FileEntry {
                    path,
                    relative: name.into(),
                    size: metadata.len(),
                    modified: metadata.modified().unwrap(),
                }
            })
            .collect();
        let comparison = Comparison {
            source_duplicates: vec![ContentGroup {
                size: 7,
                digest: None,
                paths: vec!["a".into(), "b".into()],
            }],
            ..Comparison::default()
        };
        let Command::Dupes { options, .. } = Cli::parse_from([
            "yast",
            "dupes",
            ".",
            "--action",
            "delete",
            "--permanent",
            "--execute",
        ])
        .command
        else {
            unreachable!()
        };
        (root, files, comparison, options)
    }

    #[test]
    fn changed_duplicate_or_keeper_is_failed_and_untouched() {
        for index in [0, 1] {
            let (root, files, comparison, options) = fixture();
            let planned = plan(&files, &comparison, &options, None).unwrap();
            fs::write(&files[index].path, b"changed content").unwrap();
            let action = act(root.path(), planned.into_iter().next().unwrap(), true, None);
            assert_eq!(action.status, "failed");
            assert!(action.error.unwrap().contains("file changed"));
            assert!(files.iter().all(|file| file.path.is_file()));
            assert_eq!(fs::read(&files[index].path).unwrap(), b"changed content");
        }
    }

    #[test]
    fn same_metadata_mismatch_is_not_deleted() {
        let (root, files, comparison, options) = fixture();
        let planned = plan(&files, &comparison, &options, None).unwrap();
        // Retain both size and mtime so only the byte comparison can detect the change.
        fs::write(&files[1].path, b"changed").unwrap();
        filetime::set_file_mtime(
            &files[1].path,
            filetime::FileTime::from_system_time(files[1].modified),
        )
        .unwrap();
        let action = act(root.path(), planned.into_iter().next().unwrap(), true, None);
        assert_eq!(action.status, "failed");
        assert!(action.error.unwrap().contains("no longer matches"));
        assert_eq!(fs::read(&files[1].path).unwrap(), b"changed");
    }

    #[test]
    fn streaming_comparison_detects_mismatches_after_the_first_buffer() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        let mut bytes = vec![b'a'; 70_000];
        fs::write(&left, &bytes).unwrap();
        fs::write(&right, &bytes).unwrap();
        assert!(equal_bytes(&left, &right).unwrap());
        bytes[69_999] = b'b';
        fs::write(&right, &bytes).unwrap();
        assert!(!equal_bytes(&left, &right).unwrap());
        fs::write(&right, &bytes[..69_999]).unwrap();
        assert!(!equal_bytes(&left, &right).unwrap());
        fs::write(&left, []).unwrap();
        fs::write(&right, []).unwrap();
        assert!(equal_bytes(&left, &right).unwrap());
    }

    #[test]
    fn no_clobber_rename_rejects_a_destination_created_after_planning() {
        let (root, files, _, _) = fixture();
        let destination = root.path().join("existing");
        fs::write(&destination, b"existing").unwrap();
        assert!(rename_noclobber(&files[1].path, &destination).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"existing");
        assert_eq!(fs::read(&files[1].path).unwrap(), b"content");
    }

    #[test]
    fn cross_device_copy_verifies_preserves_mtime_and_never_overwrites() {
        let (root, files, _, _) = fixture();
        let target = tempfile::tempdir().unwrap();
        let destination = target.path().join("b");
        fs::write(&destination, b"existing").unwrap();
        assert!(copy_then_remove(
            root.path(),
            &files[1],
            &files[0],
            target.path(),
            &destination
        )
        .is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"existing");
        assert_eq!(fs::read(&files[1].path).unwrap(), b"content");
        assert_eq!(fs::read_dir(target.path()).unwrap().count(), 1);
        fs::remove_file(&destination).unwrap();
        copy_then_remove(
            root.path(),
            &files[1],
            &files[0],
            target.path(),
            &destination,
        )
        .unwrap();
        assert!(!files[1].path.exists());
        assert!(files[0].path.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"content");
        assert_eq!(
            destination.metadata().unwrap().modified().unwrap(),
            files[1].modified
        );
    }
}
