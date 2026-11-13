use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Yet Another Sync Tool: compare and copy files by content, not path",
    long_about = "Compare directory trees by file content regardless of filenames or paths. \
        Detect duplicates and safely copy missing content. Sync is a dry run unless --execute is supplied.",
    after_help = "EXAMPLES:\n  yast compare photos backup --format json\n  yast dupes photos --min-size 1MiB\n  yast sync photos backup --layout flatten\n  yast sync photos backup --execute --verify\n\n\
        Exit codes: 0 = identical/success, 1 = differences/duplicates, 2 = error."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    #[command(flatten)]
    pub common: Common,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Report missing/extra content and duplicates; never modify either tree.
    #[command(
        after_help = "EXAMPLES:\n  yast compare photos backup\n  yast compare photos backup --format json --output comparison.json\n  yast compare photos backup --hash sha256 --threads 8"
    )]
    Compare { source: PathBuf, target: PathBuf },
    /// Find duplicate content within one directory.
    #[command(
        after_help = "EXAMPLES:\n  yast dupes downloads\n  yast dupes downloads --min-size 10MiB --skip-hidden\n  yast dupes downloads --exclude '*.part' --format csv"
    )]
    Dupes { dir: PathBuf },
    /// Copy missing content, once per content group. Dry run unless --execute.
    #[command(
        after_help = "EXAMPLES:\n  yast sync photos backup\n  yast sync photos backup --execute --verify\n  yast sync photos backup --layout flatten --dest-subdir incoming --execute\n\nExisting destination files are never overwritten. Extras are never deleted."
    )]
    Sync {
        source: PathBuf,
        target: PathBuf,
        #[command(flatten)]
        options: SyncOptions,
    },
}

#[derive(Args, Debug)]
pub struct Common {
    #[arg(long, global = true, value_enum, default_value = "blake3")]
    pub hash: Algorithm,
    /// Worker count for walking, hashing and copying (default: logical CPUs).
    #[arg(long, global = true, value_parser = positive_threads)]
    pub threads: Option<usize>,
    #[arg(long, global = true, value_enum, default_value = "text")]
    pub format: Format,
    /// Write the report to this file instead of stdout.
    #[arg(long, global = true)]
    pub output: Option<PathBuf>,
    /// Exclude relative paths matching this glob; may be repeated.
    #[arg(long, global = true)]
    pub exclude: Vec<String>,
    /// Exclude dotfiles, dot-directories and Windows hidden entries.
    #[arg(long, global = true)]
    pub skip_hidden: bool,
    /// Follow file and directory symlinks; traversal loops are errors.
    #[arg(long, global = true)]
    pub follow_symlinks: bool,
    /// Minimum file size, in bytes or with KiB/MiB/GiB (or KB/MB/GB).
    #[arg(long, global = true, default_value = "0", value_parser = parse_size)]
    pub min_size: u64,
    /// Fully hash every file, including unique-size/unique-prefix files.
    #[arg(long, global = true)]
    pub hash_all: bool,
    /// Disable progress (reports are still emitted).
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Emit diagnostic counters on stderr.
    #[arg(short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Args, Debug)]
pub struct SyncOptions {
    /// Actually copy files. Without this flag, only print the plan.
    #[arg(long)]
    pub execute: bool,
    #[arg(long, value_enum, default_value = "keep")]
    pub layout: Layout,
    /// Relative destination subdirectory inside target.
    #[arg(long)]
    pub dest_subdir: Option<PathBuf>,
    /// Maximum flattened filename length, in Unicode characters (minimum 16).
    #[arg(long, default_value = "64", value_parser = name_length)]
    pub max_name_len: usize,
    /// Re-hash staged copies before publishing them.
    #[arg(long)]
    pub verify: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Algorithm {
    Blake3,
    Xxh3,
    Crc32,
    Md5,
    Sha1,
    Sha256,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Format {
    Text,
    Json,
    Csv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Layout {
    Keep,
    Flatten,
}

fn parse_size(value: &str) -> Result<u64, String> {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        _ => return Err(format!("invalid size unit: {unit}")),
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .ok_or_else(|| format!("invalid or overflowing size: {value}"))
}

fn positive_threads(value: &str) -> Result<usize, String> {
    value
        .parse()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| "expected a positive worker count".into())
}

fn name_length(value: &str) -> Result<usize, String> {
    value
        .parse()
        .ok()
        .filter(|&n| n >= 16)
        .ok_or_else(|| "expected at least 16 characters".into())
}
