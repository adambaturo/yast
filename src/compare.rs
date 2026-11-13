use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use indicatif::ProgressBar;
use rayon::prelude::*;
use serde::Serialize;

use crate::cli::Common;
use crate::hash::{hash_file, PREFIX_BYTES};
use crate::scan::{ensure_unchanged, FileEntry};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Signature {
    Unique(usize),
    Full(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentGroup {
    pub size: u64,
    pub digest: Option<String>,
    pub paths: Vec<std::path::PathBuf>,
}

#[derive(Default, Debug, Serialize)]
pub struct Comparison {
    pub missing: Vec<ContentGroup>,
    pub extra: Vec<ContentGroup>,
    pub common: Vec<ContentGroup>,
    pub source_duplicates: Vec<ContentGroup>,
    pub target_duplicates: Vec<ContentGroup>,
    pub source_files: usize,
    pub target_files: usize,
    pub hashed_files: usize,
    pub partial_hashed_files: usize,
}

impl Comparison {
    pub fn different(&self) -> bool {
        !self.missing.is_empty()
            || !self.extra.is_empty()
            || !self.source_duplicates.is_empty()
            || !self.target_duplicates.is_empty()
    }
}

pub fn compare(
    source: &[FileEntry],
    target: &[FileEntry],
    options: &Common,
    progress: &ProgressBar,
) -> Result<Comparison> {
    let files: Vec<_> = source.iter().chain(target).collect();
    let mut sizes: HashMap<u64, usize> = HashMap::new();
    for file in &files {
        *sizes.entry(file.size).or_default() += 1;
    }
    let candidates: Vec<usize> = (0..files.len())
        .filter(|&i| options.hash_all || sizes[&files[i].size] > 1)
        .collect();
    progress.set_length(candidates.len() as u64);
    progress.set_message("Hashing prefixes");
    let prefixes: Vec<(usize, String)> = candidates
        .par_iter()
        .map(|&i| {
            ensure_unchanged(files[i])?;
            let digest = hash_file(&files[i].path, options.hash, Some(PREFIX_BYTES))?;
            ensure_unchanged(files[i])?;
            progress.inc(1);
            Ok((i, digest))
        })
        .collect::<Result<_>>()?;
    let mut buckets: HashMap<(u64, String), Vec<usize>> = HashMap::new();
    for (i, digest) in prefixes {
        buckets.entry((files[i].size, digest)).or_default().push(i);
    }
    let mut signatures: Vec<_> = (0..files.len()).map(Signature::Unique).collect();
    let mut full_candidates = Vec::new();
    let mut hashed_files = 0;
    for ((size, digest), indices) in buckets {
        if size <= PREFIX_BYTES {
            for i in indices {
                signatures[i] = Signature::Full(digest.clone());
                hashed_files += 1;
            }
        } else if options.hash_all || indices.len() > 1 {
            full_candidates.extend(indices);
        }
    }
    progress.set_position(0);
    progress.set_length(full_candidates.len() as u64);
    progress.set_message("Hashing full files");
    let full: Vec<_> = full_candidates
        .par_iter()
        .map(|&i| {
            ensure_unchanged(files[i])?;
            let digest = hash_file(&files[i].path, options.hash, None)?;
            ensure_unchanged(files[i])?;
            progress.inc(1);
            Ok((i, digest))
        })
        .collect::<Result<_>>()?;
    hashed_files += full.len();
    for (i, digest) in full {
        signatures[i] = Signature::Full(digest);
    }
    type Members = (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>);
    let mut groups: BTreeMap<(u64, Signature), Members> = BTreeMap::new();
    for (i, file) in files.iter().enumerate() {
        let members = groups
            .entry((file.size, signatures[i].clone()))
            .or_default();
        if i < source.len() {
            members.0.push(file.relative.clone());
        } else {
            members.1.push(file.relative.clone());
        }
    }
    let mut result = Comparison {
        source_files: source.len(),
        target_files: target.len(),
        hashed_files,
        partial_hashed_files: candidates.len(),
        ..Comparison::default()
    };
    for ((size, signature), (left, right)) in groups {
        let digest = match signature {
            Signature::Full(value) => Some(value),
            Signature::Unique(_) => None,
        };
        let group = |paths| ContentGroup {
            size,
            digest: digest.clone(),
            paths,
        };
        if left.len() > 1 {
            result.source_duplicates.push(group(left.clone()));
        }
        if right.len() > 1 {
            result.target_duplicates.push(group(right.clone()));
        }
        if right.is_empty() {
            result.missing.push(group(left));
        } else if left.is_empty() {
            result.extra.push(group(right));
        } else {
            result.common.push(group(left));
        }
    }
    for groups in [
        &mut result.missing,
        &mut result.extra,
        &mut result.common,
        &mut result.source_duplicates,
        &mut result.target_duplicates,
    ] {
        groups.sort_by(|a, b| a.paths[0].cmp(&b.paths[0]));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;
    use std::fs;

    fn entry(root: &std::path::Path, name: &str, content: &[u8]) -> FileEntry {
        let path = root.join(name);
        fs::write(&path, content).unwrap();
        let metadata = path.metadata().unwrap();
        FileEntry {
            path,
            relative: name.into(),
            size: metadata.len(),
            modified: metadata.modified().unwrap(),
        }
    }

    #[test]
    fn matches_content_not_names_and_detects_both_sides_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let source = vec![
            entry(root.path(), "a", b"match"),
            entry(root.path(), "b", b"match"),
            entry(root.path(), "c", b"missing"),
        ];
        let target = vec![
            entry(root.path(), "renamed", b"match"),
            entry(root.path(), "extra1", b"extra"),
            entry(root.path(), "extra2", b"extra"),
        ];
        let options = Cli::parse_from(["yast", "dupes", "."]).common;
        let result = compare(&source, &target, &options, &ProgressBar::hidden()).unwrap();
        assert_eq!(
            result.common[0].paths,
            [std::path::PathBuf::from("a"), "b".into()]
        );
        assert_eq!(result.missing[0].paths, [std::path::PathBuf::from("c")]);
        assert_eq!(result.extra[0].paths.len(), 2);
        assert_eq!(result.source_duplicates.len(), 1);
        assert_eq!(result.target_duplicates.len(), 1);
        assert!(result.missing[0].digest.is_none());
    }

    #[test]
    fn distinct_prefixes_prune_large_files_and_hash_all_bypasses_pruning() {
        let root = tempfile::tempdir().unwrap();
        let source = vec![entry(root.path(), "a", &vec![b'a'; 70_000])];
        let target = vec![entry(root.path(), "b", &vec![b'b'; 70_000])];
        let mut options = Cli::parse_from(["yast", "dupes", "."]).common;
        let result = compare(&source, &target, &options, &ProgressBar::hidden()).unwrap();
        assert_eq!(result.partial_hashed_files, 2);
        assert_eq!(result.hashed_files, 0);
        assert_eq!(result.missing.len(), 1);
        assert_eq!(result.extra.len(), 1);
        options.hash_all = true;
        let full = compare(&source, &target, &options, &ProgressBar::hidden()).unwrap();
        assert_eq!(full.hashed_files, 2);
        assert!(full.missing[0].digest.is_some());
    }
}
