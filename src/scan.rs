use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use globset::{Glob, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};

use crate::cli::Common;
use crate::progress::Progress;

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

pub fn scan(
    root: &Path,
    options: &Common,
    threads: usize,
    progress: &Progress,
) -> Result<Vec<FileEntry>> {
    anyhow::ensure!(root.is_dir(), "not a directory: {}", root.display());
    let mut globs = GlobSetBuilder::new();
    for pattern in &options.exclude {
        globs.add(Glob::new(pattern).with_context(|| format!("invalid glob: {pattern}"))?);
    }
    let globs = globs.build()?;
    let mut includes = GlobSetBuilder::new();
    for pattern in &options.include {
        includes.add(Glob::new(pattern).with_context(|| format!("invalid glob: {pattern}"))?);
    }
    let includes = includes.build()?;
    let files = Mutex::new(Vec::new());
    let errors = Mutex::new(Vec::new());
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .follow_links(options.follow_symlinks)
        .threads(threads);
    let filter_root = root.to_path_buf();
    let skip_hidden = options.skip_hidden;
    walker.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        let relative = entry
            .path()
            .strip_prefix(&filter_root)
            .unwrap_or(entry.path());
        !globs.is_match(relative) && !(skip_hidden && hidden(entry.path()))
    });
    walker.build_parallel().run(|| {
        Box::new(|result| {
            let result = (|| -> Result<()> {
                let entry = result?;
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return Ok(());
                }
                let relative = entry.path().strip_prefix(root)?;
                if !options.include.is_empty() && !includes.is_match(relative) {
                    return Ok(());
                }
                let metadata = entry.metadata()?;
                if metadata.len() < options.min_size {
                    return Ok(());
                }
                files.lock().unwrap().push(FileEntry {
                    path: entry.path().to_path_buf(),
                    relative: relative.to_path_buf(),
                    size: metadata.len(),
                    modified: metadata.modified()?,
                });
                progress.found_file();
                Ok(())
            })();
            if let Err(error) = result {
                errors.lock().unwrap().push(error.to_string());
            }
            WalkState::Continue
        })
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(anyhow!("scan {}: {}", root.display(), errors.join("; ")));
    }
    let mut files = files.into_inner().unwrap();
    files.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
    Ok(files)
}

fn hidden(path: &Path) -> bool {
    let dot_hidden = path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'));
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        dot_hidden
            || path
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_attributes() & 2 != 0)
    }
    #[cfg(not(windows))]
    dot_hidden
}

pub fn ensure_unchanged(entry: &FileEntry) -> Result<()> {
    let metadata = std::fs::metadata(&entry.path)
        .with_context(|| format!("reading metadata for {}", entry.path.display()))?;
    anyhow::ensure!(
        metadata.len() == entry.size && metadata.modified()? == entry.modified,
        "file changed during operation: {}",
        entry.path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn includes_match_nested_files_as_a_union_but_excludes_win() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("nested/deeper")).unwrap();
        std::fs::create_dir_all(root.path().join("cache")).unwrap();
        for path in [
            "a.jpg",
            "nested/deeper/b.png",
            "nested/deeper/c.txt",
            "skip.jpg",
            "cache/d.png",
        ] {
            std::fs::write(root.path().join(path), b"content").unwrap();
        }
        let options = Cli::parse_from([
            "yast",
            "dupes",
            ".",
            "--include",
            "*.jpg",
            "--include",
            "*.png",
            "--exclude",
            "skip.jpg",
            "--exclude",
            "cache",
        ])
        .common;
        let progress = Progress::hidden();
        let files = scan(root.path(), &options, 2, &progress).unwrap();
        let paths: Vec<_> = files.iter().map(|file| file.relative.clone()).collect();
        assert_eq!(
            paths,
            [PathBuf::from("a.jpg"), PathBuf::from("nested/deeper/b.png")]
        );
        assert_eq!(progress.files.position(), 2);

        for (pattern, expected) in [("nested", 0), ("nested/**", 2), ("absent/**", 0)] {
            let options = Cli::parse_from(["yast", "dupes", ".", "--include", pattern]).common;
            assert_eq!(
                scan(root.path(), &options, 2, &Progress::hidden())
                    .unwrap()
                    .len(),
                expected,
                "{pattern}"
            );
        }
    }

    #[test]
    fn discovery_counts_only_included_files_and_restarts_for_target() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("included"), b"content").unwrap();
        std::fs::write(root.path().join("excluded.tmp"), b"content").unwrap();
        std::fs::write(root.path().join(".hidden"), b"content").unwrap();
        std::fs::write(root.path().join("small"), b"x").unwrap();
        let options = Cli::parse_from([
            "yast",
            "dupes",
            ".",
            "--include",
            "*",
            "--exclude",
            "*.tmp",
            "--skip-hidden",
            "--min-size",
            "2",
        ])
        .common;
        let progress = Progress::hidden();
        progress.scanning("Scanning source");
        let files = scan(root.path(), &options, 2, &progress).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(progress.files.position(), 1);
        progress.inventory(files.len(), None);
        // A second traversal must not accumulate the first directory's count.
        progress.scanning("Scanning target");
        let target = tempfile::tempdir().unwrap();
        assert!(scan(target.path(), &options, 2, &progress)
            .unwrap()
            .is_empty());
        assert_eq!(progress.files.position(), 0);
    }
}
