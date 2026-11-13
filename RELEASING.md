# Releasing yast

## Version policy

`Cargo.toml` is the single source of truth; `Cargo.lock` records the same package
version. Tags are `vX.Y.Z`. Stay on `0.x` until the CLI/output contract is stable:
breaking Conventional Commits (`!` or `BREAKING CHANGE:`) bump minor, features
and fixes bump patch. After 1.0, use normal SemVer (breaking/feature/fix means
major/minor/patch). Keep PR titles conventional because only squash merges are
allowed. release-plz generates Keep a Changelog entries in `CHANGELOG.md`.

## One-time setup

An administrator must create the repository Actions secret `RELEASE_PLZ_TOKEN`.
Use a fine-grained PAT restricted to `adambaturo/yast` with **Contents** and
**Pull requests** read/write (Metadata read is implicit). Add **Workflows**
read/write if GitHub requires it when updating workflow files. Set an expiry
and rotate the secret before it expires. Never paste the value into a PR,
issue, chat, source file, or command log.

The PAT must belong to a repository admin: the tag ruleset permits only admins
to create `v*` tags. It uses `bypass_mode: always`, so admins can also override
tag update/deletion restrictions; never use that bypass for published releases.
The main ruleset's admin bypass is `pull_request`, not a direct-push exception.

The workflow uses the PAT for checkout and release-plz's `GITHUB_TOKEN` env.
Using the normal Actions token instead would suppress downstream PR CI and
release-triggered binary builds. No crates.io credentials are used:
`git_only = true` discovers versions from tags and `publish = false` prevents
registry publishing.

After this setup PR is merged and its required checks exist, an admin can apply
the stored rulesets from the repository root in PowerShell:

```powershell
gh api --method POST repos/adambaturo/yast/rulesets --input .github\rulesets\main.json
gh api --method POST repos/adambaturo/yast/rulesets --input .github\rulesets\tags.json
```

Do not run before merge or repeatedly create duplicates. To update an existing
ruleset, find its ID with `gh api repos/adambaturo/yast/rulesets` and use `PUT`
on `repos/adambaturo/yast/rulesets/ID` with the same input. Main requires
`ci-ok` and `pr-title` with strict up-to-date checks, resolved conversations,
PRs (zero mandatory approvals), linear history, and no force pushes/deletion.
The title job is intentionally skipped on main pushes; `ci-ok` accepts that
skip only for non-PR runs. Tag creation, update, force-update, and deletion
are restricted by the tag ruleset.

Repository merge settings (safe to apply before rulesets):

```powershell
gh api --method PATCH repos/adambaturo/yast -F allow_squash_merge=true -F allow_merge_commit=false -F allow_rebase_merge=false -f squash_merge_commit_title=PR_TITLE -f squash_merge_commit_message=PR_BODY -F delete_branch_on_merge=true -F allow_auto_merge=true
gh api --method PUT repos/adambaturo/yast/actions/permissions/workflow -f default_workflow_permissions=read -F can_approve_pull_request_reviews=true
```

Organization/account policy may forbid the second request; report the error
instead of weakening policy. The explicit PAT still authorizes release-plz.

## Automatic release

On each main push, the serialized release workflow first runs `release-plz
release` against that exact commit with complete tag history. A missing tag for
the current manifest version is released, so the first push with this setup
publishes **v0.1.0**, using its seeded changelog entry. Existing version tags
are not recreated. `release_always = true` enables this bootstrap; version bumps
should therefore only reach main through reviewed release PRs.

Then `release-plz release-pr` opens/updates a conventional release PR containing
`Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` updates. Review and squash-merge
it after CI passes; that main push publishes the new tag and GitHub Release
using the changelog notes. Ordinary pushes with an already-tagged version
only prepare the next release PR. Serialization and release-before-release-pr
ordering prevent a competing PR from bumping the initial version first.

The `release: published` event starts six builds: static musl Linux x64/ARM64
(cargo-zigbuild), macOS Intel/Apple Silicon, and Windows MSVC x64/ARM64. All use
the tag's source, locked dependencies, and the release profile. Archives are
`yast-vX.Y.Z-<target>.tar.gz` (Windows: `.zip`) with the binary, README, LICENSE,
and CHANGELOG at the root. A final job calculates `SHA256SUMS.txt`, attests all
archives/checksums with GitHub build provenance, and uploads them to the release.
No release assets are uploaded until every platform succeeds.

## Recovery and manual fallback

If the token was missing/expired, create/rotate the secret, then rerun the
failed Release workflow from Actions. Do not bump the version just to retry.

For a published release with missing assets, run the binaries workflow against
an existing tag (or use **Actions > Release binaries > Run workflow**):

```console
gh workflow run release-binaries.yml --repo adambaturo/yast -f tag=v0.1.0
```

The fallback validates a real published release and requires the checked-out
manifest version to match its tag. Reruns rebuild all archives and replace
existing same-named assets/checksums (`--clobber`); verify new attestations.
Prefer rerunning failed jobs before rebuilding an already-complete release.

If automatic tag/release creation cannot be recovered, an admin may manually
tag the intended reviewed main commit after confirming the version and
changelog. A tag push alone does **not** trigger asset builds; publish a GitHub
Release as well. Use your admin identity (not an Actions token) so the published
event triggers the binary workflow:

```console
git fetch origin main --tags
git tag -a v0.1.0 <reviewed-main-commit> -m "Release v0.1.0"
git push origin v0.1.0
gh release create v0.1.0 --repo adambaturo/yast --verify-tag --title v0.1.0 --notes-file release-notes.txt
```

Replace the version/commit, and prepare `release-notes.txt` from that version's
changelog section. Never move a published tag; fix forward with a new version.
If the release exists but its event did not start builds, dispatch manually.

## Cutting 1.0.0

Decide that CLI, exit codes, and machine-readable formats are stable. Update an
open release-plz PR (or create a conventional `chore: release v1.0.0` PR) to set
`Cargo.toml` to `1.0.0`, run `cargo check` to update the workspace lockfile
version, and add/review the `1.0.0` changelog entry including migration notes.
Do not leave a competing automated release PR open. Run the normal checks,
squash-merge, and let the existing pipeline create `v1.0.0` and its assets.
