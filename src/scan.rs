use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use globset::{Glob, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};

use crate::cli::Common;

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

pub fn scan(root: &Path, options: &Common, threads: usize) -> Result<Vec<FileEntry>> {
    anyhow::ensure!(root.is_dir(), "not a directory: {}", root.display());
    let mut globs = GlobSetBuilder::new();
    for pattern in &options.exclude {
        globs.add(Glob::new(pattern).with_context(|| format!("invalid glob: {pattern}"))?);
    }
    let globs = globs.build()?;
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
                let metadata = entry.metadata()?;
                if metadata.len() < options.min_size {
                    return Ok(());
                }
                files.lock().unwrap().push(FileEntry {
                    path: entry.path().to_path_buf(),
                    relative: entry.path().strip_prefix(root)?.to_path_buf(),
                    size: metadata.len(),
                    modified: metadata.modified()?,
                });
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
    if path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(metadata) = path.symlink_metadata() {
            return metadata.file_attributes() & 2 != 0;
        }
    }
    false
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
