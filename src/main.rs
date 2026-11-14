mod cli;
mod compare;
mod copy;
mod dedupe;
mod hash;
mod naming;
mod report;
mod scan;

use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};

use cli::{Cli, Command, DupesAction};
use report::Report;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    if let Command::Dupes { options, .. } = &cli.command {
        options.validate()?;
    }
    let threads = cli.common.threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    });
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    let progress = if cli.common.quiet || !io::stderr().is_terminal() {
        ProgressBar::hidden()
    } else {
        let bar = ProgressBar::new_spinner();
        bar.set_style(ProgressStyle::with_template("{spinner} {msg} {pos}/{len}")?);
        bar.enable_steady_tick(std::time::Duration::from_millis(100));
        bar
    };
    progress.set_message("Scanning");
    let result = pool.install(|| -> Result<Report> {
        let (source, target, command) = match &cli.command {
            Command::Compare { source, target } => (source, Some(target), "compare"),
            Command::Dupes { dir, .. } => (dir, None, "dupes"),
            Command::Sync { source, target, .. } => (source, Some(target), "sync"),
        };
        let source_root = source
            .canonicalize()
            .with_context(|| format!("resolving {}", source.display()))?;
        let target_root = target.map(|path| resolve(path)).transpose()?;
        let move_root = if let Command::Dupes { options, .. } = &cli.command {
            options.to.as_deref().map(resolve).transpose()?
        } else {
            None
        };
        if let Some(target) = &move_root {
            anyhow::ensure!(
                !target.starts_with(&source_root) && !source_root.starts_with(target),
                "move requires non-overlapping DIR and --to directories"
            );
        }
        if command == "sync" {
            let target = target_root.as_ref().context("missing target")?;
            anyhow::ensure!(
                !target.starts_with(&source_root) && !source_root.starts_with(target),
                "sync requires non-overlapping source and target directories"
            );
        }
        let source_files = scan::scan(&source_root, &cli.common, threads)?;
        let target_files = match &target_root {
            Some(root) if root.exists() || command != "sync" => {
                scan::scan(root, &cli.common, threads)?
            }
            _ => Vec::new(),
        };
        let mut comparison =
            compare::compare(&source_files, &target_files, &cli.common, &progress)?;
        if command == "dupes" {
            comparison.missing.clear();
        }
        let sync = if let Command::Sync { options, .. } = &cli.command {
            Some(copy::sync(
                &source_files,
                target_root.as_ref().context("missing target")?,
                &comparison,
                options,
                cli.common.hash,
                &progress,
            )?)
        } else {
            None
        };
        let dedupe = if let Command::Dupes { options, .. } = &cli.command {
            if options.action == DupesAction::Report {
                None
            } else {
                Some(dedupe::dedupe(
                    &source_root,
                    &source_files,
                    &comparison,
                    options,
                    move_root.as_deref(),
                    &progress,
                )?)
            }
        } else {
            None
        };
        Ok(Report {
            command,
            comparison,
            sync,
            dedupe,
        })
    });
    progress.finish_and_clear();
    let report = result?;
    if cli.common.verbose > 0 && !cli.common.quiet {
        eprintln!(
            "{} workers; {} full hashes; {} prefix hashes",
            threads, report.comparison.hashed_files, report.comparison.partial_hashed_files
        );
    }
    if let Some(path) = &cli.common.output {
        // Never allow reporting to overwrite an existing file or scanned content.
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("creating report {}", path.display()))?;
        report::write(&report, cli.common.format, &mut io::BufWriter::new(file))?;
    } else {
        report::write(&report, cli.common.format, &mut io::stdout().lock())?;
    }
    Ok(if let Some(sync) = &report.sync {
        if sync.failed > 0 {
            2
        } else {
            0
        }
    } else if let Some(dedupe) = &report.dedupe {
        if dedupe.failed > 0 {
            2
        } else {
            0
        }
    } else if report.comparison.different() {
        1
    } else {
        0
    })
}

fn resolve(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()));
    }
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            value => normalized.push(value.as_os_str()),
        }
    }
    let mut ancestor = normalized.as_path();
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .context("cannot resolve destination ancestor")?;
    }
    Ok(ancestor
        .canonicalize()?
        .join(normalized.strip_prefix(ancestor)?))
}
