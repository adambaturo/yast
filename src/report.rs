use std::io::Write;

use anyhow::Result;
use serde::Serialize;

use crate::cli::Format;
use crate::compare::{Comparison, ContentGroup};
use crate::copy::SyncReport;
use crate::dedupe::DedupeReport;

#[derive(Serialize)]
pub struct Report {
    pub command: &'static str,
    pub comparison: Comparison,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedupe: Option<DedupeReport>,
}

pub fn write(report: &Report, format: Format, output: &mut dyn Write) -> Result<()> {
    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut *output, report)?;
            writeln!(output)?;
        }
        Format::Text => text(report, output)?,
        Format::Csv => csv(report, output)?,
    }
    output.flush()?;
    Ok(())
}

fn text_actions(report: &Report, output: &mut dyn Write) -> Result<()> {
    if let Some(dedupe) = &report.dedupe {
        if dedupe.dry_run {
            writeln!(
                output,
                "DRY RUN: no files will be deleted or moved (use --execute to act)"
            )?;
        }
        writeln!(output, "Actions:")?;
        for action in &dedupe.actions {
            writeln!(
                output,
                "{}: {} {} ({} bytes; keeper: {}; destination: {})",
                action.status,
                action.action,
                action.path.display(),
                action.size,
                action.keeper.display(),
                action
                    .destination
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "-".into()),
            )?;
            if let Some(error) = &action.error {
                writeln!(output, "  ERROR: {error}")?;
            }
        }
        writeln!(
            output,
            "Planned: {}; done: {}; failed: {}; bytes planned: {}; bytes reclaimed: {}",
            dedupe.planned,
            dedupe.done,
            dedupe.failed,
            dedupe.bytes_planned,
            dedupe.bytes_reclaimed
        )?;
    }
    Ok(())
}

fn categories(report: &Report) -> Vec<(&'static str, &[ContentGroup])> {
    if report.command == "dupes" {
        return vec![("source_duplicate", &report.comparison.source_duplicates)];
    }
    vec![
        ("missing", &report.comparison.missing),
        ("extra", &report.comparison.extra),
        ("common", &report.comparison.common),
        ("source_duplicate", &report.comparison.source_duplicates),
        ("target_duplicate", &report.comparison.target_duplicates),
    ]
}

fn text(report: &Report, output: &mut dyn Write) -> Result<()> {
    writeln!(
        output,
        "Source: {} files; target: {} files",
        report.comparison.source_files, report.comparison.target_files
    )?;
    for (category, groups) in categories(report) {
        writeln!(output, "{category}: {} content groups", groups.len())?;
        if category != "common" {
            for group in groups {
                writeln!(
                    output,
                    "  {} bytes, {} file(s)",
                    group.size,
                    group.paths.len()
                )?;
                for path in &group.paths {
                    writeln!(output, "    {}", path.display())?;
                }
            }
        }
    }
    if let Some(sync) = &report.sync {
        if sync.dry_run {
            writeln!(
                output,
                "DRY RUN: no files will be written (use --execute to copy)"
            )?;
        }
        for action in &sync.actions {
            writeln!(
                output,
                "{}: {} -> {}",
                action.status,
                action.source.display(),
                action.destination.display()
            )?;
            if let Some(error) = &action.error {
                writeln!(output, "  ERROR: {error}")?;
            }
        }
        writeln!(
            output,
            "Planned: {}; copied: {}; skipped: {}; failed: {}",
            sync.planned, sync.copied, sync.skipped, sync.failed
        )?;
    }
    text_actions(report, output)?;
    Ok(())
}

fn csv(report: &Report, output: &mut dyn Write) -> Result<()> {
    let mut writer = csv::Writer::from_writer(output);
    let mut header = vec![
        "category",
        "group",
        "size",
        "digest",
        "path",
        "destination",
        "status",
        "error",
    ];
    if report.dedupe.is_some() {
        header.extend(["keeper", "action"]);
    }
    writer.write_record(header)?;
    for (category, groups) in categories(report) {
        for (index, group) in groups.iter().enumerate() {
            for path in &group.paths {
                let mut row = vec![
                    category.to_owned(),
                    (index + 1).to_string(),
                    group.size.to_string(),
                    group.digest.clone().unwrap_or_default(),
                    path.to_string_lossy().into_owned(),
                    String::new(),
                    String::new(),
                    String::new(),
                ];
                if report.dedupe.is_some() {
                    row.extend([String::new(), String::new()]);
                }
                writer.write_record(row)?;
            }
        }
    }
    if let Some(sync) = &report.sync {
        for action in &sync.actions {
            writer.write_record([
                "copy".to_owned(),
                String::new(),
                action.size.to_string(),
                String::new(),
                action.source.to_string_lossy().into_owned(),
                action.destination.to_string_lossy().into_owned(),
                action.status.clone(),
                action.error.clone().unwrap_or_default(),
            ])?;
        }
        writer.write_record([
            "summary".to_owned(),
            String::new(),
            String::new(),
            String::new(),
            format!(
                "planned={};copied={};skipped={};failed={}",
                sync.planned, sync.copied, sync.skipped, sync.failed
            ),
            String::new(),
            if sync.dry_run { "dry_run" } else { "executed" }.into(),
            String::new(),
        ])?;
    }
    if let Some(dedupe) = &report.dedupe {
        for action in &dedupe.actions {
            writer.write_record([
                "action".to_owned(),
                String::new(),
                action.size.to_string(),
                String::new(),
                action.path.to_string_lossy().into_owned(),
                action
                    .destination
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                action.status.to_owned(),
                action.error.clone().unwrap_or_default(),
                action.keeper.to_string_lossy().into_owned(),
                action.action.to_owned(),
            ])?;
        }
        writer.write_record([
            "summary".to_owned(),
            String::new(),
            String::new(),
            String::new(),
            format!(
                "planned={};done={};failed={};bytes_planned={};bytes_reclaimed={}",
                dedupe.planned,
                dedupe.done,
                dedupe.failed,
                dedupe.bytes_planned,
                dedupe.bytes_reclaimed
            ),
            String::new(),
            if dedupe.dry_run {
                "dry_run"
            } else {
                "executed"
            }
            .into(),
            String::new(),
            String::new(),
            String::new(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}
