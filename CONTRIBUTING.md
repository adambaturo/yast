# Contributing

Open a branch from current `main` and submit a pull request. Use
[Conventional Commits](https://www.conventionalcommits.org/) for commits and
especially the PR title: squash merge uses the PR title as the final commit
title and the PR body as its message.

Examples: `feat: add a comparison option`, `fix: reject overlapping paths`,
`docs: clarify verification`, `ci: update release automation`. Other accepted
types are `style`, `refactor`, `perf`, `test`, `build`, `chore`, and `revert`.
Optional scopes are supported, for example `chore(deps): update dependencies`.
Mark breaking changes with `!` (`feat!: change output format`) and explain them
in the PR body with a `BREAKING CHANGE:` paragraph.

`Cargo.toml` is the single source of truth for SemVer; do not routinely bump it
or edit generated changelog entries in feature PRs. release-plz prepares those
changes. During `0.x`, breaking changes bump minor; features and fixes bump
patch. See [RELEASING.md](RELEASING.md) for promotion to stable.

Releases use SemVer tags `vX.Y.Z`. release-plz prepares version/changelog PRs
and publishes GitHub Releases (not crates.io); CI then attaches six platform
archives, checksums, and provenance. See [CHANGELOG.md](CHANGELOG.md) for release
history and [RELEASING.md](RELEASING.md) for the release process.

## Development setup

Install [stable Rust](https://rustup.rs/) and the platform's native build tools
(on Windows, Visual Studio C++ Build Tools and a Windows SDK).
From a checkout, build without installing:

```console
cargo build --release --locked
```

Run `target/release/yast` (`target\release\yast.exe` on Windows).

## Local checks

Install stable Rust with rustfmt/clippy, then:

```console
rustup component add rustfmt clippy
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo install cargo-deny --locked --version 0.20.2
cargo deny --locked check
```

`deny.toml` checks RustSec advisories, licenses, dependency duplicates (warnings),
and allowed dependency sources. Do not ignore advisories just to make CI pass.
Use `actionlint` when modifying workflow YAML.

CI runs formatting, Clippy, and tests on Ubuntu, Windows, and macOS, and checks
dependency policy and the PR title. Resolve
review conversations and keep the branch up to date; `ci-ok` and `pr-title`
are the required checks once the main ruleset is enabled. No approving review
is mandatory, but all applicable checks must pass. Merge using **Squash and
merge** only; do not merge or push directly to `main`. Admin bypass is limited
to the PR flow. Release PRs follow the same checks and merge process.

## Local debugging in VS Code

Install stable Rust and the platform's native build tools (on Windows, Visual
Studio C++ Build Tools and a Windows SDK), plus the recommended
[CodeLLDB extension](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb).
Open the repository folder in VS Code, set a breakpoint, then choose a `yast:`
profile in **Run and Debug** and press **F5**.

The [launch profiles](.vscode/launch.json) build the debug binary with
`cargo build --locked --bin yast` and prompt for directories. Relative paths
resolve from the repository root; the source defaults to `src`, the comparison
target to `tests`, and the sync target to `yast-debug-output`.
The profiles cover comparison, duplicate reporting, and sync dry runs.
None modifies the selected directories; sync does not pass `--execute`.
Rust backtraces are enabled for all profiles.

## Implementation notes

The parallel walker collects only path, size and modification time. Size counts
across both trees immediately eliminate unique sizes. Same-size candidates are
hashed over the first 64 KiB, then only matching prefixes require full streaming
hashes. Small files need just one read. Full reads use chunks up to 1 MiB;
per-file buffers shrink for small files. Rayon bounds hashing/copy concurrency.
Buffered streaming avoids mmap hazards when files change during scanning.
Memory is proportional to file/path counts and worker buffers, not file contents.
The current implementation holds the inventory and report in memory rather than
using an on-disk database.

Files remain streamed with bounded buffers; interactive copying uses buffered
I/O to report successful writes, while hidden progress retains native
file-to-file copy optimizations where available.

Duplicate deletion uses the `trash` crate for OS trash integration. Moves use
the platform's atomic no-replace rename on the same filesystem; `fs::rename`
can overwrite, so it is not used directly.
