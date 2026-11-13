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

CI checks all three desktop OSes, dependency policy, and the PR title. Resolve
review conversations and keep the branch up to date; `ci-ok` and `pr-title`
are the required checks once the main ruleset is enabled. No approving review
is mandatory, but all applicable checks must pass. Merge using **Squash and
merge** only; do not merge or push directly to `main`. Admin bypass is limited
to the PR flow. Release PRs follow the same checks and merge process.
