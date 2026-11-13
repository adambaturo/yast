# yast - Yet Another Sync Tool

A parallel, content-based directory comparison and safe copy CLI, written in Rust.
Paths do not determine equality: a photo renamed or moved anywhere in the target
is already present. Files with the same content form one group.

**Sync is a dry run unless you explicitly pass `--execute`. yast never deletes
target files or overwrites existing files.** This is content synchronization,
not a directory mirror or a metadata/permissions backup.

## Installation

Download your platform's archive and `SHA256SUMS.txt` from
[GitHub Releases](https://github.com/adambaturo/yast/releases). Archives contain
the binary, README, license, and changelog.

| Platform | Targets |
| --- | --- |
| Linux (static musl) | `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` |
| macOS | `x86_64-apple-darwin`, `aarch64-apple-darwin` |
| Windows | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` |

Verify the downloaded archive before extracting. On Linux, with both files in
the current directory (substitute your version and target):

```sh
archive=yast-v0.1.0-x86_64-unknown-linux-musl.tar.gz
grep "  $archive\$" SHA256SUMS.txt | sha256sum --check --strict -
```

On macOS:

```sh
archive=yast-v0.1.0-aarch64-apple-darwin.tar.gz
grep "  $archive\$" SHA256SUMS.txt | shasum -a 256 --check -
```

On Windows PowerShell:

```powershell
$archive = 'yast-v0.1.0-x86_64-pc-windows-msvc.zip'
$entries = @(Get-Content SHA256SUMS.txt | Where-Object { ($_ -split '\s+', 2)[1] -eq $archive })
if ($entries.Count -ne 1) { throw 'Missing or duplicate checksum' }
$expected = ($entries[0] -split '\s+', 2)[0]
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ine $expected) { throw 'Checksum mismatch' }
Expand-Archive $archive -DestinationPath yast
```

Also verify build provenance with a recent [GitHub CLI](https://cli.github.com/):

```console
gh attestation verify <downloaded-archive> --repo adambaturo/yast --signer-workflow adambaturo/yast/.github/workflows/release-binaries.yml
```

On Unix, extract with `tar -xzf <downloaded-archive>` and place `yast` on your
`PATH`; on Windows, add the extracted directory to `PATH`. Checksums detect
corruption; provenance additionally verifies the producing repository/workflow.

To install from source, install [stable Rust](https://rustup.rs/) and the
platform's native build tools (on Windows, Visual Studio C++ Build Tools and a
Windows SDK), then:

```console
cargo install --git https://github.com/adambaturo/yast --locked
yast --help
```

From a checkout, build without installing with `cargo build --release --locked`, then run
`target/release/yast` (`target\release\yast.exe` on Windows).

## Versioning and releases

`Cargo.toml` is the version source of truth; releases use SemVer tags `vX.Y.Z`.
Until stable, breaking changes bump the `0.x` minor version and features/fixes
bump the patch. Squash-merged PR titles follow Conventional Commits.
release-plz prepares version/changelog PRs and publishes GitHub Releases (not
crates.io); CI then attaches six platform archives, checksums, and provenance.
See [CHANGELOG.md](CHANGELOG.md), [CONTRIBUTING.md](CONTRIBUTING.md), and
[RELEASING.md](RELEASING.md).

## Quick start

```console
yast compare ./photos ./backup
yast dupes ./photos
yast sync ./photos ./backup
yast sync ./photos ./backup --execute --verify
```

`compare` requires two existing directories. `sync` accepts a nonexistent target,
but creates it only when executing a copy. Source and target must not overlap for
sync. Symbolic links are ignored unless `--follow-symlinks` is given.

## Commands

### `yast compare <SOURCE> <TARGET>`

Report content missing from target, extra target content, content common to both,
and duplicates within each side. Each group includes size, digest when available,
and alphabetically sorted relative paths. Content groups are sorted by their first
path. Missing/extra counts are content groups, not physical file counts.

```console
yast compare ./camera ./archive --exclude '*.tmp' --exclude 'cache/**'
yast compare ./camera ./archive --format json --output comparison.json
yast compare ./camera ./archive --format csv > comparison.csv
yast compare ./camera ./archive --hash sha256 --threads 8 --min-size 1MiB
```

Example (source has two copies of `hello`, target has one renamed copy):

```text
Source: 3 files; target: 2 files
missing: 1 content groups
  7 bytes, 1 file(s)
    new.txt
extra: 1 content groups
  5 bytes, 1 file(s)
    old.txt
common: 1 content groups
source_duplicate: 1 content groups
  5 bytes, 2 file(s)
    a.txt
    nested/b.txt
target_duplicate: 0 content groups
```

Exit 1 indicates missing/extra content **or duplicate groups**, even when both
trees contain the same set of content. Empty trees and equal duplicate-free
content sets return 0.

### `yast dupes <DIR>`

Find duplicate content within one tree. Every group contains all matching paths;
zero-byte files are included by default. This command never removes duplicates.

```console
yast dupes ./downloads --min-size 10MB
yast dupes ./downloads --skip-hidden --exclude '*.part'
yast dupes ./downloads --hash xxh3 --format json
```

Returns 1 if duplicates exist, otherwise 0.

### `yast sync <SOURCE> <TARGET>`

Plan or copy source content missing anywhere in target. If source has multiple
copies, only its alphabetically first relative path is selected. Already-present
content is skipped regardless of the target filename. Extras are retained.

```console
yast sync ./photos ./backup                         # preview only
yast sync ./photos ./backup --execute               # preserve relative paths
yast sync ./photos ./backup --execute --verify      # verify staged copies
yast sync ./photos ./backup --layout flatten
yast sync ./photos ./backup --layout flatten --dest-subdir incoming --execute
yast sync ./photos ./backup --layout flatten --max-name-len 100 --execute
```

| Sync option | Default | Meaning |
| --- | --- | --- |
| `--execute` | off | Actually copy; otherwise no directories/files are created |
| `--layout keep\|flatten` | `keep` | Preserve relative paths or flatten into one folder |
| `--dest-subdir PATH` | none | Relative folder inside target, applied to either layout |
| `--max-name-len N` | `64` | Flattened filename limit in Unicode characters; at least 16 |
| `--verify` | off | Re-hash source and staged copy before publishing |

Example dry-run summary:

```text
DRY RUN: no files will be written (use --execute to copy)
planned: /photos/2023/trip/a.jpg -> /backup/2023/trip/a.jpg
Planned: 1; copied: 0; skipped: 2; failed: 0
```

`skipped` counts physical source files not selected for copying: existing content
plus additional source duplicates. `planned` counts selected content groups.
Execute reports each copied/failed action and returns 2 if any copy fails.
Successfully copied files remain in place if other copies fail.

Files are copied in parallel into temporary files in their destination folder,
flushed, assigned their source modification time, optionally verified, and
published without replacing an existing path. A file appearing after planning
causes a failure, never an overwrite. In `keep` layout, an existing destination
with different content is a planning error (use `flatten` to disambiguate).
Symlink destination parents are rejected. Do not run against trees that an
untrusted process can mutate concurrently: metadata checks detect ordinary source
changes, but this is not a filesystem snapshot or a defense against hostile races.
Permissions, ownership, access times, hard links and empty directories are not
preserved.

## Common options

These options can appear before or after a subcommand:

| Option | Default | Meaning |
| --- | --- | --- |
| `--hash blake3\|xxh3\|crc32\|md5\|sha1\|sha256` | `blake3` | Content hash |
| `--threads N` | logical CPU count | Positive worker count for walking, hashing, copying |
| `--format text\|json\|csv` | `text` | Report encoding |
| `--output FILE` | stdout | Create a new report file; existing files are not overwritten |
| `--exclude GLOB` | none | Exclude matching relative paths; repeatable |
| `--skip-hidden` | off | Skip dotfiles/directories and Windows hidden entries |
| `--follow-symlinks` | off | Follow file/directory links, reporting traversal errors/loops |
| `--min-size SIZE` | `0` | Minimum bytes; supports B, KB/MB/GB and KiB/MiB/GiB |
| `--hash-all` | off | Disable size/prefix singleton shortcuts and fully hash all files |
| `--quiet` | off | Suppress progress/diagnostics, not the requested report |
| `-v` | off | Diagnostic worker/hash counters on stderr |
| `-h`, `--help` | | Command help (long form includes explanations) |
| `-V`, `--version` | | Version information |

Filters apply to both sides, so excluded/too-small content is not considered
present in target. Ignore files such as `.gitignore` are not consulted; hidden
entries (including `.git`) are included unless excluded. Patterns match relative
paths; quote them to prevent shell expansion. Use `--exclude 'cache'` to prune
that directory and its descendants. Progress appears on stderr only when it is a
terminal and `--quiet` is absent, keeping stdout suitable for pipelines.

## Flatten naming

1. Sanitize each relative path component and join with underscores:
   `2023/trip/a.jpg` becomes `2023_trip_a.jpg`.
2. Replace control characters and Windows-illegal characters (`<>:"/\|?*`)
   with `_`; remove trailing spaces/dots. Prefix Windows reserved basenames
   (`CON`, `NUL`, `PRN`, `AUX`, `COM1`-`COM9`, `LPT1`-`LPT9`) with `_`.
3. If too long, trim the stem from the **left**, retaining nearest directories
   and filename, and preserve the final extension. An extension too large to
   fit is shortened as a last resort. Length is Unicode characters, not bytes.
4. On an existing or planned filename collision, append a deterministic short
   hash suffix before the extension, shortening the stem as needed. For example:
   `2023_trip_a_3fa9c1d204.jpg`. Further collisions use another suffix. Planned
   names are compared case-insensitively for portability.

Physical filesystem byte/UTF-16 name limits still apply; lower `--max-name-len`
for unusually long Unicode names or deep destination paths.

## Machine-readable reports

JSON has `command`, `comparison`, and (for sync) `sync`:

```json
{
  "command": "compare",
  "comparison": {
    "missing": [{"size": 3, "digest": null, "paths": ["new.txt"]}],
    "extra": [],
    "common": [],
    "source_duplicates": [],
    "target_duplicates": [],
    "source_files": 1,
    "target_files": 0,
    "hashed_files": 0,
    "partial_hashed_files": 0
  }
}
```

`digest: null` means hashing was unnecessary to establish uniqueness.
`common.paths` are source paths; extra/target duplicate paths are target-relative.
`dupes` leaves missing/extra/common empty. Sync adds `dry_run`, `planned`,
`copied`, `skipped`, `failed`, and `actions` with absolute `source`/`destination`,
`size`, `status` (`planned`, `copied`, `failed`) and nullable `error`.

CSV uses columns
`category,group,size,digest,path,destination,status,error`.
There is one row per content-group member, copy rows for sync actions, and a sync
summary row with counters in `path`. Group numbers are local to each category;
empty digests indicate skipped hashes. CSV quoting handles commas and newlines.
Operational errors go to stderr and return 2; per-copy failures are also included
in the sync report.

## Performance and correctness

The parallel walker collects only path, size and modification time. Size counts
across both trees immediately eliminate unique sizes. Same-size candidates are
hashed over the first 64 KiB, then only matching prefixes require full streaming
hashes. Small files need just one read. Full reads use chunks up to 1 MiB;
per-file buffers shrink for small files. Rayon bounds hashing/copy concurrency.
Buffered streaming avoids mmap hazards when files change during scanning.
Memory is proportional to file/path counts and worker buffers, not file contents.
The current implementation holds the inventory and report in memory rather than
using an on-disk database.

BLAKE3 is the recommended balance of speed and collision resistance. XXH3 uses a
128-bit digest. CRC32/XXH3 are non-cryptographic, and MD5/SHA-1 are legacy choices:
hash collisions can falsely classify different files as equal. Use BLAKE3 or
SHA-256 for important data. Comparison uses `(size, digest)`, not a byte-for-byte
post-hash check. Verification uses the selected algorithm, so it has the same
collision limitations.

For hundreds of thousands of files, use a release build, avoid network storage
when possible, and adjust `--threads` to your disk (many workers may hurt HDDs).
`--hash-all` trades the size/prefix shortcuts for complete digest reporting.
There is no persistent cache; each run observes the current tree. Source
size/mtime changes during hashing/copying are errors, but files modified while
preserving both values cannot be detected reliably.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Compare identical with no duplicates; dupes has none; sync/dry-run succeeded |
| `1` | Compare found differences/duplicates, or dupes found duplicates |
| `2` | Invalid arguments, traversal/hash/report error, planning error, or any sync failure |

## Development

```console
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

CI runs formatting, Clippy and tests on Ubuntu, Windows and macOS.

Licensed under the [MIT license](LICENSE).
